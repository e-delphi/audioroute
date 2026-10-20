//! Ponte interface ↔ Rust por mensagens JSON do WebView2.
//!
//! JS → Rust: `{"id": 7, "cmd": "connect", "args": {...}}`. O pedido roda numa
//! thread de comandos (abrir um dispositivo pode levar alguns ms e não deve
//! travar a janela) e a resposta volta como `{"id": 7, "ok": true, "value": ...}`
//! ou `{"id": 7, "ok": false, "error": "..."}`.
//!
//! Rust → JS (eventos): `{"event": "levels", "payload": {...}}`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use audioroute_control::Service;
use audioroute_core::{DeviceLister, IdGenerator};
use audioroute_net::{HubOptions, NetBackend, NetHub, Participant, ParticipantOptions};
use audioroute_virtual::VirtualBackend;
use audioroute_wasapi::{SystemMeter, WasapiBackend};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ui;

const LEVELS_INTERVAL: Duration = Duration::from_millis(50);
const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// De quanto em quanto tempo a lista de endpoints é relida (medidor do
/// sistema, dispositivos plugados e saída padrão trocada fora do app).
const ENDPOINTS_INTERVAL: Duration = Duration::from_secs(3);

/// Dispositivos deste computador (WASAPI + virtuais do driver) e os dos
/// participantes da rede.
type Backend = NetBackend<VirtualBackend>;

#[derive(Default)]
struct Inner {
    role: Option<String>,
    /// Só os dispositivos deste computador.
    backend: Option<WasapiBackend>,
    /// Alto-falante e microfone virtuais (driver do audioroute).
    virt: Option<VirtualBackend>,
    service: Option<Service<Backend>>,
    /// Controlador: aceita os participantes da rede.
    hub: Option<NetHub>,
    /// Participante: conectado (ou procurando) o controlador.
    participant: Option<Participant>,
}

impl Inner {
    /// Desliga o áudio e a rede e volta ao estado sem papel.
    fn reset(&mut self) {
        self.service = None; // solta o motor: os streams param aqui
        if let Some(hub) = self.hub.take() {
            hub.shutdown();
        }
        if let Some(p) = self.participant.take() {
            p.shutdown();
        }
        self.backend = None;
        self.virt = None;
        self.role = None;
    }

    /// Estado da rede para a interface.
    fn network(&self) -> Value {
        match (&self.hub, &self.participant) {
            (Some(hub), _) => json!({
                "role": "controller",
                "peers": hub.peers(),
                // Para digitar no participante se a descoberta automática falhar.
                "address": audioroute_net::local_ip().map(|ip| ip.to_string()),
                "port": hub.control_port(),
            }),
            (_, Some(p)) => json!({ "role": "participant", "status": p.status() }),
            _ => json!({ "role": "" }),
        }
    }
}

/// Endereço do controlador digitado no participante (participant.json).
fn saved_controller() -> Option<String> {
    let text = std::fs::read_to_string(data_dir().ok()?.join("participant.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("controller").and_then(Value::as_str).map(str::to_owned)
}

fn save_controller(address: Option<&str>) -> Result<(), String> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&json!({ "controller": address })).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("participant.json"), text).map_err(|e| e.to_string())
}

/// Nome deste computador na rede.
fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "computador".into())
}

#[derive(Clone, Default)]
struct AppState(Arc<Mutex<Inner>>);

impl AppState {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Um panic com o lock travado não deve derrubar o app inteiro.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

type CmdResult = Result<Value, String>;

struct Request {
    id: Value,
    cmd: String,
    args: Value,
}

static STATE: OnceLock<AppState> = OnceLock::new();
static COMMANDS: OnceLock<Sender<Request>> = OnceLock::new();

fn state() -> &'static AppState {
    STATE.get_or_init(AppState::default)
}

/// Sobe a thread de comandos e a de eventos. Chamado uma vez, antes da janela.
pub fn start() {
    let (tx, rx) = mpsc::channel::<Request>();
    let _ = COMMANDS.set(tx);
    std::thread::Builder::new()
        .name("ui-commands".into())
        .spawn(move || {
            for req in rx {
                let result = std::panic::catch_unwind(|| handle(&req.cmd, &req.args))
                    .unwrap_or_else(|_| Err(format!("falha interna em \"{}\"", req.cmd)));
                let reply = match result {
                    Ok(value) => json!({ "id": req.id, "ok": true, "value": value }),
                    Err(error) => json!({ "id": req.id, "ok": false, "error": error }),
                };
                ui::post_json(reply.to_string());
            }
        })
        .expect("criar thread de comandos");
    spawn_events(state().clone());
}

/// Mensagem vinda do JS (thread da janela).
pub fn dispatch(raw: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(raw) else { return };
    let Some(cmd) = msg.get("cmd").and_then(Value::as_str) else { return };
    let req = Request {
        id: msg.get("id").cloned().unwrap_or(Value::Null),
        cmd: cmd.to_owned(),
        args: msg.get("args").cloned().unwrap_or(Value::Null),
    };
    if let Some(tx) = COMMANDS.get() {
        let _ = tx.send(req);
    }
}

/// Desliga o áudio e a rede antes de o processo sair.
pub fn shutdown() {
    state().lock().reset();
}

fn emit(event: &str, payload: impl serde::Serialize) {
    ui::post_json(json!({ "event": event, "payload": payload }).to_string());
}

// ---------------------------------------------------------------- comandos

fn str_arg(args: &Value, key: &str) -> Result<String, String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| format!("falta o parâmetro \"{key}\""))
}

fn f64_arg(args: &Value, key: &str) -> Result<f64, String> {
    args.get(key).and_then(Value::as_f64).ok_or_else(|| format!("falta o parâmetro \"{key}\""))
}

fn bool_arg(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key).and_then(Value::as_bool).ok_or_else(|| format!("falta o parâmetro \"{key}\""))
}

fn to_value(v: impl serde::Serialize) -> CmdResult {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn service(inner: &mut Inner) -> Result<&mut Service<Backend>, String> {
    inner.service.as_mut().ok_or_else(|| "escolha um papel na tela inicial primeiro".to_string())
}

/// `%APPDATA%\com.audioroute.app`: a mesma pasta da versão Tauri, para o
/// config e o layout continuarem valendo.
fn data_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA").ok_or("variável APPDATA não definida")?;
    Ok(PathBuf::from(appdata).join("com.audioroute.app"))
}

fn handle(cmd: &str, args: &Value) -> CmdResult {
    let st = state();
    match cmd {
        "role" => Ok(json!(st.lock().role.clone().unwrap_or_default())),
        "set_role" => set_role(st, &str_arg(args, "role")?).map(|()| Value::Null),
        "reset_role" => {
            st.lock().reset();
            Ok(Value::Null)
        }
        "network" => Ok(st.lock().network()),
        "virtual_devices" => {
            let inner = st.lock();
            Ok(match &inner.virt {
                Some(v) if v.installed() => json!({ "max": audioroute_virtual::driver::MAX_PAIRS, "pairs": v.pairs() }),
                _ => Value::Null,
            })
        }
        "virtual_add" => virtual_change(st, |v| v.add_pair(&str_arg(args, "name")?).map(|_| ())),
        "virtual_rename" => {
            let pair = f64_arg(args, "pair")? as u32;
            virtual_change(st, |v| v.rename_pair(pair, &str_arg(args, "name")?))
        }
        "virtual_remove" => {
            let pair = f64_arg(args, "pair")? as u32;
            virtual_change(st, |v| v.remove_pair(pair))
        }
        "firewall" => to_value(crate::firewall::status()),
        "allow_firewall" => to_value(crate::firewall::allow()?),
        "set_controller_address" => {
            // null ou vazio: volta à busca automática.
            let address = args.get("address").and_then(Value::as_str).map(str::trim).filter(|a| !a.is_empty());
            if let Some(a) = address {
                audioroute_net::resolve(a).map_err(|e| format!("endereço \"{a}\" inválido: {e}"))?;
            }
            save_controller(address)?;
            let inner = st.lock();
            let p = inner.participant.as_ref().ok_or("este computador não está no papel de participante")?;
            p.set_controller(address.map(str::to_owned));
            Ok(inner.network())
        }
        "devices" => {
            let mut inner = st.lock();
            if let Some(s) = inner.service.as_mut() {
                return to_value(s.devices());
            }
            let backend = inner.backend.as_ref().ok_or("escolha um papel na tela inicial primeiro")?;
            let list = backend.list().map_err(|e| e.to_string())?;
            to_value(list.iter().map(audioroute_control::device_view).collect::<Vec<_>>())
        }
        "routes" => to_value(service(&mut st.lock())?.routes()),
        "refresh" => service(&mut st.lock())?.refresh().map(|()| Value::Null).map_err(|e| e.to_string()),
        "connect" => {
            let (from, to) = (str_arg(args, "from")?, str_arg(args, "to")?);
            to_value(service(&mut st.lock())?.connect(&from, &to).map_err(|e| e.to_string())?)
        }
        "disconnect" => {
            let (from, to) = (str_arg(args, "from")?, str_arg(args, "to")?);
            service(&mut st.lock())?.disconnect(&from, &to).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_gain" => {
            let (from, to, gain) = (str_arg(args, "from")?, str_arg(args, "to")?, f64_arg(args, "gain")?);
            service(&mut st.lock())?.set_gain(&from, &to, gain).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_mute" => {
            let (from, to, mute) = (str_arg(args, "from")?, str_arg(args, "to")?, bool_arg(args, "mute")?);
            service(&mut st.lock())?.set_mute(&from, &to, mute).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_delay" => {
            let (id, ms) = (str_arg(args, "id")?, f64_arg(args, "delayMs")?);
            let ms = ms.clamp(0.0, u32::MAX as f64).round() as u32;
            service(&mut st.lock())?.set_delay(&id, ms).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_description" => {
            let (id, text) = (str_arg(args, "id")?, str_arg(args, "text")?);
            service(&mut st.lock())?.set_description(&id, &text).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_mode" => {
            let (id, mode) = (str_arg(args, "id")?, str_arg(args, "mode")?);
            service(&mut st.lock())?.set_mode(&id, &mode).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_period" => {
            let id = str_arg(args, "id")?;
            let period = args.get("periodMs").and_then(Value::as_f64);
            service(&mut st.lock())?.set_period(&id, period).map(|()| Value::Null).map_err(|e| e.to_string())
        }
        "set_windows_output" => set_windows_output(st, &str_arg(args, "id")?).map(|()| Value::Null),
        "load_layout" => match std::fs::read_to_string(data_dir()?.join("layout.json")) {
            Ok(s) => Ok(json!(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!("{}")),
            Err(e) => Err(e.to_string()),
        },
        "save_layout" => {
            let dir = data_dir()?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("layout.json"), str_arg(args, "layout")?).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        _ => Err(format!("comando desconhecido: {cmd}")),
    }
}

fn set_role(st: &AppState, role: &str) -> Result<(), String> {
    let mut inner = st.lock();
    if inner.role.as_deref() == Some(role) {
        return Ok(()); // já está neste papel
    }
    if inner.role.is_some() {
        return Err("este computador já está em outro papel; use \"Trocar papel\"".into());
    }
    if role != "controller" && role != "participant" {
        return Err(format!("papel desconhecido: {role}"));
    }
    let machine_id = audioroute_wasapi::machine_id().map_err(|e| e.to_string())?;
    let backend = WasapiBackend::new(IdGenerator::new(machine_id.clone()));
    let machine_id_for_ids = machine_id.clone();
    if role == "controller" {
        let hub = NetHub::start(computer_name(), machine_id, HubOptions::default())
            .map_err(|e| format!("não foi possível abrir a rede: {e}"))?;
        let path = data_dir()?.join("config.json");
        let virt = VirtualBackend::new(
            backend.clone(),
            &IdGenerator::new(machine_id_for_ids.clone()),
            Some(data_dir()?.join("virtual.json")),
        );
        // Mostra os pares em uso no Windows e esconde os livres.
        virt.apply_windows();
        let net = NetBackend { local: virt.clone(), hub: hub.clone() };
        inner.virt = Some(virt);
        match Service::open(net, path) {
            Ok(service) => inner.service = Some(service),
            Err(e) => {
                hub.shutdown();
                return Err(e.to_string());
            }
        }
        inner.hub = Some(hub);
    } else {
        let opts = ParticipantOptions { controller: saved_controller(), ..Default::default() };
        inner.participant = Some(Participant::start(backend.clone(), machine_id, computer_name(), opts));
    }
    inner.backend = Some(backend);
    inner.role = Some(role.to_owned());
    Ok(())
}

/// Muda os pares virtuais, aplica no Windows e recarrega os dispositivos.
/// Devolve os avisos do Windows (ex.: sem permissão para renomear).
fn virtual_change(st: &AppState, change: impl FnOnce(&VirtualBackend) -> Result<(), String>) -> CmdResult {
    let mut inner = st.lock();
    let virt = inner.virt.clone().ok_or("os dispositivos virtuais só existem no controlador principal")?;
    change(&virt)?;
    let warnings = virt.apply_windows();
    if let Some(s) = inner.service.as_mut() {
        s.refresh().map_err(|e| e.to_string())?;
    }
    Ok(json!({ "warnings": warnings }))
}

/// Faz o Windows tocar tudo (vídeos, jogos, notificações) na saída `id`.
/// É o cabo fixo que sai do nó "Windows" no canvas.
fn set_windows_output(st: &AppState, id: &str) -> Result<(), String> {
    let mut inner = st.lock();
    let backend = inner.backend.clone().ok_or("escolha um papel na tela inicial primeiro")?;
    let inner_virt = inner.virt.clone();
    let service = service(&mut inner)?;
    let device = service
        .devices()
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("dispositivo {id} não encontrado"))?;
    if device.node.is_some() {
        return Err("o som do Windows só pode ir para uma saída deste computador".into());
    }
    if device.mode == "exclusive" {
        return Err(format!(
            "\"{}\" está em modo exclusivo no audioroute: o Windows não conseguiria tocar nele",
            device.name
        ));
    }
    let mut uuid = Uuid::parse_str(id).map_err(|e| e.to_string())?;
    // Cabo do Windows até a entrada "Alto-falantes (audioroute)": o padrão do
    // Windows vira o alto-falante virtual, e o som dos programas entra no canvas.
    if let Some(speaker) = inner_virt.as_ref().and_then(|v| v.windows_speaker(uuid)) {
        uuid = speaker;
    }
    backend.set_default_output(uuid).map_err(|e| e.to_string())?;
    service.refresh().map_err(|e| e.to_string())
}

// ----------------------------------------------------------------- eventos

/// Thread que envia à interface os níveis (~20×/s) e, a cada segundo, faz a
/// manutenção do serviço (dispositivos que caíram, cabos esperando) e envia
/// as estatísticas.
fn spawn_events(state: AppState) {
    std::thread::Builder::new()
        .name("ui-events".into())
        .spawn(move || {
            // Medidor do sistema: COM, então vive só nesta thread.
            let mut meter = SystemMeter::new().ok();
            let mut endpoints: Vec<(Uuid, String)> = Vec::new();
            let mut last_snapshot: Vec<(Uuid, bool)> = Vec::new();
            let (mut last_tick, mut last_endpoints) = (Instant::now(), Instant::now() - ENDPOINTS_INTERVAL);

            loop {
                std::thread::sleep(LEVELS_INTERVAL);

                let backend = state.lock().backend.clone();
                let Some(backend) = backend else {
                    endpoints.clear();
                    last_snapshot.clear();
                    continue;
                };
                if last_endpoints.elapsed() >= ENDPOINTS_INTERVAL {
                    last_endpoints = Instant::now();
                    if let Ok(list) = backend.endpoints() {
                        // Dispositivo plugado/desplugado ou padrão trocado fora do
                        // app (configurações de som): a interface precisa saber.
                        let snapshot: Vec<(Uuid, bool)> = list.iter().map(|e| (e.info.id, e.info.is_default)).collect();
                        if !last_snapshot.is_empty() && snapshot != last_snapshot {
                            if let Some(s) = state.lock().service.as_mut() {
                                let _ = s.refresh();
                            }
                            emit("changed", ());
                        }
                        last_snapshot = snapshot;
                        endpoints = list.into_iter().map(|e| (e.info.id, e.endpoint_id)).collect();
                    }
                }

                // Participante entrou, saiu ou mudou de dispositivos.
                let net_changed = {
                    let mut inner = state.lock();
                    let changed = inner.hub.as_ref().is_some_and(NetHub::take_changed);
                    if changed && let Some(s) = inner.service.as_mut() {
                        let _ = s.refresh();
                    }
                    changed
                };
                if net_changed {
                    emit("changed", ());
                }

                let visible = ui::is_visible();
                let tick = last_tick.elapsed() >= TICK_INTERVAL;
                if !visible && !tick {
                    continue; // escondida na bandeja: sem medidores
                }

                // Nível do Windows (inclui outros programas) e do motor; vale o maior.
                let mut levels: HashMap<String, f32> = HashMap::new();
                if visible && let Some(m) = meter.as_mut() {
                    let peaks = m.peaks(endpoints.iter().map(|(id, e)| (*id, e.as_str())));
                    levels.extend(peaks.into_iter().map(|(id, p)| (id.to_string(), p)));
                }
                let (engine_levels, stats, changed) = {
                    let mut inner = state.lock();
                    match inner.service.as_mut() {
                        Some(s) if tick => {
                            let changed = s.tick();
                            (s.levels(), Some(s.stats()), changed)
                        }
                        Some(s) => (s.levels(), None, false),
                        None => (HashMap::new(), None, false),
                    }
                };
                if visible {
                    for (id, peak) in engine_levels {
                        let e = levels.entry(id.to_string()).or_default();
                        *e = e.max(peak);
                    }
                    emit("levels", &levels);
                }

                if tick {
                    last_tick = Instant::now();
                    let network = state.lock().network();
                    emit("network", network);
                    if let Some(stats) = stats.filter(|_| visible) {
                        emit("stats", &stats);
                    }
                    if changed {
                        // Um cabo ligou ou caiu sozinho: a interface recarrega.
                        emit("changed", ());
                    }
                }
            }
        })
        .expect("criar thread de eventos");
}
