//! Formatos nativos dos dispositivos e conversão para o formato interno
//! (f32 intercalado estéreo). Em exclusive mode o dispositivo dita o
//! formato: muitas vezes inteiro de 16/24 bits, e às vezes mono.

use audioroute_core::audio::CHANNELS;
use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::Media::Audio::WAVE_FORMAT_PCM;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    F32,
    I16,
    /// 24 bits empacotados em 3 bytes.
    I24,
    /// Container de 32 bits (24 ou 32 bits válidos, alinhados à esquerda).
    I32,
}

impl SampleType {
    fn bytes(self) -> usize {
        match self {
            SampleType::I16 => 2,
            SampleType::I24 => 3,
            SampleType::F32 | SampleType::I32 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeFormat {
    pub rate: u32,
    pub channels: u16,
    pub sample: SampleType,
    /// Bits válidos (ex.: 24 num container de 32).
    pub valid_bits: u16,
}

impl NativeFormat {
    pub fn new(rate: u32, channels: u16, sample: SampleType) -> Self {
        let valid_bits = (sample.bytes() * 8) as u16;
        Self { rate, channels, sample, valid_bits }
    }

    pub fn frame_bytes(&self) -> usize {
        self.channels as usize * self.sample.bytes()
    }

    pub fn describe(&self) -> String {
        let sample = match (self.sample, self.valid_bits) {
            (SampleType::F32, _) => "f32".to_string(),
            (SampleType::I32, 24) => "i24 (em 32)".to_string(),
            (s, _) => format!("i{}", s.bytes() * 8),
        };
        format!("{} Hz, {} ch, {sample}", self.rate, self.channels)
    }

    /// Lê um WAVEFORMATEX (ou EXTENSIBLE) devolvido pelo Windows.
    ///
    /// # Safety
    /// `ptr` deve apontar para um WAVEFORMATEX válido.
    pub unsafe fn from_waveformat(ptr: *const WAVEFORMATEX) -> Option<Self> {
        let wf = unsafe { ptr.read_unaligned() };
        let (tag, bits) = (wf.wFormatTag as u32, wf.wBitsPerSample);
        let float = if tag == WAVE_FORMAT_EXTENSIBLE && wf.cbSize >= 22 {
            let ext = unsafe { (ptr as *const WAVEFORMATEXTENSIBLE).read_unaligned() };
            let sub = ext.SubFormat;
            if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                true
            } else if sub == KSDATAFORMAT_SUBTYPE_PCM {
                false
            } else {
                return None;
            }
        } else {
            match tag {
                WAVE_FORMAT_IEEE_FLOAT => true,
                WAVE_FORMAT_PCM => false,
                _ => return None,
            }
        };
        let sample = match (float, bits) {
            (true, 32) => SampleType::F32,
            (false, 16) => SampleType::I16,
            (false, 24) => SampleType::I24,
            (false, 32) => SampleType::I32,
            _ => return None,
        };
        Some(Self { rate: wf.nSamplesPerSec, channels: wf.nChannels, sample, valid_bits: bits })
    }

    /// Monta o WAVEFORMATEXTENSIBLE para pedir este formato ao Windows.
    pub fn to_extensible(&self) -> WAVEFORMATEXTENSIBLE {
        let bytes = self.sample.bytes() as u16;
        let block_align = bytes * self.channels;
        let channel_mask = match self.channels {
            1 => 0x4, // SPEAKER_FRONT_CENTER
            2 => 0x3, // SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT
            _ => 0,
        };
        WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: self.channels,
                nSamplesPerSec: self.rate,
                nAvgBytesPerSec: self.rate * block_align as u32,
                nBlockAlign: block_align,
                wBitsPerSample: bytes * 8,
                cbSize: 22,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: self.valid_bits },
            dwChannelMask: channel_mask,
            SubFormat: if self.sample == SampleType::F32 {
                KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            } else {
                KSDATAFORMAT_SUBTYPE_PCM
            },
        }
    }

    /// Converte `frames` frames do formato nativo para f32 estéreo.
    /// Mono vira L=R; mais de 2 canais: usa os dois primeiros.
    pub fn to_internal(&self, src: &[u8], frames: usize, dst: &mut [f32]) {
        let ch = self.channels as usize;
        let bytes = self.sample.bytes();
        for (f, out) in dst[..frames * CHANNELS].chunks_exact_mut(CHANNELS).enumerate() {
            let frame = &src[f * ch * bytes..];
            let left = self.read(frame, 0);
            let right = if ch > 1 { self.read(frame, 1) } else { left };
            out[0] = left;
            out[1] = right;
        }
    }

    /// Converte `frames` frames de f32 estéreo para o formato nativo.
    /// Mono recebe a média de L e R; canais além de 2 ficam em silêncio.
    pub fn from_internal(&self, src: &[f32], frames: usize, dst: &mut [u8]) {
        let ch = self.channels as usize;
        let bytes = self.sample.bytes();
        for (f, inp) in src[..frames * CHANNELS].chunks_exact(CHANNELS).enumerate() {
            let frame = &mut dst[f * ch * bytes..(f + 1) * ch * bytes];
            if ch == 1 {
                self.write(frame, 0, 0.5 * (inp[0] + inp[1]));
                continue;
            }
            self.write(frame, 0, inp[0]);
            self.write(frame, 1, inp[1]);
            for c in 2..ch {
                self.write(frame, c, 0.0);
            }
        }
    }

    #[inline]
    fn read(&self, frame: &[u8], channel: usize) -> f32 {
        let i = channel * self.sample.bytes();
        match self.sample {
            SampleType::F32 => f32::from_le_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]),
            SampleType::I16 => i16::from_le_bytes([frame[i], frame[i + 1]]) as f32 / 32768.0,
            SampleType::I24 => {
                // 3 bytes → i32 com sinal (desloca para cima e volta).
                let v = i32::from_le_bytes([0, frame[i], frame[i + 1], frame[i + 2]]) >> 8;
                v as f32 / 8_388_608.0
            }
            SampleType::I32 => {
                i32::from_le_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]) as f32
                    / 2_147_483_648.0
            }
        }
    }

    #[inline]
    fn write(&self, frame: &mut [u8], channel: usize, v: f32) {
        let i = channel * self.sample.bytes();
        let v = v.clamp(-1.0, 1.0);
        match self.sample {
            SampleType::F32 => frame[i..i + 4].copy_from_slice(&v.to_le_bytes()),
            SampleType::I16 => frame[i..i + 2].copy_from_slice(&((v * 32767.0) as i16).to_le_bytes()),
            SampleType::I24 => {
                let s = ((v * 8_388_607.0) as i32).to_le_bytes();
                frame[i..i + 3].copy_from_slice(&s[..3]);
            }
            SampleType::I32 => {
                frame[i..i + 4].copy_from_slice(&((v as f64 * 2_147_483_647.0) as i32).to_le_bytes())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_types_and_channel_layouts() {
        let input = [0.5f32, -0.25, 0.0, 0.75];
        for sample in [SampleType::F32, SampleType::I16, SampleType::I24, SampleType::I32] {
            let fmt = NativeFormat::new(48000, 2, sample);
            let mut native = vec![0u8; 2 * fmt.frame_bytes()];
            fmt.from_internal(&input, 2, &mut native);
            let mut back = [0f32; 4];
            fmt.to_internal(&native, 2, &mut back);
            for (a, b) in input.iter().zip(back) {
                assert!((a - b).abs() < 1e-4, "{sample:?}: {a} → {b}");
            }
        }
    }

    #[test]
    fn mono_duplicates_and_averages() {
        let fmt = NativeFormat::new(48000, 1, SampleType::I16);
        let mut native = vec![0u8; 2];
        fmt.from_internal(&[0.5, 0.0], 1, &mut native);
        let mut back = [0f32; 2];
        fmt.to_internal(&native, 1, &mut back);
        assert!((back[0] - 0.25).abs() < 1e-3 && back[0] == back[1]);
    }
}
