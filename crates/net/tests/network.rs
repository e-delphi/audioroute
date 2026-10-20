//! Controlador e participante de verdade (TCP e UDP em 127.0.0.1), com
//! dispositivos falsos que geram e gravam áudio no ritmo do relógio.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audioroute_core::audio::{CHANNELS, CaptureCallback, RenderCallback, StreamError};
use audioroute_core::{DeviceInfo, DeviceKind, DeviceLister, Engine, ShareMode, Stream, StreamInfo, StreamOpener, StreamRequest};
use audioroute_net::{HubOptions, NetBackend, NetHub, Participant, ParticipantOptions};
use uuid::Uuid;

const BLOCK: usize = 240; // 5 ms

/// Backend falso: entradas geram um sinal constante de 0,5; saídas gravam
/// o maior valor que tocaram.
#[derive(Clone)]
struct Fake {
    devices: Vec<DeviceInfo>,
    peak: Arc<Mutex<f32>>,
}

impl Fake {
    fn new(input: u128, output: u128) -> Self {
        let dev = |id, kind| DeviceInfo {
            id: Uuid::from_u128(id),
            name: format!("fake {id:x}"),
            kind,
            loopback: false,
            is_default: false,
            node: None,
        };
        Self { devices: vec![dev(input, DeviceKind::Input), dev(output, DeviceKind::Output)], peak: Arc::default() }
    }
    fn peak(&self) -> f32 {
        *self.peak.lock().unwrap()
    }
}

#[derive(Debug)]
struct NoError;
impl std::fmt::Display for NoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("-")
    }
}
impl std::error::Error for NoError {}

impl DeviceLister for Fake {
    type Error = NoError;
    fn list(&self) -> Result<Vec<DeviceInfo>, NoError> {
        Ok(self.devices.clone())
    }
}

struct FakeStream {
    info: StreamInfo,
    stop: Arc<AtomicBool>,
}
impl Stream for FakeStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}
impl Drop for FakeStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Chama `tick` a cada 5 ms, até o stream ser solto.
fn clocked(mut tick: impl FnMut() + Send + 'static) -> Box<dyn Stream> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut done = 0u32;
        while !flag.load(Ordering::SeqCst) {
            while start.elapsed() >= Duration::from_millis(5) * done {
                tick();
                done += 1;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    Box::new(FakeStream {
        info: StreamInfo {
            mode: ShareMode::Shared,
            period_frames: BLOCK as u32,
            max_callback_frames: BLOCK as u32,
            device_latency_frames: 0,
            native_format: "fake".into(),
        steady_clock: false,
        },
        stop,
    })
}

impl StreamOpener for Fake {
    fn open_capture(&self, _: Uuid, _: StreamRequest, mut cb: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        let block = vec![0.5f32; BLOCK * CHANNELS];
        Ok(clocked(move || cb(&block)))
    }
    fn open_render(&self, _: Uuid, _: StreamRequest, mut cb: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        let peak = self.peak.clone();
        let mut block = vec![0.0f32; BLOCK * CHANNELS];
        Ok(clocked(move || {
            cb(&mut block);
            let m = block.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            let mut p = peak.lock().unwrap();
            *p = p.max(m);
        }))
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(5), "esperando: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn audio_flows_both_ways_and_disconnect_is_detected() {
    let hub = NetHub::start("CONTROLADOR".into(), "m1".into(), HubOptions { control_port: 0, announce: false }).unwrap();
    let remote = Fake::new(0x100, 0x101);
    let participant = Participant::start(
        remote.clone(),
        "m2".into(),
        "NOTEBOOK".into(),
        ParticipantOptions { buffer_ms: 30, controller: Some(format!("127.0.0.1:{}", hub.control_port())) },
    );

    wait_until("participante conectar", || hub.devices().len() == 2);
    assert!(hub.take_changed());
    assert!(hub.devices().iter().all(|d| d.node.as_deref() == Some("NOTEBOOK") && !d.is_default));
    assert_eq!(participant.status().state, "connected");
    assert_eq!(participant.status().controller.as_deref(), Some("CONTROLADOR"));
    assert_eq!(hub.peers()[0].name, "NOTEBOOK");

    let local = Fake::new(0x200, 0x201);
    let backend = NetBackend { local: local.clone(), hub: hub.clone() };
    let mut engine = Engine::new(Box::new(backend), 30);

    // Microfone de lá → fone daqui.
    engine.connect(Uuid::from_u128(0x100), Uuid::from_u128(0x201), 1.0, false).unwrap();
    // Microfone daqui → alto-falante de lá.
    engine.connect(Uuid::from_u128(0x200), Uuid::from_u128(0x101), 1.0, false).unwrap();
    wait_until("áudio da rede chegar aqui", || (local.peak() - 0.5).abs() < 0.01);
    wait_until("áudio daqui chegar lá", || (remote.peak() - 0.5).abs() < 0.01);
    wait_until("participante contar os streams", || participant.status().streams == 2);
    assert!(engine.failed_devices().is_empty());

    // O latência estimada inclui a folga da rede.
    let latency = engine.estimated_latency(Uuid::from_u128(0x100), Uuid::from_u128(0x201)).unwrap();
    assert!(latency >= Duration::from_millis(25), "{latency:?}");

    // Cabo removido: o participante fecha o dispositivo dele.
    engine.disconnect(Uuid::from_u128(0x200), Uuid::from_u128(0x101)).unwrap();
    wait_until("participante fechar o stream", || participant.status().streams == 1);

    // Participante sai: o stream cai e os dispositivos somem.
    participant.shutdown();
    wait_until("dispositivos sumirem", || hub.devices().is_empty());
    wait_until("motor perceber a queda", || engine.failed_devices() == vec![Uuid::from_u128(0x100)]);
    assert!(hub.take_changed());
    hub.shutdown();
}

#[test]
fn manual_address_can_be_changed_while_running() {
    let hub = NetHub::start("C".into(), "m1".into(), HubOptions { control_port: 0, announce: false }).unwrap();
    // Começa apontando para uma porta onde não há ninguém.
    let participant = Participant::start(
        Fake::new(0x100, 0x101),
        "m2".into(),
        "P".into(),
        ParticipantOptions { buffer_ms: 30, controller: Some("127.0.0.1:1".into()) },
    );
    wait_until("mostrar o erro", || participant.status().error.is_some());
    assert_eq!(participant.status().state, "connecting");
    assert_eq!(participant.status().manual.as_deref(), Some("127.0.0.1:1"));

    participant.set_controller(Some(format!("127.0.0.1:{}", hub.control_port())));
    wait_until("conectar no endereço novo", || participant.status().state == "connected");
    assert_eq!(participant.status().error, None);
    assert_eq!(hub.devices().len(), 2);
    participant.shutdown();
    hub.shutdown();
}

#[test]
fn resolves_addresses_with_and_without_port() {
    use audioroute_net::{message::CONTROL_PORT, resolve};
    assert_eq!(resolve("192.168.0.10").unwrap(), ([192, 168, 0, 10], CONTROL_PORT).into());
    assert_eq!(resolve(" 10.0.0.2:5000 ").unwrap(), ([10, 0, 0, 2], 5000).into());
    assert_eq!(resolve("localhost").unwrap().port(), CONTROL_PORT);
    assert!(resolve("nome-que-nao-existe.invalid").is_err());
}

#[test]
fn open_fails_cleanly_when_the_participant_refuses() {
    #[derive(Clone)]
    struct Refuses(Fake);
    impl DeviceLister for Refuses {
        type Error = NoError;
        fn list(&self) -> Result<Vec<DeviceInfo>, NoError> {
            self.0.list()
        }
    }
    impl StreamOpener for Refuses {
        fn open_capture(&self, _: Uuid, _: StreamRequest, _: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
            Err("dispositivo em uso exclusivo por outro programa".into())
        }
        fn open_render(&self, _: Uuid, _: StreamRequest, _: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
            Err("não deu".into())
        }
    }

    let hub = NetHub::start("C".into(), "m1".into(), HubOptions { control_port: 0, announce: false }).unwrap();
    let participant = Participant::start(
        Refuses(Fake::new(0x100, 0x101)),
        "m2".into(),
        "P".into(),
        ParticipantOptions { buffer_ms: 30, controller: Some(format!("127.0.0.1:{}", hub.control_port())) },
    );
    wait_until("participante conectar", || hub.devices().len() == 2);
    let err = hub
        .open_capture(Uuid::from_u128(0x100), StreamRequest::default(), Box::new(|_| {}))
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("uso exclusivo"), "{err}");
    participant.shutdown();
    hub.shutdown();
}
