//! Alto-falante e microfone virtuais do audioroute, lidos e escritos direto
//! no driver (`driver/`), sem passar pelo mixer do Windows.
//!
//! No canvas:
//! * "Alto-falantes (audioroute)" é uma **entrada**: o que os programas tocam
//!   nele entra no audioroute;
//! * "Microfone (audioroute)" é uma **saída**: o que o audioroute manda para
//!   ele sai como microfone no Discord, OBS etc.
//!
//! Os mesmos dispositivos vistos pelo WASAPI ficam escondidos (seriam
//! duplicados com a direção trocada). Sem o driver instalado, nada muda.
//!
//! Cada stream tem uma thread que anda pelo relógio do sistema, 5 ms por
//! vez — o mesmo relógio do driver, então não há drift entre os dois.

pub mod driver;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audioroute_core::audio::{CaptureCallback, RenderCallback, SAMPLE_RATE, StreamError};
use audioroute_core::{
    DeviceInfo, DeviceKind, DeviceLister, IdGenerator, Role, ShareMode, Stream, StreamInfo, StreamOpener, StreamRequest,
};
use uuid::Uuid;

pub use driver::{Driver, OpenError};

/// Os dispositivos do driver terminam com isto no nome do Windows.
const SUFFIX: &str = "(audioroute)";
/// Frames por bloco (5 ms).
const BLOCK: usize = 240;
const CHANNELS: usize = 2;
/// Folga que o driver guarda antes de entregar som (10 ms).
const DRIVER_LATENCY_FRAMES: u32 = 480;

/// É um dispositivo do driver visto pelo WASAPI?
fn is_driver_endpoint(d: &DeviceInfo) -> bool {
    d.node.is_none() && d.name.ends_with(SUFFIX)
}

struct Shared {
    speaker_id: Uuid,
    mic_id: Uuid,
    /// Handle aberto enquanto algum stream virtual existe (o driver aceita um
    /// só por vez no sistema).
    driver: Mutex<Weak<Driver>>,
}

impl Shared {
    fn driver(&self) -> Result<Arc<Driver>, OpenError> {
        let mut weak = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = weak.upgrade() {
            return Ok(d);
        }
        let d = Arc::new(Driver::open()?);
        *weak = Arc::downgrade(&d);
        Ok(d)
    }

}

/// Dispositivos locais + os virtuais do driver. Barato de clonar.
#[derive(Clone)]
pub struct VirtualBackend<B> {
    local: B,
    shared: Arc<Shared>,
}

impl<B> VirtualBackend<B> {
    pub fn new(local: B, ids: &IdGenerator) -> Self {
        Self {
            local,
            shared: Arc::new(Shared {
                speaker_id: ids.device_id(Role::Input, b"audioroute:virtual-speaker"),
                mic_id: ids.device_id(Role::Output, b"audioroute:virtual-mic"),
                driver: Mutex::new(Weak::new()),
            }),
        }
    }

    pub fn local(&self) -> &B {
        &self.local
    }

    /// Entrada "Alto-falantes (audioroute)".
    pub fn speaker_id(&self) -> Uuid {
        self.shared.speaker_id
    }

    /// Saída "Microfone (audioroute)".
    pub fn mic_id(&self) -> Uuid {
        self.shared.mic_id
    }
}

impl<B: DeviceLister> VirtualBackend<B> {
    /// O alto-falante virtual como o Windows o vê (para torná-lo o padrão:
    /// o som de todos os programas entra no audioroute).
    pub fn windows_speaker(&self) -> Option<Uuid> {
        self.local.list().ok()?.into_iter().find(|d| is_driver_endpoint(d) && d.kind == DeviceKind::Output && !d.loopback).map(|d| d.id)
    }
}

impl<B: DeviceLister> DeviceLister for VirtualBackend<B> {
    type Error = B::Error;

    fn list(&self) -> Result<Vec<DeviceInfo>, B::Error> {
        let all = self.local.list()?;
        // O driver aparece no WASAPI quando está instalado. O handle só é
        // aberto quando um cabo usa um virtual: abrir à toa faria o microfone
        // virtual trocar de fonte (cabo interno → app) por um instante.
        if !all.iter().any(is_driver_endpoint) {
            return Ok(all); // sem driver: tudo como antes
        }
        // Nomes como o Windows mostra (no idioma dele).
        let speaker = all.iter().find(|d| is_driver_endpoint(d) && d.kind == DeviceKind::Output && !d.loopback);
        let mic = all.iter().find(|d| is_driver_endpoint(d) && d.kind == DeviceKind::Input && !d.loopback);
        let virtual_speaker = DeviceInfo {
            id: self.shared.speaker_id,
            name: speaker.map_or_else(|| format!("Alto-falantes {SUFFIX}"), |d| d.name.clone()),
            kind: DeviceKind::Input,
            loopback: false,
            // "Padrão" aqui quer dizer: o Windows toca nele (cabo do Windows).
            is_default: speaker.is_some_and(|d| d.is_default),
            node: None,
        };
        let virtual_mic = DeviceInfo {
            id: self.shared.mic_id,
            name: mic.map_or_else(|| format!("Microfone {SUFFIX}"), |d| d.name.clone()),
            kind: DeviceKind::Output,
            loopback: false,
            is_default: false,
            node: None,
        };
        let mut out: Vec<DeviceInfo> = all.into_iter().filter(|d| !is_driver_endpoint(d)).collect();
        out.push(virtual_speaker);
        out.push(virtual_mic);
        Ok(out)
    }
}

impl<B: StreamOpener> StreamOpener for VirtualBackend<B> {
    fn open_capture(&self, id: Uuid, request: StreamRequest, callback: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        if id != self.shared.speaker_id {
            return self.local.open_capture(id, request, callback);
        }
        let driver = self.shared.driver()?;
        Ok(Box::new(VirtualStream::start(driver, Direction::Capture(callback))?))
    }

    fn open_render(&self, id: Uuid, request: StreamRequest, callback: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        if id != self.shared.mic_id {
            return self.local.open_render(id, request, callback);
        }
        let driver = self.shared.driver()?;
        Ok(Box::new(VirtualStream::start(driver, Direction::Render(callback))?))
    }
}

enum Direction {
    /// Alto-falante virtual → motor.
    Capture(CaptureCallback),
    /// Motor → microfone virtual.
    Render(RenderCallback),
}

struct VirtualStream {
    info: StreamInfo,
    alive: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl VirtualStream {
    fn start(driver: Arc<Driver>, mut direction: Direction) -> std::io::Result<Self> {
        let alive = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, running) = (stop.clone(), alive.clone());
        let name = if matches!(direction, Direction::Capture(_)) { "virtual-speaker" } else { "virtual-mic" };
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            let mut raw = vec![0i32; BLOCK * CHANNELS];
            let mut samples = vec![0f32; BLOCK * CHANNELS];
            let start = Instant::now();
            let mut done: u64 = 0;
            while !flag.load(Ordering::SeqCst) {
                let due = (start.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64;
                // Muito atrasado (computador travou): pula em vez de despejar uma rajada.
                if due > done + SAMPLE_RATE as u64 / 5 {
                    done = due - BLOCK as u64;
                }
                while done + BLOCK as u64 <= due {
                    let result = match &mut direction {
                        Direction::Capture(cb) => driver.read_speaker(&mut raw).map(|()| {
                            for (s, r) in samples.iter_mut().zip(&raw) {
                                *s = *r as f32 / 2_147_483_648.0;
                            }
                            cb(&samples);
                        }),
                        Direction::Render(cb) => {
                            cb(&mut samples);
                            for (r, s) in raw.iter_mut().zip(&samples) {
                                *r = (s.clamp(-1.0, 1.0) * 2_147_483_647.0) as i32;
                            }
                            driver.write_mic(&raw)
                        }
                    };
                    if result.is_err() {
                        running.store(false, Ordering::SeqCst); // driver removido: o motor fecha e tenta de novo
                        return;
                    }
                    done += BLOCK as u64;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })?;
        Ok(Self {
            info: StreamInfo {
                mode: ShareMode::Shared,
                period_frames: BLOCK as u32,
                max_callback_frames: BLOCK as u32,
                device_latency_frames: DRIVER_LATENCY_FRAMES,
                native_format: "driver audioroute · 48000 Hz, 2 ch, i32".into(),
            },
            alive,
            stop,
            thread: Some(thread),
        })
    }
}

impl Stream for VirtualStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for VirtualStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
