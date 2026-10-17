//! Lado do controlador: aceita os participantes, conhece os dispositivos de
//! cada um e abre streams neles como se fossem locais.
//!
//! * microfone remoto: pede ao participante que capture e mande os pacotes;
//!   eles chegam pelo [`AudioSocket`] direto no callback do motor;
//! * alto-falante remoto: uma thread puxa o áudio do motor no ritmo do relógio
//!   deste computador (5 ms por pacote) e manda ao participante, que cuida do
//!   buffer e do drift do lado de lá.

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use audioroute_core::audio::{CHANNELS, CaptureCallback, RenderCallback, SAMPLE_RATE, StreamError};
use audioroute_core::{DeviceInfo, Stream, StreamInfo, StreamRequest};
use serde::Serialize;
use uuid::Uuid;

use crate::audio::AudioSocket;
use crate::discovery::Announcer;
use crate::message::{self, DeviceDesc, InfoDesc, Msg, PROTOCOL};
use crate::packet::MAX_FRAMES;
use crate::{NET_ALLOWANCE_FRAMES, PEER_TIMEOUT, PING_INTERVAL};

/// Quanto esperar o participante confirmar que abriu o dispositivo.
const ACK_TIMEOUT: Duration = Duration::from_secs(3);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct HubOptions {
    /// Porta TCP de controle (0: qualquer uma livre).
    pub control_port: u16,
    /// Anunciar na rede local (desligado nos testes).
    pub announce: bool,
}

impl Default for HubOptions {
    fn default() -> Self {
        Self { control_port: message::CONTROL_PORT, announce: true }
    }
}

/// Um participante conectado, como a interface o vê.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub name: String,
    pub address: String,
    pub devices: usize,
}

/// Barato de clonar.
#[derive(Clone)]
pub struct NetHub {
    hub: Arc<Hub>,
}

struct Hub {
    name: String,
    control_port: u16,
    audio: AudioSocket,
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    next_session: AtomicU64,
    next_stream: AtomicU32,
    changed: AtomicBool,
    stop: AtomicBool,
    announcer: Mutex<Option<Announcer>>,
}

struct Session {
    name: String,
    ip: IpAddr,
    audio_addr: SocketAddr,
    writer: Mutex<TcpStream>,
    devices: Mutex<Vec<DeviceDesc>>,
    /// Respostas esperadas a um Start*.
    pending: Mutex<HashMap<u32, mpsc::Sender<Result<InfoDesc, String>>>>,
    /// Streams abertos neste participante (o flag cai se ele sair).
    streams: Mutex<HashMap<u32, Arc<AtomicBool>>>,
}

impl Session {
    fn send(&self, msg: &Msg) -> io::Result<()> {
        message::send(&mut *lock(&self.writer), msg)
    }

    /// Envia o pedido de abertura e espera o participante responder.
    fn request(&self, stream: u32, msg: Msg) -> Result<InfoDesc, String> {
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(stream, tx);
        if self.send(&msg).is_err() {
            lock(&self.pending).remove(&stream);
            return Err(format!("{} desconectou", self.name));
        }
        let result = rx.recv_timeout(ACK_TIMEOUT).unwrap_or_else(|e| {
            Err(match e {
                mpsc::RecvTimeoutError::Timeout => format!("{} não respondeu", self.name),
                mpsc::RecvTimeoutError::Disconnected => format!("{} desconectou", self.name),
            })
        });
        lock(&self.pending).remove(&stream);
        result
    }
}

impl NetHub {
    pub fn start(name: String, machine_id: String, opts: HubOptions) -> io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", opts.control_port)).or_else(|_| TcpListener::bind(("0.0.0.0", 0)))?;
        listener.set_nonblocking(true)?;
        let control_port = listener.local_addr()?.port();
        let hub = Arc::new(Hub {
            name: name.clone(),
            control_port,
            audio: AudioSocket::bind()?,
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            next_stream: AtomicU32::new(1),
            changed: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            announcer: Mutex::new(None),
        });
        if opts.announce {
            *lock(&hub.announcer) = Some(Announcer::start(machine_id, name, control_port)?);
        }

        let h = hub.clone();
        std::thread::Builder::new().name("net-accept".into()).spawn(move || {
            while !h.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let h = h.clone();
                        let _ = std::thread::Builder::new()
                            .name("net-session".into())
                            .spawn(move || run_session(&h, stream));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        })?;

        let h = hub.clone();
        std::thread::Builder::new().name("net-ping".into()).spawn(move || {
            while !h.stop.load(Ordering::SeqCst) {
                std::thread::sleep(PING_INTERVAL);
                let sessions: Vec<_> = lock(&h.sessions).values().cloned().collect();
                for s in sessions {
                    let _ = s.send(&Msg::Ping);
                }
            }
        })?;

        Ok(Self { hub })
    }

    pub fn control_port(&self) -> u16 {
        self.hub.control_port
    }

    /// Para tudo: anúncio, conexões e streams.
    pub fn shutdown(&self) {
        let h = &self.hub;
        h.stop.store(true, Ordering::SeqCst);
        lock(&h.announcer).take();
        for s in lock(&h.sessions).values() {
            let _ = lock(&s.writer).shutdown(Shutdown::Both);
        }
        h.audio.shutdown();
    }

    /// Algum participante entrou, saiu ou mudou de dispositivos desde a
    /// última chamada.
    pub fn take_changed(&self) -> bool {
        self.hub.changed.swap(false, Ordering::SeqCst)
    }

    pub fn peers(&self) -> Vec<PeerView> {
        let mut out: Vec<_> = lock(&self.hub.sessions)
            .values()
            .map(|s| PeerView { name: s.name.clone(), address: s.ip.to_string(), devices: lock(&s.devices).len() })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Dispositivos de todos os participantes conectados.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        lock(&self.hub.sessions)
            .values()
            .flat_map(|s| lock(&s.devices).iter().map(|d| d.to_info(&s.name)).collect::<Vec<_>>())
            .collect()
    }

    /// O dispositivo é de um participante?
    pub fn owns(&self, id: Uuid) -> bool {
        self.session_of(id).is_some()
    }

    fn session_of(&self, id: Uuid) -> Option<Arc<Session>> {
        lock(&self.hub.sessions).values().find(|s| lock(&s.devices).iter().any(|d| d.id == id)).cloned()
    }

    fn new_stream(&self) -> u32 {
        self.hub.next_stream.fetch_add(1, Ordering::SeqCst)
    }

    pub fn open_capture(&self, id: Uuid, request: StreamRequest, callback: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        let session = self.session_of(id).ok_or("dispositivo da rede não está conectado")?;
        let stream = self.new_stream();
        self.hub.audio.register(stream, session.ip, callback);
        let info = match session.request(stream, Msg::start_capture(stream, id, request)) {
            Ok(info) => info,
            Err(e) => {
                self.hub.audio.unregister(stream);
                return Err(e.into());
            }
        };
        let alive = Arc::new(AtomicBool::new(true));
        lock(&session.streams).insert(stream, alive.clone());
        let audio = self.hub.audio.clone();
        Ok(Box::new(RemoteStream {
            info: StreamInfo {
                mode: info.mode,
                // A folga da rede entra como "período": o motor dimensiona o
                // buffer do cabo com ela.
                period_frames: info.period_frames + NET_ALLOWANCE_FRAMES,
                max_callback_frames: MAX_FRAMES as u32,
                device_latency_frames: info.device_latency_frames,
                native_format: format!("rede · {}", info.format),
            },
            alive,
            on_drop: Some(Box::new(move || {
                audio.unregister(stream);
                let _ = session.send(&Msg::Stop { stream });
                lock(&session.streams).remove(&stream);
            })),
        }))
    }

    pub fn open_render(&self, id: Uuid, request: StreamRequest, mut callback: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        let session = self.session_of(id).ok_or("dispositivo da rede não está conectado")?;
        let stream = self.new_stream();
        let info = session.request(stream, Msg::start_render(stream, id, request))?;
        let alive = Arc::new(AtomicBool::new(true));
        lock(&session.streams).insert(stream, alive.clone());

        // Puxa 5 ms do motor a cada 5 ms do relógio deste computador.
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, running) = (stop.clone(), alive.clone());
        let mut sender = self.hub.audio.sender(session.audio_addr, stream);
        std::thread::Builder::new().name("net-render".into()).spawn(move || {
            let mut buf = vec![0.0f32; MAX_FRAMES * CHANNELS];
            let start = Instant::now();
            let mut sent: u64 = 0;
            while !flag.load(Ordering::SeqCst) && running.load(Ordering::SeqCst) {
                let due = (start.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64;
                // Muito atrasado (computador travou): pula em vez de despejar uma rajada.
                if due > sent + SAMPLE_RATE as u64 / 5 {
                    sent = due - MAX_FRAMES as u64;
                }
                while sent + MAX_FRAMES as u64 <= due {
                    callback(&mut buf);
                    sender.send(&buf);
                    sent += MAX_FRAMES as u64;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })?;

        Ok(Box::new(RemoteStream {
            info: StreamInfo {
                mode: info.mode,
                period_frames: MAX_FRAMES as u32,
                max_callback_frames: MAX_FRAMES as u32,
                // O buffer e o driver de lá (o participante informa).
                device_latency_frames: info.device_latency_frames,
                native_format: format!("rede · {}", info.format),
            },
            alive,
            on_drop: Some(Box::new(move || {
                stop.store(true, Ordering::SeqCst);
                let _ = session.send(&Msg::Stop { stream });
                lock(&session.streams).remove(&stream);
            })),
        }))
    }
}

/// Stream aberto num participante.
struct RemoteStream {
    info: StreamInfo,
    alive: Arc<AtomicBool>,
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl Stream for RemoteStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for RemoteStream {
    fn drop(&mut self) {
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}

/// Conversa com um participante até ele sair.
fn run_session(hub: &Arc<Hub>, stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(PEER_TIMEOUT));
    let Ok(peer) = stream.peer_addr() else { return };
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);

    let Ok(Some(Msg::Hello { protocol, name, audio_port, devices, .. })) = message::recv(&mut reader) else { return };
    if protocol != PROTOCOL {
        return;
    }
    let session = Arc::new(Session {
        name,
        ip: peer.ip(),
        audio_addr: SocketAddr::new(peer.ip(), audio_port),
        writer: Mutex::new(stream),
        devices: Mutex::new(devices),
        pending: Mutex::new(HashMap::new()),
        streams: Mutex::new(HashMap::new()),
    });
    if session.send(&Msg::Welcome { name: hub.name.clone(), audio_port: hub.audio.port() }).is_err() {
        return;
    }
    let id = hub.next_session.fetch_add(1, Ordering::SeqCst);
    lock(&hub.sessions).insert(id, session.clone());
    hub.changed.store(true, Ordering::SeqCst);

    while let Ok(Some(msg)) = message::recv(&mut reader) {
        match msg {
            Msg::Devices { devices } => {
                *lock(&session.devices) = devices;
                hub.changed.store(true, Ordering::SeqCst);
            }
            Msg::Started { stream, info } => {
                if let Some(tx) = lock(&session.pending).remove(&stream) {
                    let _ = tx.send(Ok(info));
                }
            }
            Msg::StreamError { stream, error } => {
                if let Some(tx) = lock(&session.pending).remove(&stream) {
                    let _ = tx.send(Err(error));
                } else if let Some(alive) = lock(&session.streams).get(&stream) {
                    alive.store(false, Ordering::SeqCst); // o motor fecha e tenta de novo
                }
            }
            _ => {}
        }
    }

    // Saiu (fechou, caiu ou ficou mudo além do PEER_TIMEOUT).
    for alive in lock(&session.streams).values() {
        alive.store(false, Ordering::SeqCst);
    }
    hub.changed.store(true, Ordering::SeqCst);
    lock(&hub.sessions).remove(&id);
    lock(&session.pending).clear();
    let _ = lock(&session.writer).shutdown(Shutdown::Both);
}
