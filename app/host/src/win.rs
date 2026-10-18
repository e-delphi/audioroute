//! Pequenos auxiliares Win32.

use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{HICON, IDI_APPLICATION, LoadIconW};
use windows::core::w;

/// `&str` → UTF-16 terminado em zero.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).unwrap_or_default() }
}

/// Ícone do app (`MAINICON`, embutido pelo `build.rs`). Ícone de recurso é
/// compartilhado: não precisa de `DestroyIcon`.
pub fn main_icon() -> HICON {
    unsafe {
        LoadIconW(Some(hinstance()), w!("MAINICON"))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}
