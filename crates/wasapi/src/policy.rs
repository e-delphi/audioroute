//! Troca o dispositivo padrão do Windows (o mesmo que fazer nas
//! configurações de som).
//!
//! Não há API pública para isso: usamos a interface interna `IPolicyConfig`,
//! estável desde o Windows 7 e usada por SoundSwitch, EarTrumpet e NirCmd.

// Os métodos COM mantêm os nomes originais.
#![allow(non_snake_case)]

use windows::Win32::Media::Audio::{ERole, eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

use crate::com::ComGuard;

/// CLSID do PolicyConfigClient.
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Só o último método é usado; os anteriores só ocupam suas posições na
/// vtable e nunca são chamados (por isso estão sem os parâmetros reais).
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    unsafe fn GetMixFormat(&self) -> HRESULT;
    unsafe fn GetDeviceFormat(&self) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self) -> HRESULT;
    unsafe fn SetDeviceFormat(&self) -> HRESULT;
    unsafe fn GetProcessingPeriod(&self) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self) -> HRESULT;
    unsafe fn GetShareMode(&self) -> HRESULT;
    unsafe fn SetShareMode(&self) -> HRESULT;
    unsafe fn GetPropertyValue(&self) -> HRESULT;
    unsafe fn SetPropertyValue(&self) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
}

/// Torna o endpoint o padrão do Windows para todos os papéis (jogos e
/// vídeos, música e chamadas).
pub fn set_default_endpoint(endpoint_id: &str) -> windows::core::Result<()> {
    let _com = ComGuard::new()?;
    let id = HSTRING::from(endpoint_id);
    unsafe {
        let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
        for role in [eConsole, eMultimedia, eCommunications] {
            policy.SetDefaultEndpoint(PCWSTR(id.as_ptr()), role).ok()?;
        }
    }
    Ok(())
}
