//! Firewall do Windows: descobre se ele está barrando o audioroute na rede
//! atual e, se o usuário pedir, libera (com a confirmação de administrador).
//!
//! A leitura não precisa de administrador. O que importa é a entrada: o
//! controlador recebe a conexão TCP e o áudio dos participantes; o
//! participante recebe a descoberta e o áudio do controlador.

use serde::Serialize;
use windows::Win32::Foundation::VARIANT_FALSE;
use windows::Win32::NetworkManagement::WindowsFirewall::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::System::Variant::{VARIANT, VT_UNKNOWN, VariantChangeType};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, IUnknown, Interface, w};

/// Situação do firewall para este exe.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallStatus {
    /// O audioroute consegue receber conexões na rede atual.
    pub ok: bool,
    /// Tipo da rede atual: "privada", "pública" ou "domínio".
    pub network: String,
    /// O que está barrando (quando `ok` é falso).
    pub problem: Option<String>,
}

/// COM só durante a chamada (pode rodar em qualquer thread).
struct Com;
impl Com {
    fn init() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        Com
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn profile_name(p: i32) -> &'static str {
    match p {
        x if x == NET_FW_PROFILE2_PRIVATE.0 => "privada",
        x if x == NET_FW_PROFILE2_PUBLIC.0 => "pública",
        _ => "domínio",
    }
}

/// Uma regra de entrada do firewall que vale para este exe.
struct Rule {
    allow: bool,
    profiles: i32,
}

fn exe_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Regras de entrada ativas para o programa `exe`.
fn rules_for(policy: &INetFwPolicy2, exe: &str) -> windows::core::Result<Vec<Rule>> {
    let exe = exe.to_lowercase();
    let mut out = Vec::new();
    unsafe {
        let enumerator: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
        loop {
            let mut item = [VARIANT::default()];
            let mut fetched = 0u32;
            if enumerator.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                break;
            }
            // Cada item vem como IDispatch; passa para IUnknown e daí para a regra.
            let mut unknown = VARIANT::default();
            if VariantChangeType(&mut unknown, &item[0], Default::default(), VT_UNKNOWN).is_err() {
                continue;
            }
            let Ok(rule) = IUnknown::try_from(&unknown).and_then(|u| u.cast::<INetFwRule>()) else { continue };
            let app = rule.ApplicationName().map(|b| b.to_string().to_lowercase()).unwrap_or_default();
            if app != exe
                || rule.Direction().ok() != Some(NET_FW_RULE_DIR_IN)
                || rule.Enabled().map(|e| e == VARIANT_FALSE).unwrap_or(true)
            {
                continue;
            }
            out.push(Rule {
                allow: rule.Action().ok() == Some(NET_FW_ACTION_ALLOW),
                profiles: rule.Profiles().unwrap_or(0),
            });
        }
    }
    Ok(out)
}

/// Lê o firewall. Se algo falhar na leitura, considera liberado (melhor não
/// acusar um bloqueio que não se sabe se existe).
pub fn status() -> FirewallStatus {
    let _com = Com::init();
    let read = || -> windows::core::Result<FirewallStatus> {
        let policy: INetFwPolicy2 = unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)? };
        let current = unsafe { policy.CurrentProfileTypes()? };
        let rules = rules_for(&policy, &exe_path())?;
        let mut names = Vec::new();
        let mut problem = None;
        for p in [NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC] {
            if current & p.0 == 0 {
                continue;
            }
            let name = profile_name(p.0);
            names.push(name);
            unsafe {
                if policy.get_FirewallEnabled(p)? == VARIANT_FALSE {
                    continue; // firewall desligado nesta rede
                }
                if policy.get_BlockAllInboundTraffic(p)? != VARIANT_FALSE {
                    problem = Some(format!("o firewall está bloqueando todas as conexões de entrada na rede {name}"));
                    break;
                }
                let here: Vec<&Rule> = rules.iter().filter(|r| r.profiles & p.0 != 0).collect();
                if here.iter().any(|r| !r.allow) {
                    problem = Some(format!("há uma regra do firewall bloqueando o audioroute na rede {name}"));
                    break;
                }
                if !here.iter().any(|r| r.allow) && policy.get_DefaultInboundAction(p)? != NET_FW_ACTION_ALLOW {
                    problem = Some(format!("o audioroute ainda não foi liberado no firewall para a rede {name}"));
                    break;
                }
            }
        }
        Ok(FirewallStatus { ok: problem.is_none(), network: names.join(", "), problem })
    };
    read().unwrap_or(FirewallStatus { ok: true, network: String::new(), problem: None })
}

/// Libera o audioroute no firewall em qualquer tipo de rede: apaga as regras
/// dele (inclusive as de bloqueio criadas quando o aviso do Windows foi
/// cancelado) e cria uma de entrada permitindo. Pede administrador (UAC) e
/// espera terminar. `Err` se o usuário recusar o UAC.
pub fn allow() -> Result<FirewallStatus, String> {
    let exe = exe_path();
    let script = format!(
        "/c netsh advfirewall firewall delete rule name=all program=\"{exe}\" & \
         netsh advfirewall firewall add rule name=\"audioroute\" dir=in action=allow program=\"{exe}\" enable=yes profile=any"
    );
    let params = HSTRING::from(script);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("cmd.exe"),
        lpParameters: windows::core::PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|_| "a liberação foi cancelada".to_string())?;
        if !info.hProcess.is_invalid() {
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 0u32;
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = windows::Win32::Foundation::CloseHandle(info.hProcess);
            if code != 0 {
                return Err(format!("o netsh terminou com erro ({code})"));
            }
        }
    }
    Ok(status())
}
