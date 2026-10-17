//! Lado do participante: acha o controlador, conecta, informa os
//! dispositivos e faz o que ele pedir.
//!
//! * captura: abre o dispositivo local e manda cada bloco por UDP;
//! * reprodução: um motor local liga uma "entrada de rede" (os pacotes que
//!   chegam) ao dispositivo, com o mesmo buffer e a mesma compensação de drift
//!   de um cabo comum — o relógio do controlador e o daqui nunca batem.
//!
//! Se a conexão cair, fecha tudo e volta a procurar.

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use audioroute_core::audio::{CaptureCallback, RenderCallback, StreamError};
use audioroute_core::{DeviceLister, Engine, ShareMode, Stream, StreamInfo, StreamOpener, StreamRequest};
use serde::Serialize;
use uuid::Uuid;

use crate::audio::AudioSocket;
use crate::discovery::Discovery;
use crate::message::{self, DeviceDesc, InfoDesc, Msg, PROTOCOL};
use crate::packet::MAX_FRAMES;
use crate::{NET_ALLOWANCE_FRAMES, PEER_TIMEOUT, PING_INTERVAL};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY_DELAY: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct ParticipantOptions {
    /// Buffer dos cabos de reprodução, em ms.
    pub buffer_ms: u32,
    /// Endereço fixo do controlador. `None`: descobrir na rede.
    pub controller: Option<SocketAddr>,
}

impl Default for ParticipantOptions {
    fn default() -> Self {
        Self { buffer_ms: 30, controller: None }
    }
}

/// Estado mostrado na interface do participante.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantStatus {
    /// "searching", "connected" ou "error".
    pub state: String,
    pub controller: Option<String>,
    pub address: Option<String>,
    /// Streams abertos agora a pedido do controlador.
    pub streams: usize,
    pub error: Option<String>,
}

/// Barato de clonar.
#[derive(Clone)]
pub struct Participant {
    inner: Arc<Inner>,
}

struct Inner {
    status: Mutex<ParticipantStatus>,
    stop: AtomicBool,
    /// Conexão atual (para o shutdown poder interrompê-la).
    conn: Mutex<Option<TcpStream>>,
}

impl Participant {
    pub fn start<B>(backend: B, machine_id: String, name: String, opts: ParticipantOptions) -> Self
    where
        B: DeviceLister + StreamOpener + Clone + Send + 'static,
    {
        let inner = Arc::new(Inner {
            status: Mutex::new(ParticipantStatus { state: "searching".into(), ..Default::default() }),
            stop: AtomicBool::new(false),
            conn: Mutex::new(None),
        });
        let me = inner.clone();
        std::thread::Builder::new()
            .name("net-participant".into())
            .spawn(move || main_loop(&me, backend, &machine_id, &name, &opts))
            .expect("criar thread do participante");
        Self { inner }
    }

    pub fn status(&self) -> ParticipantStatus {
        lock(&self.inner.status).clone()
    }

    /// Desconecta e para de procurar.
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        if let Some(c) = lock(&self.inner.conn).as_ref() {
            let _ = c.shutdown(Shutdown::Both);
        }
    }
}

fn set_status(inner: &Inner, f: impl FnOnce(&mut ParticipantStatus)) {
    f(&mut lock(&inner.status));
}

fn main_loop<B>(inner: &Arc<Inner>, backend: B, machine_id: &str, name: &str, opts: &ParticipantOptions)
where
    B: DeviceLister + StreamOpener + Clone + Send + 'static,
{
    let discovery = match opts.controller {
        Some(_) => None,
        None => match Discovery::listen() {
            Ok(d) => Some(d),
            Err(e) => {
                set_status(inner, |s| {
                    s.state = "error".into();
                    s.error = Some(format!("não foi possível escutar a rede (porta {}): {e}", message::DISCOVERY_PORT));
                });
                return;
            }
        },
    };

    while !inner.stop.load(Ordering::SeqCst) {
        set_status(inner, |s| *s = ParticipantStatus { state: "searching".into(), ..Default::default() });
        let addr = match (&discovery, opts.controller) {
            (_, Some(addr)) => addr,
            (Some(d), None) => match d.next(Duration::from_millis(500)) {
                Some((_, addr)) => addr,
                None => continue,
            },
            (None, None) => unreachable!(),
        };
        if let Ok(stream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            let error = run_session(inner, &backend, machine_id, name, opts, stream).err();
            set_status(inner, |s| s.error = error.map(|e| e.to_string()));
        }
        *lock(&inner.conn) = None;
        if !inner.stop.load(Ordering::SeqCst) {
            std::thread::sleep(RETRY_DELAY);
        }
    }
}

/// Estado de uma conexão com o controlador.
struct Session {
    writer: Mutex<TcpStream>,
    /// Capturas locais enviadas ao controlador: stream → dispositivo aberto.
    captures: Mutex<HashMap<u32, Box<dyn Stream>>>,
    /// Reproduções: stream → dispositivo local onde toca.
    renders: Mutex<HashMap<u32, Uuid>>,
    engine: Mutex<Engine>,
}

impl Session {
    fn send(&self, msg: &Msg) -> io::Result<()> {
        message::send(&mut *lock(&self.writer), msg)
    }

    fn stream_count(&self) -> usize {
        lock(&self.captures).len() + lock(&self.renders).len()
    }
}

fn device_list<B: DeviceLister>(backend: &B) -> Vec<DeviceDesc> {
    backend.list().map(|l| l.iter().map(DeviceDesc::from_info).collect()).unwrap_or_default()
}

fn run_session<B>(
    inner: &Arc<Inner>,
    backend: &B,
    machine_id: &str,
    name: &str,
    opts: &ParticipantOptions,
    stream: TcpStream,
) -> io::Result<()>
where
    B: DeviceLister + StreamOpener + Clone + Send + 'static,
{
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(PEER_TIMEOUT))?;
    let peer = stream.peer_addr()?;
    *lock(&inner.conn) = Some(stream.try_clone()?);
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let audio = AudioSocket::bind()?;
    let mut devices = device_list(backend);
    message::send(
        &mut writer,
        &Msg::Hello {
            protocol: PROTOCOL,
            machine_id: machine_id.into(),
            name: name.into(),
            audio_port: audio.port(),
            devices: devices.clone(),
        },
    )?;
    let Some(Msg::Welcome { name: controller, audio_port }) = message::recv(&mut reader)? else {
        audio.shutdown();
        return Err(io::Error::other("o controlador não respondeu ao Hello"));
    };
    let controller_audio = SocketAddr::new(peer.ip(), audio_port);
    set_status(inner, |s| {
        s.state = "connected".into();
        s.controller = Some(controller);
        s.address = Some(peer.ip().to_string());
    });

    let opener = SourceOpener { local: backend.clone(), audio: audio.clone(), peer: peer.ip() };
    let session = Arc::new(Session {
        writer: Mutex::new(writer),
        captures: Mutex::new(HashMap::new()),
        renders: Mutex::new(HashMap::new()),
        engine: Mutex::new(Engine::new(Box::new(opener), opts.buffer_ms)),
    });

    // Mantém a conexão viva, avisa dispositivos novos e streams que caíram.
    let done = Arc::new(AtomicBool::new(false));
    let keepalive = {
        let (session, done, backend, inner) = (session.clone(), done.clone(), backend.clone(), inner.clone());
        std::thread::Builder::new().name("net-keepalive".into()).spawn(move || {
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(PING_INTERVAL);
                let _ = session.send(&Msg::Ping);
                let now = device_list(&backend);
                if now != devices {
                    devices = now;
                    let _ = session.send(&Msg::Devices { devices: devices.clone() });
                }
                report_dead_streams(&session);
                set_status(&inner, |s| s.streams = session.stream_count());
            }
        })?
    };

    while let Some(msg) = message::recv(&mut reader)? {
        match msg {
            Msg::StartCapture { stream, device, mode, period_frames } => {
                let mut sender = audio.sender(controller_audio, stream);
                let callback: CaptureCallback = Box::new(move |samples| sender.send(samples));
                let request = StreamRequest { mode, period_frames };
                let reply = match backend.open_capture(device, request, callback) {
                    Ok(s) => {
                        let info = InfoDesc::from(s.info());
                        lock(&session.captures).insert(stream, s);
                        Msg::Started { stream, info }
                    }
                    Err(e) => Msg::StreamError { stream, error: e.to_string() },
                };
                session.send(&reply)?;
            }
            Msg::StartRender { stream, device, mode, period_frames } => {
                let reply = start_render(&session, stream, device, StreamRequest { mode, period_frames });
                session.send(&reply)?;
            }
            Msg::Stop { stream } => {
                let capture = lock(&session.captures).remove(&stream);
                drop(capture);
                if let Some(device) = lock(&session.renders).remove(&stream) {
                    let _ = lock(&session.engine).disconnect(source_id(stream), device);
                }
            }
            _ => {}
        }
        set_status(inner, |s| s.streams = session.stream_count());
    }

    done.store(true, Ordering::SeqCst);
    let _ = keepalive.join();
    lock(&session.captures).clear();
    lock(&session.renders).clear();
    *lock(&session.engine) = Engine::new(Box::new(NoOpener), opts.buffer_ms); // fecha os dispositivos
    audio.shutdown();
    Ok(())
}

fn start_render(session: &Session, stream: u32, device: Uuid, request: StreamRequest) -> Msg {
    let mut engine = lock(&session.engine);
    let source = source_id(stream);
    let _ = engine.set_device_request(device, request);
    match engine.connect(source, device, 1.0, false) {
        Ok(()) => {
            lock(&session.renders).insert(stream, device);
            let mut info = engine.device_info(device).map(InfoDesc::from).unwrap_or(InfoDesc {
                mode: request.mode,
                period_frames: 0,
                device_latency_frames: 0,
                format: String::new(),
            });
            // O controlador soma isto à latência do cabo de lá: buffer daqui + driver.
            if let Some(lat) = engine.estimated_latency(source, device) {
                info.device_latency_frames = (lat.as_secs_f64() * audioroute_core::audio::SAMPLE_RATE as f64) as u32;
            }
            Msg::Started { stream, info }
        }
        Err(e) => Msg::StreamError { stream, error: e.to_string() },
    }
}

/// Avisa o controlador dos streams que pararam sozinhos (dispositivo
/// desconectado, driver reiniciado...). Ele fecha e tenta reabrir.
fn report_dead_streams(session: &Session) {
    let dead: Vec<u32> = lock(&session.captures).iter().filter(|(_, s)| !s.is_alive()).map(|(id, _)| *id).collect();
    for stream in dead {
        lock(&session.captures).remove(&stream);
        let _ = session.send(&Msg::StreamError { stream, error: "o dispositivo parou".into() });
    }
    let failed = lock(&session.engine).failed_devices();
    for device in failed {
        lock(&session.engine).detach_device(device);
        let streams: Vec<u32> = lock(&session.renders).iter().filter(|(_, d)| **d == device).map(|(s, _)| *s).collect();
        for stream in streams {
            lock(&session.renders).remove(&stream);
            let _ = session.send(&Msg::StreamError { stream, error: "o dispositivo parou".into() });
        }
    }
}

// ------------------------------------------------- entrada de rede no motor

/// Prefixo dos ids das "entradas de rede" do motor local ("netsrc").
const SOURCE_TAG: u64 = 0x6e65_7473_7263_0000;

fn source_id(stream: u32) -> Uuid {
    Uuid::from_u64_pair(SOURCE_TAG, stream as u64)
}

fn source_stream(id: Uuid) -> Option<u32> {
    let (hi, lo) = id.as_u64_pair();
    (hi == SOURCE_TAG).then_some(lo as u32)
}

/// Abre os dispositivos locais e, para os ids de [`source_id`], uma entrada
/// alimentada pelos pacotes do controlador.
struct SourceOpener<B> {
    local: B,
    audio: AudioSocket,
    peer: IpAddr,
}

impl<B: StreamOpener> StreamOpener for SourceOpener<B> {
    fn open_capture(&self, id: Uuid, request: StreamRequest, callback: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        let Some(stream) = source_stream(id) else { return self.local.open_capture(id, request, callback) };
        self.audio.register(stream, self.peer, callback);
        Ok(Box::new(SourceStream {
            audio: self.audio.clone(),
            stream,
            info: StreamInfo {
                mode: ShareMode::Shared,
                period_frames: NET_ALLOWANCE_FRAMES,
                max_callback_frames: MAX_FRAMES as u32,
                device_latency_frames: 0,
                native_format: "rede · PCM 16 bits".into(),
            },
        }))
    }

    fn open_render(&self, id: Uuid, request: StreamRequest, callback: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        self.local.open_render(id, request, callback)
    }
}

struct SourceStream {
    audio: AudioSocket,
    stream: u32,
    info: StreamInfo,
}

impl Stream for SourceStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}

impl Drop for SourceStream {
    fn drop(&mut self) {
        self.audio.unregister(self.stream);
    }
}

/// Opener vazio: só para trocar o motor e fechar tudo ao desconectar.
struct NoOpener;

impl StreamOpener for NoOpener {
    fn open_capture(&self, _: Uuid, _: StreamRequest, _: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        Err("desconectado".into())
    }
    fn open_render(&self, _: Uuid, _: StreamRequest, _: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        Err("desconectado".into())
    }
}
