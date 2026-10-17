//! Ícone na bandeja do sistema.
//!
//! * clique esquerdo: mostra a janela;
//! * clique direito: menu Abrir / Sair;
//! * o [X] da janela esconde para a bandeja (o áudio continua roteando).
//!
//! Tudo aqui roda na thread da janela.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, SetMenuDefaultItem, TPM_RIGHTBUTTON, TrackPopupMenu, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
    WM_NULL, WM_RBUTTONUP, WM_USER,
};
use windows::core::PCWSTR;

use crate::{ui, win};

pub const WM_TRAYICON: u32 = WM_USER + 100;
/// IDs de comando do menu (o `window_proc` repassa os >= `COMMAND_BASE`).
pub const COMMAND_BASE: u16 = 9000;
const ID_SHOW: u16 = 9001;
const ID_QUIT: u16 = 9002;

static ADDED: AtomicBool = AtomicBool::new(false);

/// `szTip`/`szInfo` são arrays fixos de UTF-16: copia com limite e zero final.
fn copy_wide(dst: &mut [u16], text: &str) {
    let max = dst.len() - 1;
    let mut n = 0;
    for (d, c) in dst.iter_mut().take(max).zip(text.encode_utf16()) {
        *d = c;
        n += 1;
    }
    dst[n] = 0;
}

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

/// Adiciona o ícone. Chamado no início e quando o Explorer reinicia
/// (`TaskbarCreated`), que apaga todos os ícones da bandeja.
pub fn install(hwnd: HWND, tooltip: &str) {
    let mut d = base_data(hwnd);
    d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    d.uCallbackMessage = WM_TRAYICON;
    d.hIcon = win::main_icon();
    copy_wide(&mut d.szTip, tooltip);
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &d) }.as_bool();
    ADDED.store(ok, Ordering::SeqCst);
}

pub fn remove(hwnd: HWND) {
    if ADDED.swap(false, Ordering::SeqCst) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &base_data(hwnd));
        }
    }
}

/// Balão de aviso saindo do ícone.
pub fn show_balloon(hwnd: HWND, title: &str, message: &str) {
    if !ADDED.load(Ordering::SeqCst) {
        return;
    }
    let mut d = base_data(hwnd);
    d.uFlags = NIF_INFO;
    copy_wide(&mut d.szInfoTitle, title);
    copy_wide(&mut d.szInfo, message);
    d.dwInfoFlags = NIIF_INFO;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
    }
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let item = |id: u16, text: &str| {
            let text = win::wide(text);
            let _ = AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(text.as_ptr()));
        };
        item(ID_SHOW, "Abrir audioroute");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        item(ID_QUIT, "Sair");
        let _ = SetMenuDefaultItem(menu, ID_SHOW as u32, 0);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // SetForegroundWindow antes e WM_NULL depois: o truque clássico para o
        // menu fechar quando o usuário clica fora dele.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

/// `WM_TRAYICON` (o `lParam` traz a mensagem do mouse).
pub fn handle_message(hwnd: HWND, lparam: LPARAM) {
    match lparam.0 as u32 {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => ui::show_window(),
        WM_RBUTTONUP => show_menu(hwnd),
        _ => {}
    }
}

/// `WM_COMMAND` de um item do menu.
pub fn handle_command(id: u16) {
    match id {
        ID_SHOW => ui::show_window(),
        ID_QUIT => ui::quit(),
        _ => {}
    }
}
