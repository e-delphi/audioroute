//! Reamostrador para compensar a diferença entre os relógios de duas placas.
//!
//! Lê a entrada a uma razão ajustável (ex.: 1.000083 frames de entrada por
//! frame de saída) e calcula os valores entre as amostras por interpolação
//! cúbica de Hermite (Catmull-Rom). Não aloca memória depois de `new`.

/// Fonte das amostras de entrada (o lado consumidor do ring buffer).
pub trait Reader {
    /// Amostras disponíveis para leitura.
    fn available(&self) -> usize;
    /// Copia exatamente `out.len()` amostras (o chamador garante que há).
    fn read_exact(&mut self, out: &mut [f32]);
}

impl Reader for rtrb::Consumer<f32> {
    fn available(&self) -> usize {
        self.slots()
    }

    fn read_exact(&mut self, out: &mut [f32]) {
        let chunk = self.read_chunk(out.len()).expect("chamador conferiu available()");
        let (a, b) = chunk.as_slices();
        out[..a.len()].copy_from_slice(a);
        out[a.len()..].copy_from_slice(b);
        chunk.commit_all();
    }
}

pub struct Resampler {
    channels: usize,
    max_ratio: f64,
    /// Frames de entrada: os guardados do bloco anterior + os recém-lidos.
    work: Vec<f32>,
    /// Quantos frames guardados estão no início de `work`.
    carry: usize,
    /// Posição (fracionária, em frames) da próxima saída dentro de `work`.
    pos: f64,
}

impl Resampler {
    pub fn new(channels: usize, max_frames: usize, max_ratio: f64) -> Self {
        let mut r = Self {
            channels,
            max_ratio,
            work: vec![0.0; ((max_frames as f64 * max_ratio) as usize + 8) * channels],
            carry: 0,
            pos: 1.0,
        };
        r.reset();
        r
    }

    /// Descarta o estado (usado após um underrun).
    pub fn reset(&mut self) {
        self.carry = 0;
        // A interpolação precisa de 1 frame antes da posição atual; começar
        // em 1 usa o primeiro frame lido como esse "anterior".
        self.pos = 1.0;
    }

    /// Frames de entrada guardados e ainda não tocados.
    pub fn buffered(&self) -> f64 {
        (self.carry as f64 - self.pos).max(0.0)
    }

    /// Preenche `out` (intercalado) lendo de `src` à razão indicada. Se
    /// faltar entrada, não consome nada, zera `out` e retorna `false`.
    pub fn process(&mut self, out: &mut [f32], ratio: f64, src: &mut impl Reader) -> bool {
        let ch = self.channels;
        let frames = out.len() / ch;
        if frames == 0 {
            return true;
        }
        let ratio = ratio.min(self.max_ratio);

        // Último frame necessário: a última saída fica em pos+(frames-1)*ratio
        // e a interpolação usa até 2 frames depois dela.
        let need = (self.pos + (frames - 1) as f64 * ratio) as usize + 3;
        if need > self.carry {
            let to_read = (need - self.carry) * ch;
            if src.available() < to_read {
                out.fill(0.0);
                return false;
            }
            src.read_exact(&mut self.work[self.carry * ch..need * ch]);
        }

        let w = &self.work;
        let mut pos = self.pos;
        for frame in out.chunks_exact_mut(ch) {
            let i = pos as usize;
            let t = (pos - i as f64) as f32;
            let base = (i - 1) * ch;
            for (c, sample) in frame.iter_mut().enumerate() {
                let y0 = w[base + c];
                let y1 = w[base + ch + c];
                let y2 = w[base + 2 * ch + c];
                let y3 = w[base + 3 * ch + c];
                // Catmull-Rom: passa exatamente pelas amostras (t=0 → y1).
                let c1 = 0.5 * (y2 - y0);
                let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
                *sample = ((c3 * t + c2) * t + c1) * t + y1;
            }
            pos += ratio;
        }

        // Guarda a partir do frame anterior à próxima posição.
        let keep_from = pos as usize - 1;
        self.work.copy_within(keep_from * ch..need * ch, 0);
        self.carry = need - keep_from;
        self.pos = pos - keep_from as f64;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(samples: &[f32]) -> rtrb::Consumer<f32> {
        let (mut tx, rx) = rtrb::RingBuffer::new(samples.len().max(1) * 2);
        for &s in samples {
            tx.push(s).unwrap();
        }
        rx
    }

    #[test]
    fn ratio_one_is_passthrough() {
        let input: Vec<f32> = (0..200).map(|i| i as f32).collect();
        let mut src = ring(&input);
        let mut r = Resampler::new(2, 32, 1.01);
        let mut got = Vec::new();
        let mut out = [0.0f32; 64];
        for _ in 0..2 {
            assert!(r.process(&mut out, 1.0, &mut src));
            got.extend_from_slice(&out);
        }
        // O frame 0 só serve de "anterior": a saída começa no frame 1.
        for (i, v) in got.iter().enumerate() {
            assert_eq!(*v, (i + 2) as f32, "amostra {i}");
        }
    }

    #[test]
    fn sine_stays_clean_with_drift() {
        let ratio = 1.0005; // 500 ppm, bem acima do drift real
        let sine = |frame: f64| (2.0 * std::f64::consts::PI * 1000.0 * frame / 48000.0).sin() as f32;
        let input: Vec<f32> = (0..20000).map(|i| sine(i as f64)).collect();
        let mut src = ring(&input);
        let mut r = Resampler::new(1, 480, 1.01);
        let mut out = vec![0.0f32; 480];
        let mut worst = 0.0f32;
        let mut n = 0usize;
        for _ in 0..20 {
            assert!(r.process(&mut out, ratio, &mut src));
            for v in &out {
                worst = worst.max((v - sine(1.0 + n as f64 * ratio)).abs());
                n += 1;
            }
        }
        assert!(worst < 1e-3, "erro máximo {worst:e}");
    }

    #[test]
    fn underrun_consumes_nothing() {
        let mut src = ring(&[0.0; 10]);
        let mut r = Resampler::new(2, 32, 1.01);
        assert!(!r.process(&mut [0.0; 64], 1.0, &mut src));
        assert_eq!(src.available(), 10);
    }
}
