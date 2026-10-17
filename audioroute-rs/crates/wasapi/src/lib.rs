//! Backend de áudio do Windows (WASAPI) para o audioroute.
//!
//! Fala direto com a Core Audio API pelo crate oficial `windows`, sem camada
//! intermediária: é o que permite usar exclusive mode e os períodos mínimos
//! que cada dispositivo aceita.

mod com;
mod enumerate;
mod format;
mod machine_id;
mod meter;
mod policy;
mod probe;
mod stream;

pub use enumerate::{Endpoint, Error, WasapiBackend};
pub use machine_id::machine_id;
pub use meter::SystemMeter;
pub use probe::Probe;
pub use stream::WasapiStream;
