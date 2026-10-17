//! Nível de áudio de qualquer dispositivo, seja quem for que o esteja usando
//! (outro programa, o próprio Windows ou nós).
//!
//! Usa a IAudioMeterInformation: o mesmo medidor das barras verdes do Painel
//! de Som. Não abre stream; só lê o pico que o Windows já calcula.

use std::collections::HashMap;

use uuid::Uuid;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::HSTRING;

use crate::com::ComGuard;

/// Lê os picos. Objetos COM: crie e use sempre na mesma thread (não é Send).
pub struct SystemMeter {
    enumerator: IMMDeviceEnumerator,
    meters: HashMap<String, IAudioMeterInformation>,
    // Último campo: o COM só é desinicializado depois dos objetos acima.
    _com: ComGuard,
}

impl SystemMeter {
    pub fn new() -> windows::core::Result<Self> {
        let com = ComGuard::new()?;
        let enumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        Ok(Self { enumerator, meters: HashMap::new(), _com: com })
    }

    /// Pico atual (0–1) de cada dispositivo. `endpoints`: GUID → ID do endpoint.
    pub fn peaks<'a>(&mut self, endpoints: impl IntoIterator<Item = (Uuid, &'a str)>) -> HashMap<Uuid, f32> {
        let mut out = HashMap::new();
        for (id, endpoint) in endpoints {
            if let Some(peak) = self.peak(endpoint) {
                out.insert(id, peak);
            }
        }
        out
    }

    fn peak(&mut self, endpoint: &str) -> Option<f32> {
        if !self.meters.contains_key(endpoint) {
            let device = unsafe { self.enumerator.GetDevice(&HSTRING::from(endpoint)) }.ok()?;
            let meter: IAudioMeterInformation = unsafe { device.Activate(CLSCTX_ALL, None) }.ok()?;
            self.meters.insert(endpoint.to_string(), meter);
        }
        match unsafe { self.meters[endpoint].GetPeakValue() } {
            Ok(v) => Some(v),
            Err(_) => {
                // Dispositivo removido ou invalidado: tenta de novo depois.
                self.meters.remove(endpoint);
                None
            }
        }
    }
}
