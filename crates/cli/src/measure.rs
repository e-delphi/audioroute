//! Medição de latência real: toca cliques numa saída e detecta na entrada.
//!
//! Serve para cabos virtuais (o que toca na saída aparece na entrada) e para
//! hardware com a saída ligada fisicamente na entrada. Mede de programa a
//! programa: buffers do Windows, driver e dispositivo, nos dois sentidos.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use audioroute_core::audio::{CHANNELS, SAMPLE_RATE};
use audioroute_core::{StreamOpener, StreamRequest};
use audioroute_wasapi::WasapiBackend;
use uuid::Uuid;

/// Um clique a cada meio segundo.
const CLICK_EVERY: u64 = SAMPLE_RATE as u64 / 2;
const CLICK_FRAMES: u64 = 24; // 0,5 ms
const THRESHOLD: f32 = 0.1;

pub fn measure(backend: WasapiBackend, output: Uuid, input: Uuid, request: StreamRequest, seconds: u64) -> Result<()> {
    let start = Instant::now();
    let ns = move || start.elapsed().as_nanos() as u64;
    let frame_ns = 1e9 / SAMPLE_RATE as f64;

    // Instante (ns) em que o último clique foi entregue ao Windows.
    let emitted = Arc::new(AtomicU64::new(0));
    let results: Arc<Mutex<Vec<f64>>> = Arc::default();

    let emit = emitted.clone();
    let mut played = 0u64;
    let _out = backend
        .open_render(
            output,
            request,
            Box::new(move |buf: &mut [f32]| {
                let now = ns();
                for (i, frame) in buf.chunks_exact_mut(CHANNELS).enumerate() {
                    let pos = (played + i as u64) % CLICK_EVERY;
                    let v = if pos < CLICK_FRAMES { 0.8 } else { 0.0 };
                    frame.fill(v);
                    if pos == 0 {
                        emit.store(now + (i as f64 * frame_ns) as u64, Relaxed);
                    }
                }
                played += (buf.len() / CHANNELS) as u64;
            }),
        )
        .map_err(|e| anyhow::anyhow!("abrindo a saída: {e}"))?;

    let (emit, res) = (emitted.clone(), results.clone());
    let mut armed_after = 0u64;
    let _in = backend
        .open_capture(
            input,
            request,
            Box::new(move |buf: &[f32]| {
                let now = ns();
                let frames = buf.len() / CHANNELS;
                for (i, frame) in buf.chunks_exact(CHANNELS).enumerate() {
                    let level = frame.iter().fold(0f32, |m, s| m.max(s.abs()));
                    // Instante aproximado em que esta amostra foi capturada.
                    let at = now.saturating_sub(((frames - i) as f64 * frame_ns) as u64);
                    if level > THRESHOLD && at > armed_after {
                        let sent = emit.load(Relaxed);
                        if sent > 0 && at > sent {
                            res.lock().unwrap().push((at - sent) as f64 / 1e6);
                        }
                        armed_after = at + 250_000_000; // ignora o resto deste clique
                    }
                }
            }),
        )
        .map_err(|e| anyhow::anyhow!("abrindo a entrada: {e}"))?;

    for (label, s) in [("saída", &_out), ("entrada", &_in)] {
        let i = s.info();
        println!("{label}: {} · período {:.2} ms · buffer {} frames", i.native_format, i.period_ms(), i.max_callback_frames);
    }
    println!("medindo por {seconds} s...");
    std::thread::sleep(Duration::from_secs(seconds));

    let mut r = results.lock().unwrap().clone();
    if r.is_empty() {
        bail!("nenhum clique chegou na entrada: o que toca nesta saída não aparece nesta entrada");
    }
    r.sort_by(f64::total_cmp);
    let median = r[r.len() / 2];
    println!(
        "{} cliques detectados · latência mínima {:.1} ms · mediana {median:.1} ms · máxima {:.1} ms",
        r.len(),
        r[0],
        r[r.len() - 1]
    );
    Ok(())
}
