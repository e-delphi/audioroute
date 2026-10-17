//! Sondagem: o que cada dispositivo oferece em cada modo, sem abrir streams.

use audioroute_core::audio::SAMPLE_RATE;
use windows::Win32::Media::Audio::{IAudioClient, IAudioClient3, WAVEFORMATEX};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree};

use crate::com::ComGuard;
use crate::format::NativeFormat;
use crate::stream::exclusive_format;

/// O que um dispositivo aceita, com períodos em frames a 48 kHz.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    /// Formato de mistura do Windows (shared).
    pub mix_format: Option<String>,
    /// Período padrão do shared (normalmente 10 ms).
    pub shared_default: Option<u32>,
    /// Menor período do shared via IAudioClient3 (se o driver oferecer).
    pub shared_min: Option<u32>,
    /// Formato aceito em exclusive a 48 kHz.
    pub exclusive_format: Option<String>,
    /// Menor período do hardware em exclusive.
    pub exclusive_min: Option<u32>,
    pub error: Option<String>,
}

const REFTIMES_PER_SEC: i64 = 10_000_000;

fn to_frames(reftime: i64) -> u32 {
    (reftime * SAMPLE_RATE as i64 / REFTIMES_PER_SEC) as u32
}

pub(crate) fn probe(endpoint_id: &str, is_capture: bool) -> Probe {
    let run = || -> windows::core::Result<Probe> {
        let _com = ComGuard::new()?;
        let device = crate::stream::device_for_probe(endpoint_id)?;
        let client3: IAudioClient3 = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let client: IAudioClient = windows::core::Interface::cast(&client3)?;
        let mut p = Probe::default();

        let mix = unsafe { client.GetMixFormat()? };
        let parsed = unsafe { NativeFormat::from_waveformat(mix) };
        p.mix_format = parsed.map(|f| f.describe());

        let (mut default, mut min) = (0i64, 0i64);
        unsafe { client.GetDevicePeriod(Some(&mut default), Some(&mut min))? };
        p.shared_default = Some(to_frames(default));
        p.exclusive_min = Some(to_frames(min));

        if parsed.is_some_and(|f| f.rate == SAMPLE_RATE) {
            let (mut d, mut f, mut mn, mut mx) = (0, 0, 0, 0);
            let r = unsafe {
                client3.GetSharedModeEnginePeriod(mix as *const WAVEFORMATEX, &mut d, &mut f, &mut mn, &mut mx)
            };
            if r.is_ok() {
                p.shared_min = Some(mn);
            }
        }
        unsafe { CoTaskMemFree(Some(mix as *const _)) };

        p.exclusive_format = exclusive_format(&client, is_capture).map(|f| f.describe());
        Ok(p)
    };
    run().unwrap_or_else(|e| Probe { error: Some(e.message()), ..Default::default() })
}
