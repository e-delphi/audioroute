//! Testes do serviço com um backend falso (sem placa de som).

use std::sync::{Arc, Mutex};

use audioroute_control::Service;
use audioroute_core::audio::{CaptureCallback, RenderCallback, StreamError};
use audioroute_core::config::Config;
use audioroute_core::{DeviceInfo, DeviceKind, DeviceLister, ShareMode, Stream, StreamInfo, StreamOpener, StreamRequest};
use uuid::Uuid;

#[derive(Clone)]
struct FakeBackend {
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
}

struct NopStream(StreamInfo);
impl Stream for NopStream {
    fn info(&self) -> &StreamInfo {
        &self.0
    }
}

#[derive(Debug)]
struct NoError;
impl std::fmt::Display for NoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nunca")
    }
}
impl std::error::Error for NoError {}

impl DeviceLister for FakeBackend {
    type Error = NoError;
    fn list(&self) -> Result<Vec<DeviceInfo>, NoError> {
        Ok(self.devices.lock().unwrap().clone())
    }
}

impl FakeBackend {
    fn stream(request: StreamRequest) -> Box<dyn Stream> {
        Box::new(NopStream(StreamInfo {
            mode: request.mode,
            period_frames: 480,
            max_callback_frames: 480,
            device_latency_frames: 480,
            native_format: "fake".into(),
        steady_clock: false,
        }))
    }
}

impl StreamOpener for FakeBackend {
    fn open_capture(&self, _: Uuid, r: StreamRequest, _: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        Ok(Self::stream(r))
    }
    fn open_render(&self, _: Uuid, r: StreamRequest, _: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        Ok(Self::stream(r))
    }
}

fn device(n: u128, name: &str, kind: DeviceKind, loopback: bool) -> DeviceInfo {
    DeviceInfo { id: Uuid::from_u128(n), name: name.into(), kind, loopback, is_default: false, node: None }
}

fn setup() -> (FakeBackend, DeviceInfo, DeviceInfo, DeviceInfo, std::path::PathBuf) {
    let mic = device(1, "Mic", DeviceKind::Input, false);
    let spk = device(2, "Caixa", DeviceKind::Output, false);
    let lb = device(3, "Caixa", DeviceKind::Input, true);
    let backend = FakeBackend { devices: Arc::new(Mutex::new(vec![mic.clone(), spk.clone(), lb.clone()])) };
    // Pasta única por teste (os testes rodam em paralelo).
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("audioroute-test-{nanos}-{:?}", std::thread::current().id()));
    (backend, mic, spk, lb, dir.join("config.json"))
}

#[test]
fn connect_saves_and_reloads() {
    let (backend, mic, spk, _, path) = setup();
    let mut s = Service::open(backend.clone(), &path).unwrap();
    s.connect(&mic.id.to_string(), &spk.id.to_string()).unwrap();
    s.set_gain(&mic.id.to_string(), &spk.id.to_string(), 0.5).unwrap();
    s.set_mode(&spk.id.to_string(), "exclusive").unwrap();
    s.set_period(&spk.id.to_string(), Some(3.0)).unwrap();

    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.routes[0].gain, 0.5);
    let spk_cfg = cfg.devices.iter().find(|d| d.id == spk.id).unwrap();
    assert_eq!((spk_cfg.mode, spk_cfg.period_ms), (ShareMode::Exclusive, Some(3.0)));

    // A caixa aberta reflete o pedido.
    let view = s.devices().into_iter().find(|d| d.id == spk.id.to_string()).unwrap();
    assert_eq!(view.stream.unwrap().mode, "exclusive");

    drop(s);
    let s2 = Service::open(backend, &path).unwrap();
    assert!(s2.routes()[0].active, "ao reabrir, o cabo volta ligado");
}

#[test]
fn missing_device_keeps_cable_and_reconnects_when_back() {
    let (backend, mic, spk, _, path) = setup();
    let mut s = Service::open(backend.clone(), &path).unwrap();
    s.connect(&mic.id.to_string(), &spk.id.to_string()).unwrap();
    drop(s);

    backend.devices.lock().unwrap().retain(|d| d.id != spk.id);
    let mut s = Service::open(backend.clone(), &path).unwrap();
    assert!(!s.routes()[0].active);
    let missing = s.devices().into_iter().find(|d| d.id == spk.id.to_string()).unwrap();
    assert!(!missing.available && missing.kind == "output");

    backend.devices.lock().unwrap().push(spk.clone());
    s.refresh().unwrap();
    assert!(s.routes()[0].active, "a caixa voltou: o cabo religa sozinho");
}

#[test]
fn rejects_invalid_requests() {
    let (backend, mic, spk, lb, path) = setup();
    let mut s = Service::open(backend, &path).unwrap();
    let (m, k) = (mic.id.to_string(), spk.id.to_string());

    assert!(s.connect(&k, &m).is_err(), "cabo de saída para entrada");
    assert!(s.connect("abc", &k).is_err(), "id inválido");
    s.connect(&m, &k).unwrap();
    assert!(s.connect(&m, &k).is_err(), "cabo repetido");
    assert!(s.set_gain(&m, &k, 9.0).is_err());
    assert!(s.set_period(&k, Some(0.1)).is_err());
    assert!(s.set_mode(&lb.id.to_string(), "exclusive").is_err(), "loopback não pode ser exclusivo");
    s.disconnect(&m, &k).unwrap();
    assert!(s.routes().is_empty());
}
