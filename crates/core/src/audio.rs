//! Contratos entre o motor e os backends de áudio.
//!
//! O áudio é "empurrado" por callbacks no ritmo do dispositivo, que é como as
//! APIs de áudio funcionam. O formato trocado é sempre o interno do motor:
//! f32 intercalado, [`CHANNELS`] canais, [`SAMPLE_RATE`] Hz. Converter do
//! formato nativo do dispositivo é responsabilidade do backend.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

/// Como o dispositivo é aberto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareMode {
    /// Compartilhado com os outros programas. Usa o menor período que o
    /// driver oferece (IAudioClient3); sem suporte, cai para 10 ms.
    #[default]
    Shared,
    /// Exclusivo: menor latência, mas nenhum outro programa usa o dispositivo.
    Exclusive,
}

impl fmt::Display for ShareMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShareMode::Shared => "shared",
            ShareMode::Exclusive => "exclusive",
        })
    }
}

/// Período padrão do exclusive mode. O mínimo que o driver anuncia (muitas
/// vezes 3 ms) nem sempre é estável: um headset USB caiu com 3 ms nos testes.
pub const DEFAULT_EXCLUSIVE_PERIOD_FRAMES: u32 = SAMPLE_RATE / 200; // 5 ms

/// Como abrir um dispositivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamRequest {
    pub mode: ShareMode,
    /// Período desejado em frames. `None`: o padrão do modo (exclusive:
    /// [`DEFAULT_EXCLUSIVE_PERIOD_FRAMES`]; shared: o menor que o Windows
    /// oferece). O backend arredonda para o que o hardware aceitar.
    pub period_frames: Option<u32>,
}

/// O que o backend conseguiu ao abrir o dispositivo.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub mode: ShareMode,
    /// Frames entregues a cada evento do dispositivo.
    pub period_frames: u32,
    /// Maior bloco que um callback pode receber.
    pub max_callback_frames: u32,
    /// Latência do próprio dispositivo/driver, em frames, quando conhecida.
    pub device_latency_frames: u32,
    /// Formato nativo negociado (ex.: "48000 Hz, 2 ch, f32").
    pub native_format: String,
    /// Os callbacks andam pelo relógio do sistema, sem a irregularidade de
    /// um driver de hardware (ex.: os dispositivos virtuais do audioroute).
    /// Um cabo entre dois assim dispensa o piso de buffer (`bufferMs`).
    pub steady_clock: bool,
}

impl StreamInfo {
    pub fn period_ms(&self) -> f64 {
        self.period_frames as f64 * 1000.0 / SAMPLE_RATE as f64
    }
}

/// Recebe as amostras capturadas. Roda na thread de áudio: não pode
/// bloquear nem alocar memória.
pub type CaptureCallback = Box<dyn FnMut(&[f32]) + Send>;

/// Preenche as amostras a tocar. Mesmas regras de [`CaptureCallback`].
pub type RenderCallback = Box<dyn FnMut(&mut [f32]) + Send>;

/// Um dispositivo aberto e rodando. Soltar (drop) para e libera.
pub trait Stream: Send {
    fn info(&self) -> &StreamInfo;

    /// `false` quando o stream parou sozinho (ex.: dispositivo desconectado
    /// ou driver reiniciado). O motor então fecha e tenta reabrir depois.
    fn is_alive(&self) -> bool {
        true
    }
}

pub type StreamError = Box<dyn std::error::Error + Send + Sync>;

/// Abre dispositivos para captura ou reprodução.
pub trait StreamOpener: Send {
    fn open_capture(
        &self,
        id: Uuid,
        request: StreamRequest,
        callback: CaptureCallback,
    ) -> Result<Box<dyn Stream>, StreamError>;

    fn open_render(
        &self,
        id: Uuid,
        request: StreamRequest,
        callback: RenderCallback,
    ) -> Result<Box<dyn Stream>, StreamError>;
}
