//! Troca o dispositivo padrão do Windows, renomeia e mostra/esconde
//! dispositivos (o mesmo que fazer nas configurações de som).
//!
//! Não há API pública para isso: usamos a interface interna `IPolicyConfig`,
//! estável desde o Windows 7 e usada por SoundSwitch, EarTrumpet e NirCmd.

// Os métodos COM mantêm os nomes originais.
#![allow(non_snake_case)]

use windows::Win32::Devices::Properties::DEVPKEY_Device_DeviceDesc;
use windows::Win32::Media::Audio::{ERole, eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantChangeType, PROPVAR_CHANGE_FLAGS};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{BOOL, GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

use crate::com::ComGuard;

/// CLSID do PolicyConfigClient.
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Os métodos sem parâmetros não são usados: só ocupam suas posições na
/// vtable e nunca são chamados.
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
    unsafe fn SetPropertyValue(&self, device_id: PCWSTR, fx_store: BOOL, key: *const PROPERTYKEY, value: *const PROPVARIANT) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, device_id: PCWSTR, visible: BOOL) -> HRESULT;
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

/// Mostra ou esconde o dispositivo no Windows (como "Desativar" nas
/// configurações de som): escondido, nenhum programa o enxerga.
pub fn set_endpoint_visible(endpoint_id: &str, visible: bool) -> windows::core::Result<()> {
    let _com = ComGuard::new()?;
    let id = HSTRING::from(endpoint_id);
    unsafe {
        let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
        policy.SetEndpointVisibility(PCWSTR(id.as_ptr()), BOOL::from(visible)).ok()
    }
}

/// Troca o nome do dispositivo no Windows (como "Renomear" nas configurações
/// de som). O Windows mostra "<nome> (<adaptador>)".
pub fn set_endpoint_name(endpoint_id: &str, name: &str) -> windows::core::Result<()> {
    let _com = ComGuard::new()?;
    let id = HSTRING::from(endpoint_id);
    let key = PROPERTYKEY { fmtid: DEVPKEY_Device_DeviceDesc.fmtid, pid: DEVPKEY_Device_DeviceDesc.pid };
    unsafe {
        // O nome é guardado como VT_LPWSTR; o From<&str> dá VT_BSTR.
        let bstr = PROPVARIANT::from(name);
        let mut value = PROPVARIANT::default();
        PropVariantChangeType(&mut value, &bstr, PROPVAR_CHANGE_FLAGS(0), VT_LPWSTR)?;
        let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
        policy.SetPropertyValue(PCWSTR(id.as_ptr()), BOOL::from(false), &key, &value).ok()
    }
}
