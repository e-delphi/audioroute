//! As duas pontas de um cabo: a escrita (thread da entrada) e a leitura com
//! reamostragem e ganho (thread da saída). Entre elas, um ring buffer SPSC.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use rtrb::{Consumer, Producer};
use uuid::Uuid;

use super::drift::{DriftController, MAX_ADJUST};
use super::resample::Resampler;
use crate::audio::{CHANNELS, SAMPLE_RATE};

/// Estado do cabo visível às duas threads de áudio e à de controle.
/// Tudo atômico: ninguém espera por ninguém.
pub(crate) struct RouteShared {
    pub id: u64,
    pub from: Uuid,
    pub to: Uuid,
    pub gain_bits: AtomicU32,
    pub mute: AtomicBool,
    /// Nível desejado do buffer, em frames (= latência do cabo).
    pub target: AtomicU32,
    /// Instante (ns desde o início do motor) da última escrita da entrada.
    pub last_write_ns: AtomicU64,
    /// Nível medido pela saída, para as estatísticas.
    pub level: AtomicU32,
    /// Razão de reamostragem − 1 (bits de f64), para as estatísticas.
    pub adjust_bits: AtomicU64,
    pub underruns: AtomicU64,
    pub overruns: AtomicU64,
    pub resyncs: AtomicU64,
}

impl RouteShared {
    pub fn new(id: u64, from: Uuid, to: Uuid, gain: f32, mute: bool, target: u32) -> Self {
        Self {
            id,
            from,
            to,
            gain_bits: AtomicU32::new(gain.to_bits()),
            mute: AtomicBool::new(mute),
            target: AtomicU32::new(target),
            last_write_ns: AtomicU64::new(0),
            level: AtomicU32::new(0),
            adjust_bits: AtomicU64::new(0f64.to_bits()),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            resyncs: AtomicU64::new(0),
        }
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Relaxed))
    }
}

/// Algo que vive na lista de um dispositivo e é identificado pelo cabo.
pub(crate) trait End {
    fn route_id(&self) -> u64;
}

// ---------------------------------------------------------------- entrada

pub(crate) struct CaptureEnd {
    pub shared: Arc<RouteShared>,
    tx: Producer<f32>,
}

impl CaptureEnd {
    pub fn new(shared: Arc<RouteShared>, tx: Producer<f32>) -> Self {
        Self { shared, tx }
    }

    /// Grava o bloco inteiro ou nada (mantém L/R alinhados).
    pub fn write(&mut self, samples: &[f32], now_ns: u64) {
        match self.tx.write_chunk(samples.len()) {
            Ok(mut chunk) => {
                let (a, b) = chunk.as_mut_slices();
                let split = a.len();
                a.copy_from_slice(&samples[..split]);
                b.copy_from_slice(&samples[split..]);
                chunk.commit_all();
                self.shared.last_write_ns.store(now_ns, Relaxed);
            }
            Err(_) => {
                // A saída não está consumindo: o bloco é descartado.
                self.shared.overruns.fetch_add(1, Relaxed);
            }
        }
    }
}

impl End for CaptureEnd {
    fn route_id(&self) -> u64 {
        self.shared.id
    }
}

// ------------------------------------------------------------------ saída

pub(crate) struct RenderEnd {
    pub shared: Arc<RouteShared>,
    rx: Consumer<f32>,
    /// Período da entrada: limita a estimativa de "áudio já capturado".
    in_period: u32,
    /// Folga acima do alvo antes de um resync.
    slack: u32,

    priming: bool,
    cur_gain: f32,
    scratch: Vec<f32>,
    rs: Resampler,
    drift: DriftController,
}

impl RenderEnd {
    pub fn new(
        shared: Arc<RouteShared>,
        rx: Consumer<f32>,
        in_period: u32,
        out_period: u32,
        max_callback_frames: u32,
    ) -> Self {
        let target = shared.target.load(Relaxed) as f64;
        let max_frames = max_callback_frames as usize;
        Self {
            shared,
            rx,
            in_period,
            slack: 3 * in_period.max(out_period),
            priming: true,
            cur_gain: 0.0,
            scratch: vec![0.0; max_frames * CHANNELS],
            rs: Resampler::new(CHANNELS, max_frames, 1.0 + MAX_ADJUST),
            drift: DriftController::new(target),
        }
    }

    /// Nível em frames: o que está no ring + o que a entrada já capturou
    /// desde a última escrita (proporcional ao tempo). Somar essa parte torna
    /// a medida contínua e independente da fase entre os dois callbacks.
    pub fn level(&self, now_ns: u64) -> f64 {
        let last = self.shared.last_write_ns.load(Relaxed);
        let since = now_ns.saturating_sub(last) as f64 * SAMPLE_RATE as f64 / 1e9;
        let pending = since.clamp(0.0, self.in_period as f64);
        (self.rx.slots() / CHANNELS) as f64 + pending + self.rs.buffered()
    }

    /// Soma o áudio deste cabo em `out`. Sem alocação e sem bloqueio.
    pub fn mix_into(&mut self, out: &mut [f32], now_ns: u64) {
        let target = self.shared.target.load(Relaxed) as f64;
        let mut level = self.level(now_ns);

        if target != self.drift.target {
            // Atraso mudou: para aumentar, toca silêncio até encher; para
            // diminuir, o resync abaixo descarta o excesso.
            self.drift.target = target;
            if level < target - self.in_period as f64 {
                self.priming = true;
            }
        }
        if self.priming {
            if level < target {
                return; // enchendo: este cabo contribui com silêncio
            }
            self.priming = false;
            self.rs.reset();
            self.drift.reset(level);
        }
        if level > target + self.slack as f64 {
            let drop = (level - target) as usize * CHANNELS;
            let drop = drop.min(self.rx.slots());
            if let Ok(chunk) = self.rx.read_chunk(drop) {
                chunk.commit_all();
            }
            self.shared.resyncs.fetch_add(1, Relaxed);
            level = target;
            self.drift.reset(level);
        }
        self.shared.level.store(level as u32, Relaxed);

        let n = out.len().min(self.scratch.len());
        let frames = n / CHANNELS;
        let ratio = self.drift.update(level, frames as f64 / SAMPLE_RATE as f64);
        self.shared.adjust_bits.store((ratio - 1.0).to_bits(), Relaxed);

        let buf = &mut self.scratch[..n];
        if !self.rs.process(buf, ratio, &mut self.rx) {
            self.shared.underruns.fetch_add(1, Relaxed);
            self.priming = true;
            return;
        }

        // Mesmo mudo, o cabo consome do ring: ao desmutar, volta sem atraso.
        let g1 = if self.shared.mute.load(Relaxed) { 0.0 } else { self.shared.gain() };
        let g0 = self.cur_gain;
        self.cur_gain = g1;
        if g0 == g1 {
            if g1 != 0.0 {
                for (o, s) in out.iter_mut().zip(buf.iter()) {
                    *o += s * g1;
                }
            }
            return;
        }
        // Rampa linear ao longo do bloco: mudar o ganho de uma vez faria um
        // degrau na onda, que se ouve como estalo. g é calculado a partir de
        // g0 em cada frame para o último chegar exatamente em g1.
        let delta = g1 - g0;
        for (f, (o, s)) in out
            .chunks_exact_mut(CHANNELS)
            .zip(buf.chunks_exact(CHANNELS))
            .enumerate()
        {
            let g = g0 + delta * (f + 1) as f32 / frames as f32;
            for (oc, sc) in o.iter_mut().zip(s) {
                *oc += sc * g;
            }
        }
    }
}

impl End for RenderEnd {
    fn route_id(&self) -> u64 {
        self.shared.id
    }
}
