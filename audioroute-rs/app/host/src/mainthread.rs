//! Fila de trabalho para a thread da janela.
//!
//! O WebView2 só pode ser usado na thread que o criou, e o loop é Win32 puro
//! (`GetMessage`/`DispatchMessage`): `queue` guarda o closure e posta
//! `WM_APP_WAKE` na janela; o `window_proc` chama `drain`.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Mensagem privada que acorda o `GetMessage`.
pub const WM_APP_WAKE: u32 = WM_APP + 1;

type Job = Box<dyn FnOnce() + Send + 'static>;

static QUEUE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
static WAKE_HWND: AtomicIsize = AtomicIsize::new(0);

/// Registra a janela que recebe o `WM_APP_WAKE`.
pub fn set_wake_window(hwnd: HWND) {
    WAKE_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
}

/// Enfileira `job` para rodar na thread da janela. Seguro de qualquer thread.
pub fn queue(job: impl FnOnce() + Send + 'static) {
    QUEUE.lock().unwrap_or_else(|p| p.into_inner()).push_back(Box::new(job));
    let hwnd = WAKE_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_WAKE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Roda tudo o que está pendente. Só o `window_proc` chama.
pub fn drain() {
    loop {
        // Solta o lock antes de rodar: o job pode enfileirar outro.
        let job = QUEUE.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
        let Some(job) = job else { break };
        // Uma tarefa que falha não derruba o app nem as seguintes.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
    }
}
