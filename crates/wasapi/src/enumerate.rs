//! Enumeração dos dispositivos de áudio via WASAPI (IMMDeviceEnumerator).
//!
//! Segue exatamente a lógica do miniaudio usado na versão Go (mesma ordem,
//! mesmo ID nativo, mesmo critério de padrão), para os GUIDs serem idênticos.

use audioroute_core::audio::{CaptureCallback, RenderCallback, StreamError};
use audioroute_core::{DeviceInfo, DeviceKind, DeviceLister, IdGenerator, Role, Stream, StreamOpener, StreamRequest};
use uuid::Uuid;

use crate::probe::{self, Probe};
use crate::stream;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, EDataFlow, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
    eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::PWSTR;

use crate::com::ComGuard;

/// O miniaudio guarda o ID em `wchar_t[64]` e ignora dispositivos com ID
/// maior; fazemos o mesmo para as listas baterem.
const MAX_ID_CHARS: usize = 63;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("WASAPI: {0}")]
    Windows(#[from] windows::core::Error),
}

/// Um dispositivo com o ID do endpoint do Windows, necessário para abri-lo.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub info: DeviceInfo,
    /// Ex.: `{0.0.1.00000000}.{guid}`. O loopback usa o endpoint da saída.
    pub endpoint_id: String,
}

/// Backend de áudio do Windows. Barato de clonar.
#[derive(Clone)]
pub struct WasapiBackend {
    ids: IdGenerator,
}

impl WasapiBackend {
    pub fn new(ids: IdGenerator) -> Self {
        Self { ids }
    }

    /// Lista entradas, saídas e loopbacks, com os IDs de endpoint.
    pub fn endpoints(&self) -> Result<Vec<Endpoint>, Error> {
        let _com = ComGuard::new()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };

        let capture = list_flow(&enumerator, eCapture)?;
        let render = list_flow(&enumerator, eRender)?;

        let mut out = Vec::with_capacity(capture.len() + 2 * render.len());
        let mut add = |raw: &RawEndpoint, role, kind, loopback: bool| {
            out.push(Endpoint {
                info: DeviceInfo {
                    id: self.ids.device_id(role, &native_id_bytes(&raw.id)),
                    name: raw.name.clone(),
                    kind,
                    loopback,
                    is_default: raw.is_default && !loopback,
                    node: None,
                },
                endpoint_id: raw.id.clone(),
            });
        };
        for raw in &capture {
            add(raw, Role::Input, DeviceKind::Input, false);
        }
        for raw in &render {
            add(raw, Role::Output, DeviceKind::Output, false);
        }
        // Cada saída também pode ser capturada como entrada (o que toca nela).
        for raw in &render {
            add(raw, Role::Loopback, DeviceKind::Input, true);
        }
        Ok(out)
    }
}

impl WasapiBackend {
    /// Endpoint (e se é loopback) de um dispositivo pelo GUID.
    fn find(&self, id: Uuid, kind: DeviceKind) -> Result<Endpoint, StreamError> {
        let endpoint = self
            .endpoints()?
            .into_iter()
            .find(|e| e.info.id == id)
            .ok_or_else(|| format!("dispositivo {id} não encontrado"))?;
        if endpoint.info.kind != kind {
            return Err(format!("dispositivo {id} é {}, esperado {kind}", endpoint.info.kind).into());
        }
        Ok(endpoint)
    }

    /// Torna a saída o dispositivo padrão do Windows: tudo o que os outros
    /// programas tocam passa a sair nela.
    pub fn set_default_output(&self, id: Uuid) -> Result<(), StreamError> {
        let e = self.find(id, DeviceKind::Output)?;
        if e.info.loopback {
            return Err("loopback não pode ser o padrão do Windows".into());
        }
        crate::policy::set_default_endpoint(&e.endpoint_id).map_err(|e| format!("trocar o padrão do Windows: {e}"))?;
        Ok(())
    }

    /// O que o dispositivo aceita em cada modo (sem abrir stream).
    pub fn probe(&self, endpoint: &Endpoint) -> Probe {
        // O loopback usa o endpoint de saída: a sondagem é a da saída.
        probe::probe(&endpoint.endpoint_id, endpoint.info.kind == DeviceKind::Input && !endpoint.info.loopback)
    }
}

impl StreamOpener for WasapiBackend {
    fn open_capture(
        &self,
        id: Uuid,
        stream: StreamRequest,
        callback: CaptureCallback,
    ) -> Result<Box<dyn Stream>, StreamError> {
        let e = self.find(id, DeviceKind::Input)?;
        let request = stream::Request { endpoint_id: e.endpoint_id, loopback: e.info.loopback, stream };
        Ok(Box::new(stream::open(request, stream::Callback::Capture(callback))?))
    }

    fn open_render(
        &self,
        id: Uuid,
        stream: StreamRequest,
        callback: RenderCallback,
    ) -> Result<Box<dyn Stream>, StreamError> {
        let e = self.find(id, DeviceKind::Output)?;
        let request = stream::Request { endpoint_id: e.endpoint_id, loopback: false, stream };
        Ok(Box::new(stream::open(request, stream::Callback::Render(callback))?))
    }
}

impl DeviceLister for WasapiBackend {
    type Error = Error;

    fn list(&self) -> Result<Vec<DeviceInfo>, Error> {
        Ok(self.endpoints()?.into_iter().map(|e| e.info).collect())
    }
}

struct RawEndpoint {
    id: String,
    name: String,
    is_default: bool,
}

fn list_flow(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> Result<Vec<RawEndpoint>, Error> {
    // Sem dispositivo padrão (ex.: nenhum microfone) não é erro.
    let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }
        .ok()
        .and_then(|d| device_id(&d).ok());

    let collection = unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)? };
    let count = unsafe { collection.GetCount()? };
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let Ok(device) = (unsafe { collection.Item(i) }) else { continue };
        let Ok(id) = device_id(&device) else { continue };
        if id.encode_utf16().count() > MAX_ID_CHARS {
            continue;
        }
        out.push(RawEndpoint {
            name: friendly_name(&device).unwrap_or_default(),
            is_default: default_id.as_deref() == Some(id.as_str()),
            id,
        });
    }
    Ok(out)
}

/// ID do endpoint. A string é alocada pelo COM e liberada aqui.
fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
    unsafe {
        let p: PWSTR = device.GetId()?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map_err(|_| windows::core::Error::empty())
    }
}

/// Nome amigável (ex.: "Fones de ouvido (2- GK71)").
fn friendly_name(device: &IMMDevice) -> windows::core::Result<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName)?;
        let text = PropVariantToStringAlloc(&value);
        let _ = PropVariantClear(&mut value);
        let text = text?;
        let s = text.to_string();
        CoTaskMemFree(Some(text.0 as *const _));
        s.map_err(|_| windows::core::Error::empty())
    }
}

/// Bytes UTF-16LE do ID sem os zeros finais: é o que o Go faz com o
/// `ma_device_id` do miniaudio (`bytes.TrimRight(id, "\x00")`).
fn native_id_bytes(id: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = id.encode_utf16().flat_map(u16::to_le_bytes).collect();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_id_trims_trailing_zeros_like_go() {
        // '}' em UTF-16LE é 7D 00: o 00 final também sai, como no Go.
        assert_eq!(native_id_bytes("a}"), vec![0x61, 0x00, 0x7D]);
    }
}
