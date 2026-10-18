//! Compensação de drift: um controlador PI mantém o nível do buffer da rota
//! no alvo, ajustando a razão do reamostrador.
//!
//! Modelo: d(nível)/dt = drift − ajuste. Com ajuste = Kp·e + Ki·∫e, o sistema
//! fecha em s² + Kp·s + Ki. Kp = 2ωn e Ki = ωn² dão amortecimento crítico;
//! ωn = 0,1 rad/s converge em ~40 s, ignorando o jitter dos callbacks.

use crate::audio::SAMPLE_RATE;

const WN: f64 = 0.1;
const KP: f64 = 2.0 * WN;
const KI: f64 = WN * WN;
/// Constante de tempo (s) da média do nível.
const LEVEL_SMOOTHING: f64 = 1.0;
/// ±2000 ppm ≈ ±3,5 cents: inaudível.
pub const MAX_ADJUST: f64 = 2000e-6;

#[derive(Debug, Clone)]
pub struct DriftController {
    /// Nível desejado, em frames.
    pub target: f64,
    level: f64,
    integral: f64,
    started: bool,
}

impl DriftController {
    pub fn new(target: f64) -> Self {
        Self { target, level: 0.0, integral: 0.0, started: false }
    }

    /// Recomeça a média do nível (após priming ou resync). O integral é
    /// mantido: ele guarda o drift aprendido, que não mudou.
    pub fn reset(&mut self, level: f64) {
        self.level = level;
        self.started = true;
    }

    /// Recebe o nível atual (frames) e o tempo desde a última chamada (s);
    /// retorna a razão de reamostragem.
    pub fn update(&mut self, level: f64, dt: f64) -> f64 {
        if !self.started {
            self.reset(level);
        }
        self.level += (level - self.level) * dt / LEVEL_SMOOTHING;

        let e = (self.level - self.target) / SAMPLE_RATE as f64; // segundos de áudio
        let mut adjust = KP * e + KI * self.integral;
        if adjust > MAX_ADJUST {
            adjust = MAX_ADJUST;
        } else if adjust < -MAX_ADJUST {
            adjust = -MAX_ADJUST;
        } else {
            // Anti-windup: só acumula enquanto não está no limite.
            self.integral += e * dt;
        }
        1.0 + adjust
    }
}
