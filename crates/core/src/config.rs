//! Formato, leitura, validação e gravação do config.json.
//!
//! Mesmo formato da versão Go: um config serve para as duas.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DeviceInfo, DeviceKind, ShareMode};

/// O buffer precisa de alguns períodos do dispositivo mais margem para o
/// jitter. Em shared mode (blocos de 10 ms) 30 ms é seguro; em exclusive
/// mode dá para ir bem abaixo.
pub const DEFAULT_BUFFER_MS: u32 = 30;
pub const DEFAULT_GAIN: f64 = 1.0;

pub const MIN_BUFFER_MS: u32 = 1;
pub const MAX_BUFFER_MS: u32 = 500;
pub const MAX_DELAY_MS: u32 = 5000;
pub const MAX_GAIN: f64 = 4.0;
pub const MIN_PERIOD_MS: f64 = 1.0;
pub const MAX_PERIOD_MS: f64 = 100.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_buffer_ms")]
    pub buffer_ms: u32,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub routes: Vec<Route>,
}

/// Ajustes de um dispositivo. O tipo (entrada/saída) quem informa é o nó.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Device {
    pub id: Uuid,
    /// Nome do sistema; só informativo.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub delay_ms: u32,
    /// "shared" (padrão, omitido no arquivo) ou "exclusive".
    #[serde(default, skip_serializing_if = "is_shared")]
    pub mode: ShareMode,
    /// Período desejado em ms (ex.: 5). Omitido: padrão do modo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_ms: Option<f64>,
    /// Computador da rede onde o dispositivo fica (omitido: este computador).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

impl Device {
    pub fn new(id: Uuid, name: impl Into<String>) -> Self {
        Self { id, name: name.into(), description: String::new(), delay_ms: 0, mode: ShareMode::Shared, period_ms: None, node: None }
    }

    /// Período desejado em frames, se configurado.
    pub fn period_frames(&self) -> Option<u32> {
        self.period_ms.map(|ms| (ms * crate::audio::SAMPLE_RATE as f64 / 1000.0).round() as u32)
    }
}

/// Um dispositivo do config que reapareceu com outro GUID.
#[derive(Debug, Clone, PartialEq)]
pub struct Rebind {
    pub old: Uuid,
    pub new: Uuid,
    pub name: String,
}

fn is_shared(mode: &ShareMode) -> bool {
    *mode == ShareMode::Shared
}

/// Um cabo de uma entrada (`from`) para uma saída (`to`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub from: Uuid,
    pub to: Uuid,
    #[serde(default = "default_gain")]
    pub gain: f64,
    #[serde(default)]
    pub mute: bool,
}

fn default_buffer_ms() -> u32 {
    DEFAULT_BUFFER_MS
}
fn default_gain() -> f64 {
    DEFAULT_GAIN
}

impl Default for Config {
    fn default() -> Self {
        Self { buffer_ms: DEFAULT_BUFFER_MS, devices: Vec::new(), routes: Vec::new() }
    }
}

/// Erros de config: vários problemas juntos, para corrigir tudo de uma vez.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("JSON inválido na linha {line}, coluna {column}: {message}")]
    Json { line: usize, column: usize, message: String },
    #[error("{}", .0.join("\n"))]
    Invalid(Vec<String>),
}

impl Config {
    /// Lê, decodifica e valida um arquivo.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        Self::parse(&std::fs::read(path)?)
    }

    /// Decodifica e valida. Aceita UTF-8 com BOM (editores do Windows).
    pub fn parse(data: &[u8]) -> Result<Self, ConfigError> {
        let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
        let cfg: Config = serde_json::from_slice(data).map_err(|e| ConfigError::Json {
            line: e.line(),
            column: e.column(),
            message: e.to_string(),
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Grava de forma atômica (arquivo temporário + renomear).
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut json = serde_json::to_vec_pretty(self).expect("Config sempre serializa");
        json.push(b'\n');
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Regras que não dependem dos dispositivos reais.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errs = Vec::new();

        if !(MIN_BUFFER_MS..=MAX_BUFFER_MS).contains(&self.buffer_ms) {
            errs.push(format!(
                "bufferMs: {} fora do intervalo {MIN_BUFFER_MS}–{MAX_BUFFER_MS}",
                self.buffer_ms
            ));
        }

        let mut seen_devices = HashMap::new();
        for (i, d) in self.devices.iter().enumerate() {
            if d.id.is_nil() {
                errs.push(format!("devices[{i}]: id obrigatório"));
            } else if let Some(first) = seen_devices.insert(d.id, i) {
                errs.push(format!("devices[{i}]: id {} repetido (já usado em devices[{first}])", d.id));
            }
            if d.delay_ms > MAX_DELAY_MS {
                errs.push(format!("devices[{i}]: delayMs {} fora do intervalo 0–{MAX_DELAY_MS}", d.delay_ms));
            }
            if let Some(ms) = d.period_ms
                && !(MIN_PERIOD_MS..=MAX_PERIOD_MS).contains(&ms)
            {
                errs.push(format!("devices[{i}]: periodMs {ms} fora do intervalo {MIN_PERIOD_MS}–{MAX_PERIOD_MS}"));
            }
        }

        let mut seen_routes = HashMap::new();
        for (i, r) in self.routes.iter().enumerate() {
            if r.from.is_nil() {
                errs.push(format!("routes[{i}]: from obrigatório"));
            }
            if r.to.is_nil() {
                errs.push(format!("routes[{i}]: to obrigatório"));
            }
            if !r.from.is_nil() && r.from == r.to {
                errs.push(format!("routes[{i}]: from e to são o mesmo dispositivo"));
            }
            if !(0.0..=MAX_GAIN).contains(&r.gain) {
                errs.push(format!("routes[{i}]: gain {} fora do intervalo 0–{MAX_GAIN}", r.gain));
            }
            if let Some(first) = seen_routes.insert((r.from, r.to), i) {
                errs.push(format!("routes[{i}]: rota repetida (igual a routes[{first}])"));
            }
        }

        if errs.is_empty() { Ok(()) } else { Err(ConfigError::Invalid(errs)) }
    }

    /// Confere o config contra os dispositivos disponíveis. Rotas com
    /// dispositivo inexistente ou de tipo errado são erro; ajustes de
    /// dispositivos ausentes ou com nome diferente viram avisos.
    pub fn check(&self, available: &[DeviceInfo]) -> Result<Vec<String>, ConfigError> {
        let by_id: HashMap<Uuid, &DeviceInfo> = available.iter().map(|d| (d.id, d)).collect();
        let mut errs = Vec::new();

        for (i, r) in self.routes.iter().enumerate() {
            check_end(&mut errs, &by_id, i, "from", r.from, DeviceKind::Input);
            check_end(&mut errs, &by_id, i, "to", r.to, DeviceKind::Output);
        }

        let mut warnings = Vec::new();
        for (i, cd) in self.devices.iter().enumerate() {
            match by_id.get(&cd.id) {
                None => warnings.push(format!("devices[{i}]: {} não encontrado (desconectado?)", cd.id)),
                Some(d) if !cd.name.is_empty() && cd.name != d.name => warnings.push(format!(
                    "devices[{i}]: name {:?} difere do nome atual {:?}",
                    cd.name, d.name
                )),
                Some(_) => {}
            }
        }

        if errs.is_empty() { Ok(warnings) } else { Err(ConfigError::Invalid(errs)) }
    }

    /// Religa dispositivos que sumiram mas reapareceram com outro GUID.
    ///
    /// O GUID vem do ID que o Windows dá ao endpoint. Quando o driver
    /// reinicia ou o dispositivo muda de porta USB, o Windows pode criar um
    /// endpoint novo, e o nome ganha ou perde um prefixo ("(2- GK71)" →
    /// "(GK71)"). Se houver exatamente um dispositivo presente, do mesmo
    /// tipo e com o mesmo nome normalizado, os cabos passam para ele.
    pub fn rebind_missing(&mut self, available: &[DeviceInfo]) -> Vec<Rebind> {
        let present: HashMap<Uuid, &DeviceInfo> = available.iter().map(|d| (d.id, d)).collect();
        let mut used: std::collections::HashSet<Uuid> = self.devices.iter().map(|d| d.id).collect();
        used.extend(self.routes.iter().flat_map(|r| [r.from, r.to]));

        let mut rebinds = Vec::new();
        for i in 0..self.devices.len() {
            let old = &self.devices[i];
            if present.contains_key(&old.id) || old.name.is_empty() {
                continue;
            }
            let Some(kind) = self.kind_from_routes(old.id) else { continue };
            let wanted = normalize_name(&old.name);
            let mut candidates = available
                .iter()
                // Só no mesmo computador: o microfone de outro nó com o mesmo
                // nome não é o mesmo microfone.
                .filter(|d| d.kind == kind && d.node == old.node && !used.contains(&d.id) && normalize_name(&d.name) == wanted);
            let (Some(found), None) = (candidates.next(), candidates.next()) else { continue };

            let old_id = old.id;
            rebinds.push(Rebind { old: old_id, new: found.id, name: found.name.clone() });
            used.insert(found.id);
            self.devices[i].id = found.id;
            self.devices[i].name = found.name.clone();
            for r in &mut self.routes {
                if r.from == old_id {
                    r.from = found.id;
                }
                if r.to == old_id {
                    r.to = found.id;
                }
            }
        }
        rebinds
    }

    fn kind_from_routes(&self, id: Uuid) -> Option<DeviceKind> {
        self.routes.iter().find_map(|r| {
            if r.from == id {
                Some(DeviceKind::Input)
            } else if r.to == id {
                Some(DeviceKind::Output)
            } else {
                None
            }
        })
    }
}

/// Nome sem o prefixo de instância que o Windows acrescenta ("(2- GK71)" →
/// "(gk71)"), em minúsculas.
fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(i) = rest.find('(') {
        out.push_str(&rest[..=i]);
        rest = &rest[i + 1..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && rest[digits..].starts_with("- ") {
            rest = &rest[digits + 2..];
        }
    }
    out.push_str(rest);
    out.to_lowercase()
}

fn check_end(
    errs: &mut Vec<String>,
    by_id: &HashMap<Uuid, &DeviceInfo>,
    i: usize,
    field: &str,
    id: Uuid,
    want: DeviceKind,
) {
    match by_id.get(&id) {
        None => errs.push(format!("routes[{i}]: {field} {id} não encontrado (rode --list)")),
        Some(d) if d.kind != want => errs.push(format!(
            "routes[{i}]: {field} {:?} é {}; precisa ser {}",
            d.name,
            KindPt(d.kind),
            KindPt(want)
        )),
        Some(_) => {}
    }
}

struct KindPt(DeviceKind);

impl fmt::Display for KindPt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            DeviceKind::Input => "uma entrada",
            DeviceKind::Output => "uma saída",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIC: &str = "11111111-1111-5111-8111-111111111111";
    const SPK: &str = "22222222-2222-5222-8222-222222222222";

    fn parse(json: &str) -> Result<Config, ConfigError> {
        Config::parse(json.as_bytes())
    }

    #[test]
    fn defaults_are_applied() {
        let cfg = parse(&format!(
            r#"{{"routes":[{{"from":"{MIC}","to":"{SPK}"}},{{"from":"{SPK}","to":"{MIC}","gain":0}}]}}"#
        ))
        .unwrap();
        assert_eq!(cfg.buffer_ms, DEFAULT_BUFFER_MS);
        assert_eq!(cfg.routes[0].gain, DEFAULT_GAIN);
        assert_eq!(cfg.routes[1].gain, 0.0, "gain 0 explícito deve continuar 0");
    }

    #[test]
    fn accepts_bom() {
        assert!(Config::parse(b"\xEF\xBB\xBF{\"bufferMs\": 20}").is_ok());
    }

    #[test]
    fn rejects_with_clear_messages() {
        let cases = [
            (r#"{"buferMs": 20}"#.to_string(), "unknown field"),
            ("{\n  \"bufferMs\": 20,\n}".to_string(), "linha 3"),
            (r#"{"bufferMs": 0}"#.to_string(), "bufferMs: 0"),
            (format!(r#"{{"devices":[{{"id":"{MIC}"}},{{"id":"{MIC}"}}]}}"#), "repetido"),
            (format!(r#"{{"routes":[{{"from":"{MIC}","to":"{MIC}"}}]}}"#), "mesmo dispositivo"),
            (format!(r#"{{"routes":[{{"from":"{MIC}","to":"{SPK}","gain":9}}]}}"#), "gain 9"),
            (r#"{"routes":[{"from":"abc","to":"x"}]}"#.to_string(), "linha 1"),
        ];
        for (json, want) in cases {
            let err = parse(&json).expect_err(&json).to_string();
            assert!(err.contains(want), "erro {err:?} não contém {want:?}");
        }
    }

    /// O caso real: o headset "(2- GK71)" voltou como "(GK71)" com outro GUID.
    #[test]
    fn rebinds_device_recreated_by_windows() {
        let (old_mic, old_phones) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let (new_mic, new_phones, monitor) = (Uuid::from_u128(11), Uuid::from_u128(12), Uuid::from_u128(13));
        let mut cfg = Config {
            devices: vec![
                Device::new(old_mic, "Grupo de microfones (2- GK71)"),
                Device::new(old_phones, "Fones de ouvido (2- GK71)"),
            ],
            routes: vec![Route { from: old_mic, to: old_phones, gain: 1.0, mute: false }],
            ..Default::default()
        };
        let info = |id, name: &str, kind, loopback| DeviceInfo { id, name: name.into(), kind, loopback, is_default: false, node: None };
        let available = [
            info(new_mic, "Grupo de microfones (GK71)", DeviceKind::Input, false),
            info(new_phones, "Fones de ouvido (GK71)", DeviceKind::Output, false),
            // O loopback tem o mesmo nome da saída, mas é entrada: não confunde.
            info(Uuid::from_u128(14), "Fones de ouvido (GK71)", DeviceKind::Input, true),
            info(monitor, "2 - XB273K V5", DeviceKind::Output, false),
        ];

        let rebinds = cfg.rebind_missing(&available);
        assert_eq!(rebinds.len(), 2);
        assert_eq!(cfg.routes[0].from, new_mic);
        assert_eq!(cfg.routes[0].to, new_phones);
        assert_eq!(cfg.devices[1].name, "Fones de ouvido (GK71)");
        assert!(cfg.check(&available).is_ok());
    }

    #[test]
    fn normalizes_windows_instance_prefix() {
        assert_eq!(normalize_name("Fones de ouvido (2- GK71)"), "fones de ouvido (gk71)");
        assert_eq!(normalize_name("Fones de ouvido (GK71)"), "fones de ouvido (gk71)");
        assert_eq!(normalize_name("2 - XB273K V5 (AMD)"), "2 - xb273k v5 (amd)");
    }

    #[test]
    fn same_format_as_go() {
        // Ida e volta: o JSON gravado usa os mesmos nomes de campo do Go.
        let cfg = parse(&format!(
            r#"{{"bufferMs":30,"devices":[{{"id":"{MIC}","description":"x","delayMs":5}}],"routes":[{{"from":"{MIC}","to":"{SPK}","gain":0.5,"mute":true}}]}}"#
        ))
        .unwrap();
        let json = serde_json::to_string(&cfg).unwrap();
        for field in ["\"bufferMs\"", "\"delayMs\"", "\"description\"", "\"gain\"", "\"mute\""] {
            assert!(json.contains(field), "{json} sem {field}");
        }
    }
}
