//! Pacote de áudio (UDP).
//!
//! ```text
//! 0  u16  MAGIC
//! 2  u8   versão
//! 3  u8   canais
//! 4  u32  stream
//! 8  u64  posição do 1º frame (contagem desde o início do stream)
//! 16 u16  frames
//! 18 ...  amostras i16 intercaladas
//! ```
//! Tudo em little-endian. A posição permite ao receptor completar com
//! silêncio o que se perdeu e descartar o que chegou atrasado.

use audioroute_core::audio::CHANNELS;

pub const MAGIC: u16 = 0xA0D1;
pub const VERSION: u8 = 1;
pub const HEADER: usize = 18;
/// 5 ms a 48 kHz: 960 bytes de áudio, bem abaixo do MTU de 1500.
pub const MAX_FRAMES: usize = 240;
pub const MAX_PACKET: usize = HEADER + MAX_FRAMES * CHANNELS * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub stream: u32,
    pub pos: u64,
    pub frames: usize,
}

/// Escreve o pacote em `out` (até [`MAX_FRAMES`] frames de `samples`) e
/// devolve o tamanho.
pub fn encode(out: &mut [u8; MAX_PACKET], stream: u32, pos: u64, samples: &[f32]) -> usize {
    let frames = (samples.len() / CHANNELS).min(MAX_FRAMES);
    out[0..2].copy_from_slice(&MAGIC.to_le_bytes());
    out[2] = VERSION;
    out[3] = CHANNELS as u8;
    out[4..8].copy_from_slice(&stream.to_le_bytes());
    out[8..16].copy_from_slice(&pos.to_le_bytes());
    out[16..18].copy_from_slice(&(frames as u16).to_le_bytes());
    for (i, s) in samples[..frames * CHANNELS].iter().enumerate() {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out[HEADER + 2 * i..HEADER + 2 * i + 2].copy_from_slice(&v.to_le_bytes());
    }
    HEADER + frames * CHANNELS * 2
}

/// Lê o cabeçalho; `None` se não for um pacote nosso.
pub fn decode_header(packet: &[u8]) -> Option<Header> {
    if packet.len() < HEADER
        || u16::from_le_bytes([packet[0], packet[1]]) != MAGIC
        || packet[2] != VERSION
        || packet[3] as usize != CHANNELS
    {
        return None;
    }
    let frames = u16::from_le_bytes([packet[16], packet[17]]) as usize;
    if frames > MAX_FRAMES || packet.len() < HEADER + frames * CHANNELS * 2 {
        return None;
    }
    Some(Header {
        stream: u32::from_le_bytes(packet[4..8].try_into().unwrap()),
        pos: u64::from_le_bytes(packet[8..16].try_into().unwrap()),
        frames,
    })
}

/// Converte as amostras do pacote para f32 em `out` (tamanho frames × canais).
pub fn decode_samples(packet: &[u8], out: &mut [f32]) {
    for (i, o) in out.iter_mut().enumerate() {
        let v = i16::from_le_bytes([packet[HEADER + 2 * i], packet[HEADER + 2 * i + 1]]);
        *o = v as f32 / i16::MAX as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let samples: Vec<f32> = (0..2 * 100).map(|i| (i as f32 / 200.0) - 0.5).collect();
        let mut buf = [0u8; MAX_PACKET];
        let n = encode(&mut buf, 7, 12_345, &samples);
        let h = decode_header(&buf[..n]).unwrap();
        assert_eq!(h, Header { stream: 7, pos: 12_345, frames: 100 });
        let mut out = vec![0.0; 200];
        decode_samples(&buf[..n], &mut out);
        for (a, b) in samples.iter().zip(&out) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_header(b"hello world, not audio").is_none());
        let mut buf = [0u8; MAX_PACKET];
        let n = encode(&mut buf, 1, 0, &[0.0; 4]);
        assert!(decode_header(&buf[..n - 1]).is_none());
    }

    #[test]
    fn clamps_and_limits_frames() {
        let mut buf = [0u8; MAX_PACKET];
        let n = encode(&mut buf, 1, 0, &vec![2.0; 2 * (MAX_FRAMES + 50)]);
        assert_eq!(n, MAX_PACKET);
        let mut out = vec![0.0; 2 * MAX_FRAMES];
        decode_samples(&buf[..n], &mut out);
        assert!(out.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }
}
