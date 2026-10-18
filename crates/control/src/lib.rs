//! Fachada do audioroute: tudo o que uma interface pode pedir (listar
//! dispositivos, ligar e desligar cabos, ajustar ganho, modo, período...).
//!
//! Hoje quem chama é o app Tauri; na etapa de rede, o mesmo serviço será
//! exposto por HTTP/WebSocket. Toda alteração vai para o motor e é gravada no
//! config.json na hora.

mod types;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use audioroute_core::audio::SAMPLE_RATE;
use audioroute_core::config::{self, Config, ConfigError, MAX_DELAY_MS, MAX_GAIN, MAX_PERIOD_MS, MIN_PERIOD_MS};
use audioroute_core::{DeviceInfo, DeviceKind, DeviceLister, Engine, ShareMode, StreamOpener, StreamRequest};
use uuid::Uuid;

pub use types::{Device, Route, RouteStats, StreamView};

/// Erro mostrado na interface: só a mensagem.
#[derive(Debug, Clone)]
pub struct ControlError(pub String);

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ControlError {}

macro_rules! from_error {
    ($($t:ty),*) => {$(
        impl From<$t> for ControlError {
            fn from(e: $t) -> Self { ControlError(e.to_string()) }
        }
    )*};
}
from_error!(ConfigError, audioroute_core::engine::EngineError, uuid::Error);

pub type Result<T> = std::result::Result<T, ControlError>;

/// O que o serviço precisa do backend de áudio.
pub trait Backend: DeviceLister + StreamOpener + Clone + 'static {}
impl<T: DeviceLister + StreamOpener + Clone + 'static> Backend for T {}

pub struct Service<B: Backend> {
    backend: B,
    engine: Engine,
    cfg: Config,
    path: PathBuf,
    /// Dispositivos presentes agora.
    devices: HashMap<Uuid, DeviceInfo>,
    /// Cabos ligados no motor.
    active: HashSet<(Uuid, Uuid)>,
    /// Por que um cabo do config não pôde ser ligado.
    errors: HashMap<(Uuid, Uuid), String>,
}

impl<B: Backend> Service<B> {
    /// Carrega o config (ou começa vazio), lista os dispositivos e liga os
    /// cabos possíveis.
    pub fn open(backend: B, path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let cfg = if path.exists() { Config::load(&path)? } else { Config::default() };
        let mut engine = Engine::new(Box::new(backend.clone()), cfg.buffer_ms);
        for d in &cfg.devices {
            // Só registra o pedido: nada está aberto ainda.
            let _ = engine.set_device_request(d.id, request_of(d));
            let _ = engine.set_device_delay(d.id, d.delay_ms);
        }
        let mut s = Self {
            backend,
            engine,
            cfg,
            path,
            devices: HashMap::new(),
            active: HashSet::new(),
            errors: HashMap::new(),
        };
        s.refresh()?;
        Ok(s)
    }

    /// Relê os dispositivos, religa os que o Windows recriou com outro GUID e
    /// tenta ligar os cabos que estavam esperando.
    pub fn refresh(&mut self) -> Result<()> {
        let list = self.backend.list().map_err(|e| ControlError(e.to_string()))?;
        if !self.cfg.rebind_missing(&list).is_empty() {
            self.save()?;
        }
        self.devices = list.into_iter().map(|d| (d.id, d)).collect();
        self.tick();
        Ok(())
    }

    /// Chamado periodicamente: tira os cabos de dispositivos que caíram e
    /// tenta ligar os que estão esperando. Retorna se algo mudou.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        for id in self.engine.failed_devices() {
            for r in self.engine.detach_device(id) {
                self.active.remove(&(r.from, r.to));
                self.errors.insert((r.from, r.to), "dispositivo parou; tentando reabrir".into());
            }
            changed = true;
        }
        let waiting: Vec<_> = self
            .cfg
            .routes
            .iter()
            .filter(|r| !self.active.contains(&(r.from, r.to)) && self.can_route(r.from, r.to))
            .cloned()
            .collect();
        for r in waiting {
            match self.engine.connect(r.from, r.to, r.gain as f32, r.mute) {
                Ok(()) => {
                    self.active.insert((r.from, r.to));
                    self.errors.remove(&(r.from, r.to));
                    changed = true;
                }
                Err(e) => {
                    self.errors.insert((r.from, r.to), e.to_string());
                }
            }
        }
        changed
    }

    fn can_route(&self, from: Uuid, to: Uuid) -> bool {
        matches!(
            (self.devices.get(&from), self.devices.get(&to)),
            (Some(i), Some(o)) if i.kind == DeviceKind::Input && o.kind == DeviceKind::Output
        )
    }

    // ---------------------------------------------------------- consultas

    /// Dispositivos presentes e também os ausentes que o config menciona.
    pub fn devices(&self) -> Vec<Device> {
        let settings: HashMap<Uuid, &config::Device> = self.cfg.devices.iter().map(|d| (d.id, d)).collect();
        let mut present: Vec<&DeviceInfo> = self.devices.values().collect();
        // Este computador primeiro, depois os da rede (agrupados por computador).
        present.sort_by_key(|d| (d.node.clone(), d.loopback, d.kind == DeviceKind::Output, d.name.clone()));

        let mut out: Vec<Device> = present
            .into_iter()
            .map(|info| {
                let set = settings.get(&info.id);
                Device {
                    id: info.id.to_string(),
                    name: info.name.clone(),
                    kind: info.kind.to_string(),
                    loopback: info.loopback,
                    is_default: info.is_default,
                    available: true,
                    node: info.node.clone(),
                    description: set.map(|s| s.description.clone()).unwrap_or_default(),
                    delay_ms: set.map_or(0, |s| s.delay_ms),
                    mode: set.map_or(ShareMode::Shared, |s| s.mode).to_string(),
                    period_ms: set.and_then(|s| s.period_ms),
                    stream: self.engine.device_info(info.id).map(|i| StreamView {
                        mode: i.mode.to_string(),
                        period_ms: i.period_ms(),
                        format: i.native_format.clone(),
                    }),
                }
            })
            .collect();

        for set in &self.cfg.devices {
            if !self.devices.contains_key(&set.id) {
                out.push(Device {
                    id: set.id.to_string(),
                    name: set.name.clone(),
                    kind: self.kind_from_routes(set.id),
                    loopback: false,
                    is_default: false,
                    available: false,
                    node: set.node.clone(),
                    description: set.description.clone(),
                    delay_ms: set.delay_ms,
                    mode: set.mode.to_string(),
                    period_ms: set.period_ms,
                    stream: None,
                });
            }
        }
        out
    }

    fn kind_from_routes(&self, id: Uuid) -> String {
        for r in &self.cfg.routes {
            if r.from == id {
                return DeviceKind::Input.to_string();
            }
            if r.to == id {
                return DeviceKind::Output.to_string();
            }
        }
        "unknown".into()
    }

    pub fn routes(&self) -> Vec<Route> {
        self.cfg
            .routes
            .iter()
            .map(|r| {
                let key = (r.from, r.to);
                let active = self.active.contains(&key);
                Route {
                    from: r.from.to_string(),
                    to: r.to.to_string(),
                    gain: r.gain,
                    mute: r.mute,
                    active,
                    error: if active { None } else { self.errors.get(&key).cloned() },
                }
            })
            .collect()
    }

    /// Pico (0–1) de cada dispositivo aberto pelo motor desde a última leitura.
    pub fn levels(&self) -> HashMap<Uuid, f32> {
        self.engine.levels()
    }

    pub fn stats(&mut self) -> Vec<RouteStats> {
        let (_, routes) = self.engine.stats();
        routes
            .into_iter()
            .map(|r| RouteStats {
                from: r.from.to_string(),
                to: r.to.to_string(),
                level_ms: r.level.as_secs_f64() * 1000.0,
                target_ms: r.target.as_secs_f64() * 1000.0,
                latency_ms: self.engine.estimated_latency(r.from, r.to).unwrap_or_default().as_secs_f64() * 1000.0,
                drift_ppm: r.adjust_ppm,
                underruns: r.underruns,
                overruns: r.overruns,
                resyncs: r.resyncs,
            })
            .collect()
    }

    // ---------------------------------------------------------- alterações

    pub fn connect(&mut self, from: &str, to: &str) -> Result<Route> {
        let (from, to) = (parse(from)?, parse(to)?);
        if self.cfg.routes.iter().any(|r| r.from == from && r.to == to) {
            return Err(ControlError("o cabo já existe".into()));
        }
        if !self.can_route(from, to) {
            return Err(ControlError("o cabo precisa ir de uma entrada presente para uma saída presente".into()));
        }
        self.engine.connect(from, to, config::DEFAULT_GAIN as f32, false)?;
        self.active.insert((from, to));
        self.cfg.routes.push(config::Route { from, to, gain: config::DEFAULT_GAIN, mute: false });
        self.remember(from);
        self.remember(to);
        self.save()?;
        Ok(self.routes().into_iter().find(|r| r.from == from.to_string() && r.to == to.to_string()).unwrap())
    }

    pub fn disconnect(&mut self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (parse(from)?, parse(to)?);
        let i = self.route_index(from, to)?;
        if self.active.remove(&(from, to)) {
            self.engine.disconnect(from, to)?;
        }
        self.errors.remove(&(from, to));
        self.cfg.routes.remove(i);
        self.save()
    }

    pub fn set_gain(&mut self, from: &str, to: &str, gain: f64) -> Result<()> {
        if !(0.0..=MAX_GAIN).contains(&gain) {
            return Err(ControlError(format!("gain {gain} fora do intervalo 0–{MAX_GAIN}")));
        }
        let (from, to) = (parse(from)?, parse(to)?);
        let i = self.route_index(from, to)?;
        if self.active.contains(&(from, to)) {
            self.engine.set_gain(from, to, gain as f32)?;
        }
        self.cfg.routes[i].gain = gain;
        self.save()
    }

    pub fn set_mute(&mut self, from: &str, to: &str, mute: bool) -> Result<()> {
        let (from, to) = (parse(from)?, parse(to)?);
        let i = self.route_index(from, to)?;
        if self.active.contains(&(from, to)) {
            self.engine.set_mute(from, to, mute)?;
        }
        self.cfg.routes[i].mute = mute;
        self.save()
    }

    pub fn set_delay(&mut self, id: &str, delay_ms: u32) -> Result<()> {
        if delay_ms > MAX_DELAY_MS {
            return Err(ControlError(format!("atraso {delay_ms} ms fora do intervalo 0–{MAX_DELAY_MS}")));
        }
        let id = parse(id)?;
        self.engine.set_device_delay(id, delay_ms)?;
        self.remember(id).delay_ms = delay_ms;
        self.save()
    }

    pub fn set_description(&mut self, id: &str, text: &str) -> Result<()> {
        let id = parse(id)?;
        self.remember(id).description = text.trim().to_string();
        self.save()
    }

    /// Muda o modo. Se o dispositivo estiver aberto, é reaberto; se não for
    /// possível (ex.: outro programa usa em exclusivo), nada muda.
    pub fn set_mode(&mut self, id: &str, mode: &str) -> Result<()> {
        let id = parse(id)?;
        let mode = match mode {
            "shared" => ShareMode::Shared,
            "exclusive" => ShareMode::Exclusive,
            other => return Err(ControlError(format!("modo desconhecido: {other}"))),
        };
        if mode == ShareMode::Exclusive && self.devices.get(&id).is_some_and(|d| d.loopback) {
            return Err(ControlError("loopback só funciona em modo compartilhado".into()));
        }
        let period = self.remember(id).period_frames();
        self.engine.set_device_request(id, StreamRequest { mode, period_frames: period })?;
        self.remember(id).mode = mode;
        self.save()
    }

    /// Período desejado em ms (`None` = padrão do modo).
    pub fn set_period(&mut self, id: &str, period_ms: Option<f64>) -> Result<()> {
        if let Some(ms) = period_ms
            && !(MIN_PERIOD_MS..=MAX_PERIOD_MS).contains(&ms)
        {
            return Err(ControlError(format!("período {ms} ms fora do intervalo {MIN_PERIOD_MS}–{MAX_PERIOD_MS}")));
        }
        let id = parse(id)?;
        let mode = self.remember(id).mode;
        let frames = period_ms.map(|ms| (ms * SAMPLE_RATE as f64 / 1000.0).round() as u32);
        self.engine.set_device_request(id, StreamRequest { mode, period_frames: frames })?;
        self.remember(id).period_ms = period_ms;
        self.save()
    }

    // ------------------------------------------------------------- interno

    /// Entrada do dispositivo em cfg.devices, criada se não existir.
    fn remember(&mut self, id: Uuid) -> &mut config::Device {
        let present = self.devices.get(&id);
        let name = present.map(|d| d.name.clone());
        let node = present.map(|d| d.node.clone());
        let i = match self.cfg.devices.iter().position(|d| d.id == id) {
            Some(i) => i,
            None => {
                self.cfg.devices.push(config::Device::new(id, name.clone().unwrap_or_default()));
                self.cfg.devices.len() - 1
            }
        };
        let d = &mut self.cfg.devices[i];
        if let Some(name) = name {
            d.name = name;
        }
        if let Some(node) = node {
            d.node = node;
        }
        d
    }

    fn route_index(&self, from: Uuid, to: Uuid) -> Result<usize> {
        self.cfg
            .routes
            .iter()
            .position(|r| r.from == from && r.to == to)
            .ok_or_else(|| ControlError("o cabo não existe".into()))
    }

    fn save(&self) -> Result<()> {
        Ok(self.cfg.save(&self.path)?)
    }
}

/// Visão simples de um dispositivo, sem config (papel de participante).
pub fn device_view(info: &DeviceInfo) -> Device {
    Device {
        id: info.id.to_string(),
        name: info.name.clone(),
        kind: info.kind.to_string(),
        loopback: info.loopback,
        is_default: info.is_default,
        available: true,
        node: info.node.clone(),
        description: String::new(),
        delay_ms: 0,
        mode: ShareMode::Shared.to_string(),
        period_ms: None,
        stream: None,
    }
}

fn request_of(d: &config::Device) -> StreamRequest {
    StreamRequest { mode: d.mode, period_frames: d.period_frames() }
}

fn parse(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).map_err(|e| ControlError(format!("id inválido: {e}")))
}
