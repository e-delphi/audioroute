//! Motor de roteamento: liga entradas a saídas e faz a mixagem com ganho,
//! mute, atraso e compensação de drift.
//!
//! O `Engine` vive na thread de controle. Cada dispositivo aberto tem um
//! [`device::Processor`] rodando no callback da sua thread de áudio; os dois
//! conversam só por filas lock-free (ver `device.rs`).

mod device;
mod drift;
pub mod resample;
mod route;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use rtrb::RingBuffer;
use uuid::Uuid;

use crate::audio::{
    CHANNELS, SAMPLE_RATE, ShareMode, Stream, StreamError, StreamInfo, StreamOpener, StreamRequest,
};
use crate::device::DeviceKind;
use device::{Command, Control, DeviceShared, MAX_ROUTES, Processor};
use route::{CaptureEnd, RenderEnd, RouteShared};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("cabo já existe")]
    RouteExists,
    #[error("cabo não existe")]
    NoRoute,
    #[error("o dispositivo {0} já está aberto como {1}")]
    WrongKind(Uuid, DeviceKind),
    #[error("limite de {MAX_ROUTES} cabos por dispositivo")]
    TooManyRoutes,
    #[error("abrindo {kind} {id}: {source}")]
    Device { id: Uuid, kind: DeviceKind, source: StreamError },
}

/// Margem de segurança somada ao mínimo de buffer, para o jitter dos callbacks.
const JITTER_MARGIN: Duration = Duration::from_millis(1);
/// Margem dos cabos entre dois dispositivos de relógio do sistema: são
/// threads comuns, sujeitas aos atrasos do agendador do Windows.
const STEADY_MARGIN: Duration = Duration::from_millis(5);

pub struct Engine {
    opener: Box<dyn StreamOpener>,
    buffer_ms: u32,
    epoch: Instant,
    next_id: u64,
    devices: HashMap<Uuid, OpenDevice>,
    routes: HashMap<(Uuid, Uuid), Arc<RouteShared>>,
    delays: HashMap<Uuid, u32>,
    requests: HashMap<Uuid, StreamRequest>,
}

/// Um cabo com seus parâmetros, para recriá-lo depois.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteSpec {
    pub from: Uuid,
    pub to: Uuid,
    pub gain: f32,
    pub mute: bool,
}

struct OpenDevice {
    kind: DeviceKind,
    info: StreamInfo,
    shared: Arc<DeviceShared>,
    control: DeviceControl,
    routes: usize,
    // Por último: ao soltar, a thread de áudio para antes do resto sumir.
    _stream: Box<dyn Stream>,
}

enum DeviceControl {
    Capture(Control<CaptureEnd>),
    Render(Control<RenderEnd>),
}

impl Engine {
    pub fn new(opener: Box<dyn StreamOpener>, buffer_ms: u32) -> Self {
        Self {
            opener,
            buffer_ms,
            epoch: Instant::now(),
            next_id: 1,
            devices: HashMap::new(),
            routes: HashMap::new(),
            delays: HashMap::new(),
            requests: HashMap::new(),
        }
    }

    /// Liga um cabo de `from` (entrada) para `to` (saída), abrindo os
    /// dispositivos se preciso.
    pub fn connect(&mut self, from: Uuid, to: Uuid, gain: f32, mute: bool) -> Result<(), EngineError> {
        self.collect_garbage();
        if self.routes.contains_key(&(from, to)) {
            return Err(EngineError::RouteExists);
        }
        self.ensure_device(from, DeviceKind::Input)?;
        if let Err(e) = self.ensure_device(to, DeviceKind::Output) {
            self.release_if_unused(from);
            return Err(e);
        }
        if self.devices[&from].routes >= MAX_ROUTES || self.devices[&to].routes >= MAX_ROUTES {
            self.release_if_unused(from);
            self.release_if_unused(to);
            return Err(EngineError::TooManyRoutes);
        }

        let in_period = self.devices[&from].info.period_frames;
        let out = &self.devices[&to].info;
        let (out_period, out_max) = (out.period_frames, out.max_callback_frames);
        let target = self.target_frames(from, to);
        let capacity = (target + 4 * in_period.max(out_period) + out_max) as usize * CHANNELS;

        let id = self.next_id;
        self.next_id += 1;
        let shared = Arc::new(RouteShared::new(id, from, to, gain, mute, target));
        let (tx, rx) = RingBuffer::new(capacity);
        let render_end = RenderEnd::new(shared.clone(), rx, in_period, out_period, out_max);
        let capture_end = CaptureEnd::new(shared.clone(), tx);

        // A saída primeiro: quando a entrada começar a escrever, já há quem leia.
        self.send_render(to, Command::Add(Box::new(render_end)));
        self.send_capture(from, Command::Add(Box::new(capture_end)));
        self.routes.insert((from, to), shared);
        self.devices.get_mut(&from).unwrap().routes += 1;
        self.devices.get_mut(&to).unwrap().routes += 1;
        Ok(())
    }

    /// Remove o cabo e fecha os dispositivos que ficaram sem cabos.
    pub fn disconnect(&mut self, from: Uuid, to: Uuid) -> Result<(), EngineError> {
        self.collect_garbage();
        let shared = self.routes.remove(&(from, to)).ok_or(EngineError::NoRoute)?;
        self.send_capture(from, Command::Remove(shared.id));
        self.send_render(to, Command::Remove(shared.id));
        for id in [from, to] {
            self.devices.get_mut(&id).unwrap().routes -= 1;
            self.release_if_unused(id);
        }
        Ok(())
    }

    pub fn set_gain(&mut self, from: Uuid, to: Uuid, gain: f32) -> Result<(), EngineError> {
        let r = self.routes.get(&(from, to)).ok_or(EngineError::NoRoute)?;
        r.gain_bits.store(gain.to_bits(), Relaxed);
        Ok(())
    }

    pub fn set_mute(&mut self, from: Uuid, to: Uuid, mute: bool) -> Result<(), EngineError> {
        let r = self.routes.get(&(from, to)).ok_or(EngineError::NoRoute)?;
        r.mute.store(mute, Relaxed);
        Ok(())
    }

    /// Atraso de sincronização de um dispositivo; vale para todos os cabos
    /// dele, inclusive os criados depois.
    pub fn set_device_delay(&mut self, id: Uuid, delay_ms: u32) -> Result<(), EngineError> {
        self.delays.insert(id, delay_ms);
        // Recria os cabos afetados: o ring de cada um é dimensionado pelo alvo.
        self.reconnect_routes_of(id)
    }

    /// Modo (shared/exclusive) de um dispositivo.
    pub fn set_device_mode(&mut self, id: Uuid, mode: ShareMode) -> Result<(), EngineError> {
        let request = StreamRequest { mode, ..self.request(id) };
        self.set_device_request(id, request)
    }

    /// Período desejado (frames) de um dispositivo; `None` = padrão do modo.
    pub fn set_device_period(&mut self, id: Uuid, period_frames: Option<u32>) -> Result<(), EngineError> {
        let request = StreamRequest { period_frames, ..self.request(id) };
        self.set_device_request(id, request)
    }

    /// Como abrir um dispositivo. Se estiver aberto, é reaberto; se falhar,
    /// volta ao pedido anterior.
    pub fn set_device_request(&mut self, id: Uuid, request: StreamRequest) -> Result<(), EngineError> {
        let previous = self.requests.insert(id, request).unwrap_or_default();
        if previous == request {
            return Ok(());
        }
        if let Err(e) = self.reconnect_routes_of(id) {
            self.requests.insert(id, previous);
            self.reconnect_routes_of(id)?;
            return Err(e);
        }
        Ok(())
    }

    fn request(&self, id: Uuid) -> StreamRequest {
        self.requests.get(&id).copied().unwrap_or_default()
    }

    /// Dispositivos cujo stream parou sozinho (desconectado, driver caiu).
    pub fn failed_devices(&self) -> Vec<Uuid> {
        self.devices.iter().filter(|(_, d)| !d._stream.is_alive()).map(|(id, _)| *id).collect()
    }

    /// Remove todos os cabos de um dispositivo (fechando-o) e devolve os
    /// cabos removidos, para quem chamou tentar religar mais tarde.
    pub fn detach_device(&mut self, id: Uuid) -> Vec<RouteSpec> {
        let affected: Vec<RouteSpec> = self
            .routes
            .values()
            .filter(|r| r.from == id || r.to == id)
            .map(|r| RouteSpec { from: r.from, to: r.to, gain: r.gain(), mute: r.mute.load(Relaxed) })
            .collect();
        for r in &affected {
            let _ = self.disconnect(r.from, r.to);
        }
        affected
    }

    /// Informações do stream de um dispositivo aberto.
    pub fn device_info(&self, id: Uuid) -> Option<&StreamInfo> {
        self.devices.get(&id).map(|d| &d.info)
    }

    /// Pico (0–1) de cada dispositivo aberto desde a última leitura.
    pub fn levels(&self) -> HashMap<Uuid, f32> {
        self.devices
            .iter()
            .map(|(id, d)| (*id, f32::from_bits(d.shared.peak_bits.swap(0, Relaxed))))
            .collect()
    }

    /// Estatísticas. As de dispositivo zeram a cada leitura (janela).
    pub fn stats(&mut self) -> (Vec<DeviceStats>, Vec<RouteStats>) {
        self.collect_garbage();
        let devices = self
            .devices
            .iter()
            .map(|(id, d)| {
                let s = &d.shared;
                let calls = s.calls.swap(0, Relaxed);
                let sum = s.busy_sum_ns.swap(0, Relaxed);
                DeviceStats {
                    id: *id,
                    kind: d.kind,
                    info: d.info.clone(),
                    calls,
                    busy_avg: Duration::from_nanos(if calls > 0 { sum / calls } else { 0 }),
                    busy_max: Duration::from_nanos(s.busy_max_ns.swap(0, Relaxed)),
                    gap_min: Duration::from_nanos(s.gap_min_ns.swap(0, Relaxed)),
                    gap_max: Duration::from_nanos(s.gap_max_ns.swap(0, Relaxed)),
                }
            })
            .collect();
        let routes = self
            .routes
            .values()
            .map(|r| RouteStats {
                from: r.from,
                to: r.to,
                level: frames_to_duration(r.level.load(Relaxed)),
                target: frames_to_duration(r.target.load(Relaxed)),
                adjust_ppm: f64::from_bits(r.adjust_bits.load(Relaxed)) * 1e6,
                underruns: r.underruns.load(Relaxed),
                overruns: r.overruns.load(Relaxed),
                resyncs: r.resyncs.load(Relaxed),
            })
            .collect();
        (devices, routes)
    }

    /// Latência estimada de um cabo: buffer da entrada + nível alvo do ring
    /// + buffer da saída (sem contar conversores do próprio hardware).
    pub fn estimated_latency(&self, from: Uuid, to: Uuid) -> Option<Duration> {
        let r = self.routes.get(&(from, to))?;
        let input = self.devices.get(&from)?.info.device_latency_frames;
        let output = self.devices.get(&to)?.info.device_latency_frames;
        Some(frames_to_duration(input + r.target.load(Relaxed) + output))
    }

    // ------------------------------------------------------------ interno

    /// Nível alvo do ring: o maior entre o bufferMs pedido e o mínimo seguro
    /// (um período de cada lado + margem de jitter), mais os atrasos.
    fn target_frames(&self, from: Uuid, to: Uuid) -> u32 {
        let in_period = self.devices.get(&from).map_or(0, |d| d.info.period_frames);
        let out_period = self.devices.get(&to).map_or(0, |d| d.info.period_frames);
        // As duas pontas no relógio do sistema: só o mínimo (com a margem das
        // threads) basta. Com um dispositivo de hardware no meio, vale o piso
        // pedido (bufferMs).
        let steady = |id: Uuid| self.devices.get(&id).is_some_and(|d| d.info.steady_clock);
        let both_steady = steady(from) && steady(to);
        let margin = if both_steady { STEADY_MARGIN } else { JITTER_MARGIN };
        let minimum = in_period + out_period + (margin.as_secs_f64() * SAMPLE_RATE as f64) as u32;
        let requested = self.buffer_ms * SAMPLE_RATE / 1000;
        let delays = self.delays.get(&from).unwrap_or(&0) + self.delays.get(&to).unwrap_or(&0);
        let base = if both_steady { minimum } else { requested.max(minimum) };
        base + delays * SAMPLE_RATE / 1000
    }

    fn ensure_device(&mut self, id: Uuid, kind: DeviceKind) -> Result<(), EngineError> {
        if let Some(d) = self.devices.get(&id) {
            return if d.kind == kind { Ok(()) } else { Err(EngineError::WrongKind(id, d.kind)) };
        }
        let request = self.request(id);
        let shared = Arc::new(DeviceShared::default());
        let (control, stream) = match kind {
            DeviceKind::Input => {
                let (control, mut processor): (_, Processor<CaptureEnd>) =
                    device::channel(shared.clone(), self.epoch);
                let stream = self.opener.open_capture(
                    id,
                    request,
                    Box::new(move |samples: &[f32]| processor.capture(samples)),
                );
                (DeviceControl::Capture(control), stream)
            }
            DeviceKind::Output => {
                let (control, mut processor): (_, Processor<RenderEnd>) =
                    device::channel(shared.clone(), self.epoch);
                let stream = self.opener.open_render(
                    id,
                    request,
                    Box::new(move |out: &mut [f32]| processor.render(out)),
                );
                (DeviceControl::Render(control), stream)
            }
        };
        let stream = stream.map_err(|source| EngineError::Device { id, kind, source })?;
        let info = stream.info().clone();
        self.devices.insert(id, OpenDevice { kind, info, shared, control, routes: 0, _stream: stream });
        Ok(())
    }

    fn release_if_unused(&mut self, id: Uuid) {
        if self.devices.get(&id).is_some_and(|d| d.routes == 0) {
            // Soltar o stream para a thread de áudio e espera ela terminar.
            self.devices.remove(&id);
        }
    }

    /// Recria todos os cabos que tocam em `id` (com o mesmo ganho e mute).
    fn reconnect_routes_of(&mut self, id: Uuid) -> Result<(), EngineError> {
        let affected = self.detach_device(id);
        let mut first_error = None;
        for r in affected {
            if let Err(e) = self.connect(r.from, r.to, r.gain, r.mute) {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn send_capture(&mut self, id: Uuid, command: Command<CaptureEnd>) {
        if let Some(OpenDevice { control: DeviceControl::Capture(c), .. }) = self.devices.get_mut(&id) {
            // A fila tem o dobro do máximo de cabos: só enche se a thread de
            // áudio estiver parada, e aí o comando não faria diferença.
            let _ = c.commands.push(command);
        }
    }

    fn send_render(&mut self, id: Uuid, command: Command<RenderEnd>) {
        if let Some(OpenDevice { control: DeviceControl::Render(c), .. }) = self.devices.get_mut(&id) {
            let _ = c.commands.push(command);
        }
    }

    fn collect_garbage(&mut self) {
        for d in self.devices.values_mut() {
            match &mut d.control {
                DeviceControl::Capture(c) => c.collect_garbage(),
                DeviceControl::Render(c) => c.collect_garbage(),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceStats {
    pub id: Uuid,
    pub kind: DeviceKind,
    pub info: StreamInfo,
    pub calls: u64,
    pub busy_avg: Duration,
    pub busy_max: Duration,
    pub gap_min: Duration,
    pub gap_max: Duration,
}

#[derive(Debug, Clone)]
pub struct RouteStats {
    pub from: Uuid,
    pub to: Uuid,
    pub level: Duration,
    pub target: Duration,
    pub adjust_ppm: f64,
    pub underruns: u64,
    pub overruns: u64,
    pub resyncs: u64,
}

pub fn frames_to_duration(frames: u32) -> Duration {
    Duration::from_secs_f64(frames as f64 / SAMPLE_RATE as f64)
}

#[cfg(test)]
mod tests;
