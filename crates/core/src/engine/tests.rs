//! Testes do motor com um backend falso: os callbacks são chamados à mão,
//! sem placa de som.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::audio::{CaptureCallback, RenderCallback};

const PERIOD: u32 = 96; // 2 ms

#[derive(Default)]
struct FakeState {
    capture: HashMap<Uuid, CaptureCallback>,
    render: HashMap<Uuid, RenderCallback>,
    open: HashMap<Uuid, StreamRequest>,
    dead: std::collections::HashSet<Uuid>,
}

#[derive(Clone, Default)]
struct FakeOpener {
    state: Arc<Mutex<FakeState>>,
    broken: Option<Uuid>,
    /// Dispositivos que dizem andar pelo relógio do sistema.
    steady: Vec<Uuid>,
}

struct FakeStream {
    id: Uuid,
    info: StreamInfo,
    state: Arc<Mutex<FakeState>>,
}

impl Stream for FakeStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn is_alive(&self) -> bool {
        !self.state.lock().unwrap().dead.contains(&self.id)
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        let mut s = self.state.lock().unwrap();
        s.open.remove(&self.id);
        s.capture.remove(&self.id);
        s.render.remove(&self.id);
    }
}

impl FakeOpener {
    fn stream(&self, id: Uuid, request: StreamRequest) -> Result<Box<dyn Stream>, StreamError> {
        if Some(id) == self.broken {
            return Err("falha simulada".into());
        }
        self.state.lock().unwrap().open.insert(id, request);
        let info = StreamInfo {
            mode: request.mode,
            period_frames: PERIOD,
            max_callback_frames: PERIOD,
            device_latency_frames: PERIOD,
            native_format: "fake".into(),
            steady_clock: self.steady.contains(&id),
        };
        Ok(Box::new(FakeStream { id, info, state: self.state.clone() }))
    }

    fn feed(&self, id: Uuid, v: f32) {
        let block = vec![v; PERIOD as usize * CHANNELS];
        (self.state.lock().unwrap().capture.get_mut(&id).unwrap())(&block);
    }

    fn play(&self, id: Uuid) -> Vec<f32> {
        let mut out = vec![0.0; PERIOD as usize * CHANNELS];
        (self.state.lock().unwrap().render.get_mut(&id).unwrap())(&mut out);
        out
    }

    fn is_open(&self, id: Uuid) -> bool {
        self.state.lock().unwrap().open.contains_key(&id)
    }
}

impl StreamOpener for FakeOpener {
    fn open_capture(&self, id: Uuid, req: StreamRequest, cb: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        let s = self.stream(id, req)?;
        self.state.lock().unwrap().capture.insert(id, cb);
        Ok(s)
    }

    fn open_render(&self, id: Uuid, req: StreamRequest, cb: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        let s = self.stream(id, req)?;
        self.state.lock().unwrap().render.insert(id, cb);
        Ok(s)
    }
}

const MIC_A: Uuid = Uuid::from_u128(0xa1);
const MIC_B: Uuid = Uuid::from_u128(0xa2);
const SPEAKER: Uuid = Uuid::from_u128(0xb1);
const PHONES: Uuid = Uuid::from_u128(0xb2);
const BROKEN: Uuid = Uuid::from_u128(0xc1);

/// bufferMs 1 → o alvo vira o mínimo seguro: 2 períodos (4 ms) + 1 ms de margem.
fn engine() -> (Engine, FakeOpener) {
    let opener = FakeOpener { broken: Some(BROKEN), ..Default::default() };
    (Engine::new(Box::new(opener.clone()), 1), opener)
}

/// Blocos necessários para encher o alvo (5 ms = 240 frames = 2,5 blocos).
const FILL: usize = 3;

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn steady_clock_cables_skip_the_buffer_floor() {
    let opener = FakeOpener { steady: vec![MIC_A, SPEAKER], ..Default::default() };
    let mut e = Engine::new(Box::new(opener), 30);
    e.connect(MIC_A, SPEAKER, 1.0, false).unwrap(); // os dois no relógio do sistema
    e.connect(MIC_B, SPEAKER, 1.0, false).unwrap(); // um de hardware no meio
    let mut target = |from| {
        let (_, routes) = e.stats();
        routes.into_iter().find(|r| r.from == from).unwrap().target
    };
    // Mínimo: 2 períodos (4 ms) + 5 ms de margem para as threads.
    assert_eq!(target(MIC_A), frames_to_duration(2 * PERIOD + 240));
    // Com hardware: o piso de 30 ms continua.
    assert_eq!(target(MIC_B), Duration::from_millis(30));
}

#[test]
fn mixes_two_inputs_with_gain() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 1.0, false).unwrap();
    e.connect(MIC_B, SPEAKER, 0.5, false).unwrap();

    o.feed(MIC_A, 0.25);
    o.feed(MIC_B, 0.5);
    assert_eq!(o.play(SPEAKER)[0], 0.0, "durante o priming toca silêncio");

    for _ in 1..FILL {
        o.feed(MIC_A, 0.25);
        o.feed(MIC_B, 0.5);
    }
    o.play(SPEAKER); // primeiro bloco faz a rampa de ganho a partir de 0
    o.feed(MIC_A, 0.25);
    o.feed(MIC_B, 0.5);
    let out = o.play(SPEAKER);
    assert!(near(*out.last().unwrap(), 0.25 + 0.5 * 0.5), "mix = {}", out.last().unwrap());
}

#[test]
fn fan_out_and_clamp() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 2.0, false).unwrap();
    e.connect(MIC_A, PHONES, 0.5, false).unwrap();
    for _ in 0..FILL + 2 {
        o.feed(MIC_A, 0.8);
    }
    o.play(SPEAKER);
    o.play(PHONES);
    assert_eq!(*o.play(SPEAKER).last().unwrap(), 1.0, "0,8×2 limitado a 1");
    assert!(near(*o.play(PHONES).last().unwrap(), 0.4));
}

#[test]
fn gain_change_ramps() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 1.0, false).unwrap();
    for _ in 0..FILL {
        o.feed(MIC_A, 0.5);
    }
    // Uso real: entrada e saída alternando, um bloco de cada.
    for _ in 0..2 {
        o.play(SPEAKER);
        o.feed(MIC_A, 0.5);
    }
    e.set_gain(MIC_A, SPEAKER, 0.0).unwrap();
    let out = o.play(SPEAKER);
    assert!(out[0] > 0.4 && *out.last().unwrap() == 0.0, "rampa: {} → {}", out[0], out.last().unwrap());
}

#[test]
fn devices_open_and_close_with_cables() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 1.0, false).unwrap();
    e.connect(MIC_A, PHONES, 1.0, false).unwrap();
    assert!(o.is_open(MIC_A) && o.is_open(SPEAKER) && o.is_open(PHONES));

    e.disconnect(MIC_A, SPEAKER).unwrap();
    assert!(!o.is_open(SPEAKER) && o.is_open(MIC_A));
    e.disconnect(MIC_A, PHONES).unwrap();
    assert!(!o.is_open(MIC_A) && !o.is_open(PHONES));
}

#[test]
fn errors() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 1.0, false).unwrap();
    assert!(matches!(e.connect(MIC_A, SPEAKER, 1.0, false), Err(EngineError::RouteExists)));
    assert!(matches!(e.disconnect(MIC_B, SPEAKER), Err(EngineError::NoRoute)));
    assert!(e.connect(MIC_B, BROKEN, 1.0, false).is_err());
    assert!(!o.is_open(MIC_B), "entrada aberta só para o cabo que falhou deve fechar");
    assert!(matches!(e.connect(SPEAKER, PHONES, 1.0, false), Err(EngineError::WrongKind(..))));
}

#[test]
fn delay_and_mode_reopen_keep_cables() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 0.7, true).unwrap();
    e.set_device_delay(MIC_A, 10).unwrap();
    e.set_device_mode(SPEAKER, ShareMode::Exclusive).unwrap();

    e.set_device_period(SPEAKER, Some(144)).unwrap();
    let opened = o.state.lock().unwrap().open[&SPEAKER];
    assert_eq!(opened, StreamRequest { mode: ShareMode::Exclusive, period_frames: Some(144) });
    let (_, routes) = e.stats();
    assert_eq!(routes.len(), 1);
    // 5 ms (mínimo) + 10 ms de atraso
    assert_eq!(routes[0].target, frames_to_duration(240 + 480));
    let r = &e.routes[&(MIC_A, SPEAKER)];
    assert_eq!(r.gain(), 0.7);
    assert!(r.mute.load(Relaxed));
}

/// Um dispositivo que cai é detectado; seus cabos voltam para quem chamou
/// religar quando ele reaparecer.
#[test]
fn failed_device_is_detected_and_detached() {
    let (mut e, o) = engine();
    e.connect(MIC_A, SPEAKER, 0.5, false).unwrap();
    e.connect(MIC_A, PHONES, 1.0, true).unwrap();
    assert!(e.failed_devices().is_empty());

    o.state.lock().unwrap().dead.insert(SPEAKER);
    assert_eq!(e.failed_devices(), vec![SPEAKER]);

    let detached = e.detach_device(SPEAKER);
    assert_eq!(detached, vec![RouteSpec { from: MIC_A, to: SPEAKER, gain: 0.5, mute: false }]);
    assert!(!o.is_open(SPEAKER), "dispositivo caído deve ser fechado");
    assert!(o.is_open(MIC_A) && o.is_open(PHONES), "o resto continua");

    o.state.lock().unwrap().dead.clear();
    let r = detached[0];
    e.connect(r.from, r.to, r.gain, r.mute).unwrap();
    assert!(o.is_open(SPEAKER));
}

/// Relógios divergentes com jitter, por 10 minutos simulados: o controlador
/// precisa achar o drift sem underruns nem resyncs.
#[test]
fn drift_is_compensated() {
    use rand_like::Lcg;
    for drift_ppm in [0.0, 100.0, -100.0, 500.0, -500.0] {
        const P: u32 = 480; // 10 ms
        let shared = Arc::new(RouteShared::new(1, MIC_A, SPEAKER, 1.0, false, 30 * 48));
        let (tx, rx) = RingBuffer::new((30 * 48 + 8 * P as usize) * CHANNELS);
        let mut writer = CaptureEnd::new(shared.clone(), tx);
        let mut reader = RenderEnd::new(shared.clone(), rx, P, P, P);

        let block = vec![0.0f32; P as usize * CHANNELS];
        let mut out = vec![0.0f32; P as usize * CHANNELS];
        let period_in = 0.010 / (1.0 + drift_ppm * 1e-6);
        let mut rng = Lcg(7);
        let (mut k_in, mut k_out) = (0u64, 0u64);
        let next_in = |k: u64, r: &mut Lcg| k as f64 * period_in + r.next() * 0.002;
        let next_out = |k: u64, r: &mut Lcg| 0.003 + k as f64 * 0.010 + r.next() * 0.002;
        let (mut t_in, mut t_out) = (next_in(0, &mut rng), next_out(0, &mut rng));
        let (mut sum_adjust, mut n) = (0.0, 0);

        while t_out < 600.0 {
            if t_in <= t_out {
                writer.write(&block, (t_in * 1e9) as u64);
                k_in += 1;
                t_in = next_in(k_in, &mut rng);
            } else {
                reader.mix_into(&mut out, (t_out * 1e9) as u64);
                k_out += 1;
                t_out = next_out(k_out, &mut rng);
                if t_out > 300.0 {
                    sum_adjust += f64::from_bits(shared.adjust_bits.load(Relaxed)) * 1e6;
                    n += 1;
                }
            }
        }
        let adjust = sum_adjust / n as f64;
        let failures = shared.underruns.load(Relaxed) + shared.resyncs.load(Relaxed);
        assert_eq!(failures, 0, "drift {drift_ppm}: {failures} falhas");
        assert!((adjust - drift_ppm).abs() < 2.0, "drift {drift_ppm}: ajuste {adjust:.1}");
    }
}

mod rand_like {
    /// Gerador simples e determinístico (sem dependência externa).
    pub struct Lcg(pub u64);
    impl Lcg {
        pub fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }
}
