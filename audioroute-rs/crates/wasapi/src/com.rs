//! Inicialização do COM por thread.

use std::marker::PhantomData;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// Mantém o COM inicializado (modo MTA) na thread atual enquanto existir.
///
/// O COM é por thread: quem cria objetos COM precisa dele inicializado. Se a
/// thread já estava em outro modo (ex.: STA de uma interface gráfica), usa
/// como está e não desinicializa no final.
pub struct ComGuard {
    uninit: bool,
    // *const () não é Send: o guard não pode ir para outra thread.
    _not_send: PhantomData<*const ()>,
}

impl ComGuard {
    pub fn new() -> windows::core::Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self { uninit: false, _not_send: PhantomData });
        }
        hr.ok()?; // S_OK e S_FALSE ("já inicializado") contam como sucesso
        Ok(Self { uninit: true, _not_send: PhantomData })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
        }
    }
}
