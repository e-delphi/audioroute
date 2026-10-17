//! Núcleo do audioroute: tipos, identidade, config e motor de áudio.
//!
//! Este crate não depende de plataforma: o backend WASAPI (e, no futuro, o de
//! rede e o do Android) implementa os traits definidos aqui.

pub mod audio;
pub mod config;
pub mod device;
pub mod engine;
pub mod identity;

pub use audio::{ShareMode, Stream, StreamInfo, StreamOpener, StreamRequest};
pub use device::{DeviceInfo, DeviceKind, DeviceLister};
pub use engine::Engine;
pub use identity::{IdGenerator, Role};
