//! Linha de comando do audioroute.
//!
//!   audioroute-cli --list                    lista os dispositivos
//!   audioroute-cli --list --json             lista no formato do config.json
//!   audioroute-cli --probe                   o que cada dispositivo aceita em cada modo
//!   audioroute-cli config.json               roda as rotas até Ctrl+C
//!   audioroute-cli config.json --duration 30 roda por 30 segundos

mod measure;
mod run;

use anyhow::{Context, Result};
use audioroute_core::config;
use audioroute_core::{DeviceInfo, DeviceKind, DeviceLister, IdGenerator};
use audioroute_wasapi::WasapiBackend;

fn main() {
    if let Err(e) = start() {
        eprintln!("erro: {e:#}");
        std::process::exit(1);
    }
}

fn start() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a.trim_start_matches('-') == flag && a.starts_with('-'));
    let value = |flag: &str| {
        args.iter()
            .position(|a| a.trim_start_matches('-') == flag && a.starts_with('-'))
            .and_then(|i| args.get(i + 1))
    };

    if has("list") {
        // --raw: como o Windows vê (sem juntar os virtuais do driver).
        let devices = if has("raw") { backend()?.list()? } else { local_backend()?.list()? };
        if has("json") { print_json(&devices) } else { print_table(&devices) };
        return Ok(());
    }
    // Pares virtuais do driver.
    if has("virtual") || value("virtual-add").is_some() || value("virtual-remove").is_some() {
        let v = local_backend()?;
        if let Some(name) = value("virtual-add") {
            let pair = v.add_pair(name).map_err(anyhow::Error::msg)?;
            println!("par {} em uso: {name}", pair + 1);
        }
        if let Some(n) = value("virtual-remove") {
            let n: u32 = n.parse().context("--virtual-remove N (1, 2, ...)")?;
            v.remove_pair(n.checked_sub(1).context("os pares começam em 1")?).map_err(anyhow::Error::msg)?;
        }
        for w in v.apply_windows() {
            println!("aviso: {w}");
        }
        for p in v.pairs() {
            println!("par {}  {:<20} alto-falante {}  microfone {}", p.pair + 1, format!("{:?}", p.name), p.speaker, p.mic);
        }
        return Ok(());
    }
    // Diagnóstico dos dispositivos do driver no Windows.
    if let Some(pattern) = value("filters") {
        let pattern = pattern.to_lowercase();
        let matches = |e: &audioroute_wasapi::FilterEndpoint| {
            e.name.to_lowercase().contains(&pattern) || e.filter.to_lowercase().contains(&pattern)
        };
        for e in audioroute_wasapi::filter_endpoints()?.into_iter().filter(matches) {
            println!(
                "{} {}  {}  {:?}  {}",
                if e.output { "saída  " } else { "entrada" },
                if e.active { "ativo   " } else { "escondido" },
                e.endpoint_id,
                e.name,
                e.filter
            );
        }
        return Ok(());
    }
    if let Some(id) = value("endpoint-name") {
        let name = args.iter().skip_while(|a| *a != id).nth(1).context("uso: --endpoint-name ID NOME")?;
        audioroute_wasapi::set_endpoint_name(id, name)?;
        return Ok(());
    }
    if let Some(id) = value("endpoint-visible") {
        let on = args.iter().skip_while(|a| *a != id).nth(1).context("uso: --endpoint-visible ID 0|1")?;
        audioroute_wasapi::set_endpoint_visible(id, on == "1")?;
        return Ok(());
    }
    if has("probe") {
        return probe(&backend()?);
    }
    if has("measure") {
        let period_arg = value("period");
        let ids: Vec<&String> = args.iter().filter(|a| !a.starts_with('-') && Some(*a) != period_arg).collect();
        let [out, inp] = ids[..] else {
            anyhow::bail!("uso: audioroute-cli --measure SAIDA_ID ENTRADA_ID [--exclusive]");
        };
        let period_ms = value("period").map(|s| s.parse::<f64>()).transpose().context("--period em ms")?;
        let request = audioroute_core::StreamRequest {
            mode: if has("exclusive") { audioroute_core::ShareMode::Exclusive } else { Default::default() },
            period_frames: period_ms.map(|ms| (ms * 48.0).round() as u32),
        };
        return measure::measure(backend()?, out.parse()?, inp.parse()?, request, 6);
    }
    let duration = value("duration").map(|s| s.parse::<u64>()).transpose().context("--duration em segundos")?;
    if let Some(path) = args.iter().find(|a| !a.starts_with('-') && Some(*a) != value("duration")) {
        if has("check") {
            return run::check(&local_backend()?, path);
        }
        return run::run(local_backend()?, path, duration.map(std::time::Duration::from_secs));
    }
    eprintln!(
        "Uso:\n  audioroute-cli --list [--json] [--raw]\n  audioroute-cli --probe\n  audioroute-cli config.json --check\n  audioroute-cli config.json [--duration SEGUNDOS]"
    );
    Ok(())
}

pub fn backend() -> Result<WasapiBackend> {
    let machine_id = audioroute_wasapi::machine_id().context("lendo MachineGuid")?;
    Ok(WasapiBackend::new(IdGenerator::new(machine_id)))
}

/// WASAPI + o alto-falante e o microfone virtuais do driver (se instalado).
pub fn local_backend() -> Result<audioroute_virtual::VirtualBackend> {
    let machine_id = audioroute_wasapi::machine_id().context("lendo MachineGuid")?;
    let ids = IdGenerator::new(machine_id);
    // Os mesmos pares do app (%APPDATA%\com.audioroute.appirtual.json).
    let state = std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("com.audioroute.app").join("virtual.json"));
    Ok(audioroute_virtual::VirtualBackend::new(WasapiBackend::new(ids.clone()), &ids, state))
}

fn print_table(devices: &[DeviceInfo]) {
    let section = |title: &str, matches: &dyn Fn(&DeviceInfo) -> bool| {
        println!("\n{title}");
        let mut any = false;
        for d in devices.iter().filter(|d| matches(d)) {
            any = true;
            let mark = if d.is_default { '*' } else { ' ' };
            println!(" {mark} {}  {}", d.id, d.name);
        }
        if !any {
            println!("   (nenhum)");
        }
    };
    section("ENTRADAS", &|d| d.kind == DeviceKind::Input && !d.loopback);
    section("SAÍDAS", &|d| d.kind == DeviceKind::Output);
    section("LOOPBACK (captura o que toca na saída)", &|d| d.loopback);
    println!("\n * = dispositivo padrão");
}

fn print_json(devices: &[DeviceInfo]) {
    #[derive(serde::Serialize)]
    struct Out {
        devices: Vec<config::Device>,
    }
    let out = Out {
        devices: devices
            .iter()
            .map(|d| config::Device::new(d.id, d.name.clone()))
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&out).expect("serializa"));
}

fn probe(backend: &WasapiBackend) -> Result<()> {
    let ms = |frames: Option<u32>| match frames {
        Some(f) => format!("{:>5.2} ms", f as f64 / 48.0),
        None => "    —   ".into(),
    };
    println!("Períodos por modo (quanto menor, menor a latência):\n");
    println!("{:<48} {:>10} {:>10} {:>10}  formato exclusive", "dispositivo", "shared", "shared LL", "exclusive");
    for e in backend.endpoints()?.iter().filter(|e| !e.info.loopback) {
        let p = backend.probe(e);
        let icon = if e.info.kind == DeviceKind::Input { "🎤" } else { "🔊" };
        let name: String = format!("{icon} {}", e.info.name).chars().take(46).collect();
        if let Some(err) = p.error {
            println!("{name:<48} erro: {err}");
            continue;
        }
        println!(
            "{name:<48} {:>10} {:>10} {:>10}  {}",
            ms(p.shared_default),
            ms(p.shared_min),
            ms(p.exclusive_min),
            p.exclusive_format.unwrap_or_else(|| "não aceita 48 kHz".into())
        );
    }
    println!("\nshared LL = shared de baixa latência (IAudioClient3); — = indisponível.");
    Ok(())
}
