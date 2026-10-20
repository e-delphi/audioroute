//! Roda as rotas de um config.json mostrando estatísticas.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use audioroute_core::config::Config;
use audioroute_core::engine::RouteSpec;
use audioroute_core::{DeviceKind, DeviceLister, Engine, StreamOpener, StreamRequest};
use uuid::Uuid;

const STATS_INTERVAL: Duration = Duration::from_secs(2);

/// Só carrega, religa e valida o config, sem abrir dispositivo nenhum.
pub fn check(backend: &impl DeviceLister, path: &str) -> Result<()> {
    let mut cfg = Config::load(path).with_context(|| path.to_string())?;
    let devices = backend.list()?;
    for r in cfg.rebind_missing(&devices) {
        println!("religado: {} → {:?} ({})", r.old, r.name, r.new);
    }
    for warning in cfg.check(&devices)? {
        println!("aviso: {warning}");
    }
    let name = |id: &Uuid| devices.iter().find(|d| d.id == *id).map_or("?", |d| d.name.as_str());
    println!("config OK: {path}");
    for r in &cfg.routes {
        println!("  {} → {}  (gain {:.2}{})", name(&r.from), name(&r.to), r.gain, if r.mute { ", mudo" } else { "" });
    }
    Ok(())
}

pub fn run<B: DeviceLister + StreamOpener + 'static>(backend: B, path: &str, duration: Option<Duration>) -> Result<()> {
    let mut cfg = Config::load(path).with_context(|| path.to_string())?;
    let devices = backend.list()?;
    for r in cfg.rebind_missing(&devices) {
        eprintln!("aviso: dispositivo {} reapareceu como {:?} ({}); cabos religados", r.old, r.name, r.new);
    }
    for warning in cfg.check(&devices)? {
        eprintln!("aviso: {warning}");
    }
    let names: HashMap<Uuid, String> = devices
        .iter()
        .map(|d| (d.id, if d.loopback { format!("{} [loopback]", d.name) } else { d.name.clone() }))
        .collect();
    let name = |id: &Uuid| names.get(id).map(String::as_str).unwrap_or("?");

    let mut engine = Engine::new(Box::new(backend), cfg.buffer_ms);
    for d in &cfg.devices {
        engine.set_device_request(d.id, StreamRequest { mode: d.mode, period_frames: d.period_frames() })?;
        engine.set_device_delay(d.id, d.delay_ms)?;
    }
    for r in &cfg.routes {
        engine
            .connect(r.from, r.to, r.gain as f32, r.mute)
            .with_context(|| format!("{} → {}", name(&r.from), name(&r.to)))?;
    }

    println!("config: {path}  (bufferMs pedido: {})", cfg.buffer_ms);
    for r in &cfg.routes {
        let latency = engine.estimated_latency(r.from, r.to).unwrap_or_default();
        println!(
            "  {} → {}  latência estimada {:.1} ms{}",
            name(&r.from),
            name(&r.to),
            latency.as_secs_f64() * 1000.0,
            if r.mute { " (mudo)" } else { "" }
        );
    }
    println!("\nrodando — Ctrl+C para parar");

    let start = Instant::now();
    engine.stats(); // descarta a janela de abertura dos dispositivos
    // Cabos de dispositivos que caíram, esperando o dispositivo voltar.
    let mut pending: Vec<RouteSpec> = Vec::new();
    loop {
        std::thread::sleep(STATS_INTERVAL);

        for id in engine.failed_devices() {
            eprintln!("aviso: {} parou (desconectado ou driver reiniciado); tentando reabrir", name(&id));
            pending.extend(engine.detach_device(id));
        }
        pending.retain(|r| match engine.connect(r.from, r.to, r.gain, r.mute) {
            Ok(()) => {
                eprintln!("religado: {} → {}", name(&r.from), name(&r.to));
                false
            }
            Err(_) => true,
        });

        let (devs, routes) = engine.stats();
        println!("\n[{:>4.0}s]", start.elapsed().as_secs_f64());
        for d in &devs {
            let kind = if d.kind == DeviceKind::Input { "in " } else { "out" };
            println!(
                "  {kind} {:<34.34} {:<38.38} período {:>5.2} ms  {:>4} cb/s  ocupado méd {:>4} µs máx {:>4} µs  intervalo {:.2}–{:.2} ms",
                name(&d.id),
                d.info.native_format,
                d.info.period_ms(),
                (d.calls as f64 / STATS_INTERVAL.as_secs_f64()) as u64,
                d.busy_avg.as_micros(),
                d.busy_max.as_micros(),
                d.gap_min.as_secs_f64() * 1000.0,
                d.gap_max.as_secs_f64() * 1000.0,
            );
        }
        for r in &routes {
            println!(
                "  cabo {:.22} → {:.22}  buffer {:>5.2} ms (alvo {:.2})  drift {:+6.1} ppm  underruns {}  overruns {}  resyncs {}",
                name(&r.from),
                name(&r.to),
                r.level.as_secs_f64() * 1000.0,
                r.target.as_secs_f64() * 1000.0,
                r.adjust_ppm,
                r.underruns,
                r.overruns,
                r.resyncs
            );
        }
        if duration.is_some_and(|d| start.elapsed() >= d) {
            break;
        }
    }
    println!("parando...");
    Ok(())
}
