//! Socket UDP de áudio: envia e recebe os pacotes de [`crate::packet`].
//!
//! Cada lado tem um só socket. Quem recebe registra um callback por stream
//! (o mesmo `CaptureCallback` que um microfone usaria): o motor não sabe que
//! o áudio veio da rede. Pacote perdido vira silêncio; pacote atrasado é
//! descartado.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use audioroute_core::audio::{CHANNELS, CaptureCallback, SAMPLE_RATE};

use crate::packet::{self, MAX_FRAMES, MAX_PACKET};

/// Buraco maior que isto não é preenchido com silêncio: o stream recomeça
/// (o motor ressincroniza o cabo sozinho).
const MAX_GAP_FRAMES: u64 = SAMPLE_RATE as u64 / 2;

struct Receiver {
    /// Só aceita pacotes deste computador.
    peer: IpAddr,
    callback: CaptureCallback,
    next_pos: Option<u64>,
    scratch: Vec<f32>,
}

struct Inner {
    socket: UdpSocket,
    receivers: Mutex<HashMap<u32, Receiver>>,
    stop: AtomicBool,
}

/// Barato de clonar (todos os clones usam o mesmo socket).
#[derive(Clone)]
pub struct AudioSocket {
    inner: Arc<Inner>,
}

impl AudioSocket {
    /// Abre numa porta livre e começa a receber.
    pub fn bind() -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let inner = Arc::new(Inner { socket, receivers: Mutex::new(HashMap::new()), stop: AtomicBool::new(false) });
        let rx = inner.clone();
        std::thread::Builder::new().name("net-audio-rx".into()).spawn(move || receive_loop(&rx))?;
        Ok(Self { inner })
    }

    pub fn port(&self) -> u16 {
        self.inner.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Passa a entregar o áudio do stream ao callback.
    pub fn register(&self, stream: u32, peer: IpAddr, callback: CaptureCallback) {
        let r = Receiver { peer, callback, next_pos: None, scratch: vec![0.0; MAX_FRAMES * CHANNELS] };
        self.lock().insert(stream, r);
    }

    /// Para de entregar. O callback é liberado aqui, fora da thread de rede.
    pub fn unregister(&self, stream: u32) {
        let removed = self.lock().remove(&stream);
        drop(removed);
    }

    /// Envia o áudio de um stream para `dest`.
    pub fn sender(&self, dest: SocketAddr, stream: u32) -> PacketSender {
        PacketSender { inner: self.inner.clone(), dest, stream, pos: 0, buf: Box::new([0; MAX_PACKET]) }
    }

    /// Para a thread de recepção (o socket fecha quando o último clone sair).
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Receiver>> {
        self.inner.receivers.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn receive_loop(inner: &Inner) {
    let mut buf = [0u8; 2048];
    while !inner.stop.load(Ordering::SeqCst) {
        let Ok((n, from)) = inner.socket.recv_from(&mut buf) else { continue };
        let packet = &buf[..n];
        let Some(h) = packet::decode_header(packet) else { continue };
        let mut map = inner.receivers.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = map.get_mut(&h.stream) else { continue };
        if from.ip() != r.peer {
            continue;
        }
        match r.next_pos {
            // Atrasado ou repetido: o lugar dele já passou.
            Some(next) if h.pos < next => continue,
            // Perdido no caminho: silêncio no lugar, para o tempo não encolher.
            Some(next) if h.pos > next && h.pos - next <= MAX_GAP_FRAMES => {
                r.scratch.fill(0.0);
                let mut gap = (h.pos - next) as usize;
                while gap > 0 {
                    let n = gap.min(MAX_FRAMES);
                    (r.callback)(&r.scratch[..n * CHANNELS]);
                    gap -= n;
                }
            }
            _ => {}
        }
        let samples = &mut r.scratch[..h.frames * CHANNELS];
        packet::decode_samples(packet, samples);
        (r.callback)(samples);
        r.next_pos = Some(h.pos + h.frames as u64);
    }
}

/// Envia o áudio de um stream. Não aloca: pode ser usado num callback de áudio.
pub struct PacketSender {
    inner: Arc<Inner>,
    dest: SocketAddr,
    stream: u32,
    pos: u64,
    buf: Box<[u8; MAX_PACKET]>,
}

impl PacketSender {
    pub fn send(&mut self, samples: &[f32]) {
        for chunk in samples.chunks(MAX_FRAMES * CHANNELS) {
            let n = packet::encode(&mut self.buf, self.stream, self.pos, chunk);
            // UDP: se falhar, é um pacote perdido como outro qualquer.
            let _ = self.inner.socket.send_to(&self.buf[..n], self.dest);
            self.pos += (chunk.len() / CHANNELS) as u64;
        }
    }
}
