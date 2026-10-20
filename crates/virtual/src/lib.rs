//! Pares de alto-falante e microfone virtuais do audioroute, lidos e escritos
//! direto no driver (`driver/`), sem passar pelo mixer do Windows.
//!
//! O driver tem [`driver::MAX_PAIRS`] pares fixos; o app escolhe quais usar e
//! dá nome a eles (guardado em `virtual.json`). No canvas, cada par usado
//! vira:
//! * uma **entrada** (o alto-falante): o que os programas tocam nele entra no
//!   audioroute;
//! * uma **saída** (o microfone): o que o audioroute manda para ele sai como
//!   microfone no Discord, OBS etc.
//!
//! No Windows, os pares em uso ganham o nome ("Discord (audioroute)") e os
//! livres ficam escondidos. As versões dos dispositivos do driver vistas pelo
//! WASAPI somem do canvas (seriam duplicadas com a direção trocada). Sem o
//! driver instalado, nada muda.
//!
//! Cada stream tem uma thread que anda pelo relógio do sistema, 2 ms por
//! vez — o mesmo relógio do driver, então não há drift entre os dois.

pub mod driver;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audioroute_core::audio::{CaptureCallback, RenderCallback, SAMPLE_RATE, StreamError};
use audioroute_core::{
    DeviceInfo, DeviceKind, DeviceLister, IdGenerator, Role, ShareMode, Stream, StreamInfo, StreamOpener, StreamRequest,
};
use audioroute_wasapi::{FilterEndpoint, WasapiBackend};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use driver::{Driver, OpenError};

/// Os dispositivos do driver terminam com isto no nome do Windows.
const SUFFIX: &str = "(audioroute)";
/// Frames por bloco (2 ms).
const BLOCK: usize = 96;
const CHANNELS: usize = 2;
/// Folga que o driver guarda antes de entregar som (4 ms).
const DRIVER_LATENCY_FRAMES: u32 = 192;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// É um dispositivo do driver visto pelo WASAPI?
fn is_driver_endpoint(d: &DeviceInfo) -> bool {
    d.node.is_none() && d.name.ends_with(SUFFIX)
}

/// Par em uso, como fica guardado.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VirtualPair {
    /// 0..MAX_PAIRS-1.
    pub pair: u32,
    /// Nome dado pelo usuário (vazio: o nome padrão do Windows).
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    pairs: Vec<VirtualPair>,
}

/// Um par em uso, como a interface vê.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairView {
    pub pair: u32,
    pub name: String,
    /// Id da entrada (alto-falante) e da saída (microfone) no canvas.
    pub speaker: String,
    pub mic: String,
}

struct Shared {
    ids: IdGenerator,
    state_path: Option<PathBuf>,
    pairs: Mutex<Vec<VirtualPair>>,
    /// Handle aberto enquanto algum stream virtual existe (o driver aceita um
    /// só por vez no sistema).
    driver: Mutex<Weak<Driver>>,
}

impl Shared {
    fn driver(&self) -> Result<Arc<Driver>, OpenError> {
        let mut weak = lock(&self.driver);
        if let Some(d) = weak.upgrade() {
            return Ok(d);
        }
        let d = Arc::new(Driver::open()?);
        *weak = Arc::downgrade(&d);
        Ok(d)
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.state_path else { return Ok(()) };
        let state = State { pairs: lock(&self.pairs).clone() };
        let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

/// Dispositivos do WASAPI + os pares virtuais do driver. Barato de clonar.
#[derive(Clone)]
pub struct VirtualBackend {
    local: WasapiBackend,
    shared: Arc<Shared>,
}

impl VirtualBackend {
    /// `state_path`: onde guardar os pares em uso (`None`: só em memória).
    /// Sem arquivo ainda, começa com o par 1 em uso, sem nome.
    pub fn new(local: WasapiBackend, ids: &IdGenerator, state_path: Option<PathBuf>) -> Self {
        let pairs = state_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<State>(&t).ok())
            .map(|s| s.pairs)
            .unwrap_or_else(|| vec![VirtualPair { pair: 0, name: String::new() }]);
        Self {
            local,
            shared: Arc::new(Shared {
                ids: ids.clone(),
                state_path,
                pairs: Mutex::new(pairs),
                driver: Mutex::new(Weak::new()),
            }),
        }
    }

    pub fn local(&self) -> &WasapiBackend {
        &self.local
    }

    /// Id no canvas da entrada (alto-falante) do par. O par 0 mantém o id da
    /// versão de um par só, para os cabos salvos continuarem valendo.
    pub fn speaker_id(&self, pair: u32) -> Uuid {
        let key = if pair == 0 { "audioroute:virtual-speaker".to_owned() } else { format!("audioroute:virtual-speaker:{pair}") };
        self.shared.ids.device_id(Role::Input, key.as_bytes())
    }

    /// Id no canvas da saída (microfone) do par.
    pub fn mic_id(&self, pair: u32) -> Uuid {
        let key = if pair == 0 { "audioroute:virtual-mic".to_owned() } else { format!("audioroute:virtual-mic:{pair}") };
        self.shared.ids.device_id(Role::Output, key.as_bytes())
    }

    /// Par de um id do canvas, e se é o alto-falante (`true`) ou o microfone.
    fn pair_of(&self, id: Uuid) -> Option<(u32, bool)> {
        lock(&self.shared.pairs).iter().find_map(|p| {
            if self.speaker_id(p.pair) == id {
                Some((p.pair, true))
            } else if self.mic_id(p.pair) == id {
                Some((p.pair, false))
            } else {
                None
            }
        })
    }

    /// O driver está instalado (os dispositivos dele aparecem no Windows)?
    pub fn installed(&self) -> bool {
        self.local.list().is_ok_and(|l| l.iter().any(is_driver_endpoint))
    }

    pub fn pairs(&self) -> Vec<PairView> {
        let mut out: Vec<PairView> = lock(&self.shared.pairs)
            .iter()
            .map(|p| PairView {
                pair: p.pair,
                name: p.name.clone(),
                speaker: self.speaker_id(p.pair).to_string(),
                mic: self.mic_id(p.pair).to_string(),
            })
            .collect();
        out.sort_by_key(|p| p.pair);
        out
    }

    /// Usa o próximo par livre. Devolve o número dele.
    pub fn add_pair(&self, name: &str) -> Result<u32, String> {
        let pair = {
            let mut pairs = lock(&self.shared.pairs);
            let free = (0..driver::MAX_PAIRS).find(|n| !pairs.iter().any(|p| p.pair == *n));
            let pair = free.ok_or(format!("o driver tem só {} pares, e todos estão em uso", driver::MAX_PAIRS))?;
            pairs.push(VirtualPair { pair, name: name.trim().to_owned() });
            pair
        };
        self.shared.save()?;
        Ok(pair)
    }

    pub fn rename_pair(&self, pair: u32, name: &str) -> Result<(), String> {
        {
            let mut pairs = lock(&self.shared.pairs);
            let p = pairs.iter_mut().find(|p| p.pair == pair).ok_or("par não está em uso")?;
            p.name = name.trim().to_owned();
        }
        self.shared.save()
    }

    pub fn remove_pair(&self, pair: u32) -> Result<(), String> {
        lock(&self.shared.pairs).retain(|p| p.pair != pair);
        self.shared.save()
    }

    /// Aplica os pares no Windows: os em uso aparecem (com o nome, se houver)
    /// e os livres ficam escondidos. Devolve o que não deu para aplicar.
    pub fn apply_windows(&self) -> Vec<String> {
        let endpoints = match audioroute_wasapi::filter_endpoints() {
            Ok(e) => e,
            Err(e) => return vec![e.to_string()],
        };
        let pairs = lock(&self.shared.pairs).clone();
        let mut errors = Vec::new();
        for n in 0..driver::MAX_PAIRS {
            let used = pairs.iter().find(|p| p.pair == n);
            for (kind, ep) in [("alto-falante", find_filter(&endpoints, true, n)), ("microfone", find_filter(&endpoints, false, n))] {
                let Some(ep) = ep else { continue };
                if ep.active != used.is_some() {
                    if let Err(e) = audioroute_wasapi::set_endpoint_visible(&ep.endpoint_id, used.is_some()) {
                        errors.push(format!("mostrar/esconder o {kind} {}: {e}", n + 1));
                    }
                }
                if let Some(p) = used.filter(|p| !p.name.is_empty()) {
                    let wanted = format!("{} {SUFFIX}", p.name);
                    if ep.name != wanted {
                        if let Err(e) = audioroute_wasapi::set_endpoint_name(&ep.endpoint_id, &p.name) {
                            errors.push(format!("dar nome ao {kind} {}: {e}", n + 1));
                        }
                    }
                }
            }
        }
        errors
    }

    /// O alto-falante do par como o Windows o vê (para torná-lo o padrão: o
    /// som de todos os programas entra no audioroute por ele).
    pub fn windows_speaker(&self, id: Uuid) -> Option<Uuid> {
        let (pair, true) = self.pair_of(id)? else { return None };
        let endpoint = find_filter(&audioroute_wasapi::filter_endpoints().ok()?, true, pair)?;
        self.local.endpoints().ok()?.into_iter().find(|e| e.endpoint_id == endpoint.endpoint_id && !e.info.loopback).map(|e| e.info.id)
    }
}

/// O dispositivo do Windows do par `n` (alto-falante ou microfone).
fn find_filter(endpoints: &[FilterEndpoint], output: bool, n: u32) -> Option<FilterEndpoint> {
    let suffix = if output { format!("\\wavespeaker{}", n + 1) } else { format!("\\wavemic{}", n + 1) };
    endpoints.iter().find(|e| e.output == output && e.filter.to_lowercase().ends_with(&suffix)).cloned()
}

impl DeviceLister for VirtualBackend {
    type Error = audioroute_wasapi::Error;

    fn list(&self) -> Result<Vec<DeviceInfo>, Self::Error> {
        let all = self.local.list()?;
        // O driver aparece no WASAPI quando está instalado. O handle só é
        // aberto quando um cabo usa um virtual: abrir à toa faria os
        // microfones trocarem de fonte (cabo interno → app) por um instante.
        if !all.iter().any(is_driver_endpoint) {
            return Ok(all); // sem driver: tudo como antes
        }
        // "Padrão" na entrada virtual quer dizer: o Windows toca nela.
        let default_speaker = all.iter().find(|d| is_driver_endpoint(d) && d.kind == DeviceKind::Output && d.is_default).cloned();
        let default_pair = default_speaker.and_then(|d| {
            let ep = self.local.endpoints().ok()?.into_iter().find(|e| e.info.id == d.id)?;
            let filters = audioroute_wasapi::filter_endpoints().ok()?;
            (0..driver::MAX_PAIRS).find(|n| find_filter(&filters, true, *n).is_some_and(|f| f.endpoint_id == ep.endpoint_id))
        });

        let mut out: Vec<DeviceInfo> = all.into_iter().filter(|d| !is_driver_endpoint(d)).collect();
        for p in lock(&self.shared.pairs).iter() {
            let number = if p.pair == 0 { String::new() } else { format!(" {}", p.pair + 1) };
            let (speaker, mic) = if p.name.is_empty() {
                (format!("Alto-falantes{number} {SUFFIX}"), format!("Microfone{number} {SUFFIX}"))
            } else {
                (format!("{} (alto-falante)", p.name), format!("{} (microfone)", p.name))
            };
            out.push(DeviceInfo {
                id: self.speaker_id(p.pair),
                name: speaker,
                kind: DeviceKind::Input,
                loopback: false,
                is_default: default_pair == Some(p.pair),
                node: None,
            });
            out.push(DeviceInfo {
                id: self.mic_id(p.pair),
                name: mic,
                kind: DeviceKind::Output,
                loopback: false,
                is_default: false,
                node: None,
            });
        }
        Ok(out)
    }
}

impl StreamOpener for VirtualBackend {
    fn open_capture(&self, id: Uuid, request: StreamRequest, callback: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        match self.pair_of(id) {
            Some((pair, true)) => {
                let driver = self.shared.driver()?;
                Ok(Box::new(VirtualStream::start(driver, pair, Direction::Capture(callback))?))
            }
            _ => self.local.open_capture(id, request, callback),
        }
    }

    fn open_render(&self, id: Uuid, request: StreamRequest, callback: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        match self.pair_of(id) {
            Some((pair, false)) => {
                let driver = self.shared.driver()?;
                Ok(Box::new(VirtualStream::start(driver, pair, Direction::Render(callback))?))
            }
            _ => self.local.open_render(id, request, callback),
        }
    }
}

enum Direction {
    /// Alto-falante do par → motor.
    Capture(CaptureCallback),
    /// Motor → microfone do par.
    Render(RenderCallback),
}

struct VirtualStream {
    info: StreamInfo,
    alive: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl VirtualStream {
    fn start(driver: Arc<Driver>, pair: u32, mut direction: Direction) -> std::io::Result<Self> {
        let alive = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, running) = (stop.clone(), alive.clone());
        let name = if matches!(direction, Direction::Capture(_)) { "virtual-speaker" } else { "virtual-mic" };
        let thread = std::thread::Builder::new().name(format!("{name}-{}", pair + 1)).spawn(move || {
            // Prioridade de áudio (MMCSS), como as threads do WASAPI: sem ela o
            // agendador atrasa os blocos de 2 ms quando o computador está ocupado.
            let mut task = 0u32;
            let mmcss = unsafe {
                windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task)
            }
            .ok();
            let mut raw = vec![0i32; BLOCK * CHANNELS];
            let mut samples = vec![0f32; BLOCK * CHANNELS];
            let mut scratch = Vec::with_capacity(8 + BLOCK * CHANNELS * 4);
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
                        Direction::Capture(cb) => driver.read_speaker(pair, &mut raw).map(|()| {
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
                            driver.write_mic(pair, &raw, &mut scratch)
                        }
                    };
                    if result.is_err() {
                        running.store(false, Ordering::SeqCst); // driver removido: o motor fecha e tenta de novo
                        break;
                    }
                    done += BLOCK as u64;
                }
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if let Some(h) = mmcss {
                unsafe {
                    let _ = windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(h);
                }
            }
        })?;
        Ok(Self {
            info: StreamInfo {
                mode: ShareMode::Shared,
                period_frames: BLOCK as u32,
                max_callback_frames: BLOCK as u32,
                device_latency_frames: DRIVER_LATENCY_FRAMES,
                native_format: format!("driver audioroute, par {} · 48000 Hz, 2 ch, i32", pair + 1),
                // Anda pelo relógio do sistema, como o driver: sem drift nem jitter de hardware.
                steady_clock: true,
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
