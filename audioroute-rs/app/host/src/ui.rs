//! Janela Win32 + WebView2 criadas à mão (sem Tauri/wry).
//!
//! * a interface vem de dentro do exe ([`crate::assets`]), servida pelo
//!   `WebResourceRequested` em `https://audioroute.app/`;
//! * com `AUDIOROUTE_UI_DIR` (pasta `app/ui`), lê a interface do disco e
//!   libera DevTools e F5 (ver [`crate::assets`]);
//! * JS → Rust: `chrome.webview.postMessage` → [`crate::bridge::dispatch`];
//!   Rust → JS: [`post_json`] → `PostWebMessageAsJson`;
//! * o [X] esconde para a bandeja; "Sair" no menu da bandeja fecha de verdade.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    WebMessageReceivedEventHandler, WebResourceRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{COLORREF, CloseHandle, E_FAIL, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HBRUSH, HGDIOBJ, PAINTSTRUCT, UpdateWindow};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::{SHCreateMemStream, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, PWSTR, w};

use crate::{assets, bridge, mainthread, tray, win};

const TITLE: &str = "audioroute";
const CLASS_NAME: &str = "audioroute.Main";
const MUTEX_NAME: &str = "audioroute.SingleInstance";
/// Mensagem que uma 2ª instância manda para a 1ª aparecer.
const SHOW_MSG_NAME: &str = "audioroute.ShowInstance";
/// Endereço da interface embutida. `https` dá contexto seguro ao JS.
const UI_ORIGIN: &str = "https://audioroute.app";

/// Fundo enquanto a interface não carrega (o `--bg` do CSS, #111318, em BGR).
const BACKGROUND: u32 = 0x0018_1311;

const MIN_WIDTH: i32 = 800;
const MIN_HEIGHT: i32 = 500;

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static WM_SHOW_INSTANCE: AtomicU32 = AtomicU32::new(0);
static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// "Sair" do menu da bandeja: o `WM_CLOSE` fecha em vez de esconder.
static REAL_QUIT: AtomicBool = AtomicBool::new(false);
/// O aviso "continua na bandeja" aparece só na 1ª vez.
static TRAY_HINT_SHOWN: AtomicBool = AtomicBool::new(false);
static VISIBLE: AtomicBool = AtomicBool::new(true);

struct Web {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

thread_local! {
    // O WebView2 vive e morre na thread da janela (STA).
    static WEB: RefCell<Option<Web>> = const { RefCell::new(None) };
}

fn main_window() -> HWND {
    HWND(MAIN_HWND.load(Ordering::SeqCst) as *mut _)
}

// ------------------------------------------------------------- API pública

/// Janela visível (não escondida na bandeja nem minimizada). Os medidores só
/// são enviados quando alguém pode vê-los.
pub fn is_visible() -> bool {
    VISIBLE.load(Ordering::SeqCst)
}

/// Envia um JSON à interface. Qualquer thread: vai pela fila da janela.
pub fn post_json(json: String) {
    mainthread::queue(move || {
        WEB.with(|w| {
            if let Some(web) = w.borrow().as_ref() {
                unsafe {
                    let _ = web.webview.PostWebMessageAsJson(&HSTRING::from(json));
                }
            }
        })
    });
}

/// Traz a janela de volta (da bandeja ou minimizada) para a frente.
pub fn show_window() {
    let hwnd = main_window();
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        // Anti-roubo de foco: vindo da bandeja, o processo em primeiro plano é
        // o Explorer e o Windows recusaria o SetForegroundWindow. Compartilhar
        // a entrada com a thread em primeiro plano por um instante libera.
        let fg = GetForegroundWindow();
        let me = GetCurrentThreadId();
        let mut attached = None;
        if !fg.0.is_null() && fg != hwnd {
            let other = GetWindowThreadProcessId(fg, None);
            if other != 0 && other != me && AttachThreadInput(me, other, true).as_bool() {
                attached = Some(other);
            }
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if let Some(t) = attached {
            let _ = AttachThreadInput(me, t, false);
        }
    }
    VISIBLE.store(true, Ordering::SeqCst);
}

/// Fecha o app de verdade (o áudio para).
pub fn quit() {
    REAL_QUIT.store(true, Ordering::SeqCst);
    let hwnd = main_window();
    if !hwnd.0.is_null() {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

fn hide_to_tray(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    VISIBLE.store(false, Ordering::SeqCst);
    if !TRAY_HINT_SHOWN.swap(true, Ordering::SeqCst) {
        tray::show_balloon(
            hwnd,
            TITLE,
            "Continua roteando o áudio aqui na bandeja. Para fechar, clique com o botão direito e escolha Sair.",
        );
    }
}

fn shell_open(url: &str) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn set_dark_title_bar(hwnd: HWND) {
    let on = BOOL::from(true);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const BOOL as *const _, size_of::<BOOL>() as u32);
    }
}

// ----------------------------------------------------------------- WebView2

/// Dados do WebView2 em `%LOCALAPPDATA%\audioroute`: sem isto ele criaria uma
/// pasta `audioroute.exe.WebView2` ao lado do exe.
fn user_data_folder() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("audioroute").join("WebView2");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn resize_webview(hwnd: HWND) {
    WEB.with(|w| {
        if let Some(web) = w.borrow().as_ref() {
            let mut rc = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rc);
                let _ = web.controller.SetBounds(rc);
            }
        }
    });
}

/// Responde os pedidos a `https://audioroute.app/...` com os arquivos embutidos.
fn serve_asset(env: &ICoreWebView2Environment, args: &ICoreWebView2WebResourceRequestedEventArgs) -> windows::core::Result<()> {
    unsafe {
        let request = args.Request()?;
        let mut p = PWSTR::null();
        request.Uri(&mut p)?;
        let uri = take_pwstr(p);
        let path = uri.strip_prefix(UI_ORIGIN).unwrap_or("/");
        let path = path.split(['?', '#']).next().unwrap_or("/");
        let response = match assets::get(path) {
            Some((data, mime)) => {
                let stream = SHCreateMemStream(Some(&data));
                env.CreateWebResourceResponse(
                    stream.as_ref(),
                    200,
                    w!("OK"),
                    &HSTRING::from(format!("Content-Type: {mime}\r\nCache-Control: no-cache")),
                )?
            }
            None => env.CreateWebResourceResponse(None, 404, w!("Not Found"), w!(""))?,
        };
        args.SetResponse(&response)
    }
}

fn on_controller_ready(hwnd: HWND, env: ICoreWebView2Environment, controller: ICoreWebView2Controller) -> windows::core::Result<()> {
    let webview = unsafe { controller.CoreWebView2()? };
    let dev = assets::dev_dir().is_some();

    unsafe {
        // Sem branco piscando antes de a página pintar.
        if let Ok(c2) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = c2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR { A: 255, R: 0x11, G: 0x13, B: 0x18 });
        }
        // Sem coisas de navegador que não fazem sentido num app (DevTools só
        // em desenvolvimento).
        if let Ok(settings) = webview.Settings() {
            let devtools = cfg!(debug_assertions) || dev;
            let _ = settings.SetAreDevToolsEnabled(devtools);
            let _ = settings.SetAreDefaultContextMenusEnabled(devtools);
            let _ = settings.SetIsStatusBarEnabled(false);
            if let Ok(s3) = settings.cast::<ICoreWebView2Settings3>() {
                let _ = s3.SetAreBrowserAcceleratorKeysEnabled(devtools);
            }
        }

        let mut token = Default::default();

        let h = WebMessageReceivedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut p = PWSTR::null();
                if args.TryGetWebMessageAsString(&mut p).is_ok() {
                    bridge::dispatch(&take_pwstr(p));
                }
            }
            Ok(())
        }));
        webview.add_WebMessageReceived(&h, &mut token)?;

        webview.AddWebResourceRequestedFilter(&HSTRING::from(format!("{UI_ORIGIN}/*")), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        let h = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                serve_asset(&env, &args)?;
            }
            Ok(())
        }));
        webview.add_WebResourceRequested(&h, &mut token)?;

        // Só a própria interface navega; links externos vão para o navegador.
        let h = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let mut p = PWSTR::null();
                if args.Uri(&mut p).is_ok() {
                    let uri = take_pwstr(p);
                    if !(uri.starts_with(UI_ORIGIN) || uri.starts_with("about:")) {
                        let _ = args.SetCancel(true);
                        if uri.starts_with("http://") || uri.starts_with("https://") {
                            shell_open(&uri);
                        }
                    }
                }
            }
            Ok(())
        }));
        webview.add_NavigationStarting(&h, &mut token)?;

        let h = NewWindowRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let _ = args.SetHandled(true);
                let mut p = PWSTR::null();
                if args.Uri(&mut p).is_ok() {
                    let uri = take_pwstr(p);
                    if uri.starts_with("http://") || uri.starts_with("https://") {
                        shell_open(&uri);
                    }
                }
            }
            Ok(())
        }));
        webview.add_NewWindowRequested(&h, &mut token)?;
    }

    WEB.with(|w| *w.borrow_mut() = Some(Web { controller, webview: webview.clone() }));
    resize_webview(hwnd);
    unsafe { webview.Navigate(&HSTRING::from(format!("{UI_ORIGIN}/index.html"))) }
}

fn create_webview(hwnd: HWND) {
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |err, env| {
        err?;
        let env = env.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        let env2 = env.clone();
        let done = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |err, controller| {
            err?;
            let controller = controller.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            on_controller_ready(hwnd, env2.clone(), controller)
        }));
        unsafe { env.CreateCoreWebView2Controller(hwnd, &done) }
    }));

    let folder = user_data_folder().map(|p| HSTRING::from(p.as_os_str())).unwrap_or_default();
    let result = unsafe { CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &folder, &options, &handler) };
    if let Err(e) = result {
        let text = format!(
            "Não foi possível iniciar o WebView2 ({e}).\n\nInstale o \"Microsoft Edge WebView2 Runtime\" e abra o audioroute de novo."
        );
        unsafe {
            MessageBoxW(Some(hwnd), &HSTRING::from(text), &HSTRING::from(TITLE), MB_OK | MB_ICONERROR);
        }
    }
}

// ------------------------------------------------------------------- janela

fn paint_background(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let brush = CreateSolidBrush(COLORREF(BACKGROUND));
        FillRect(dc, &ps.rcPaint, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = EndPaint(hwnd, &ps);
    }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 2ª instância pedindo para esta aparecer.
    if msg != 0 && msg == WM_SHOW_INSTANCE.load(Ordering::SeqCst) {
        show_window();
        return LRESULT(0);
    }
    // O Explorer reiniciou e apagou os ícones da bandeja.
    if msg != 0 && msg == WM_TASKBAR_CREATED.load(Ordering::SeqCst) {
        tray::install(hwnd, TITLE);
        return LRESULT(0);
    }

    match msg {
        WM_CREATE => {
            MAIN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
            mainthread::set_wake_window(hwnd);
            set_dark_title_bar(hwnd);
            create_webview(hwnd);
            LRESULT(0)
        }
        mainthread::WM_APP_WAKE => {
            mainthread::drain();
            LRESULT(0)
        }
        WM_PAINT => {
            paint_background(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            resize_webview(hwnd);
            VISIBLE.store(wparam.0 as u32 != SIZE_MINIMIZED, Ordering::SeqCst);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
            info.ptMinTrackSize.x = MIN_WIDTH * dpi / 96;
            info.ptMinTrackSize.y = MIN_HEIGHT * dpi / 96;
            LRESULT(0)
        }
        tray::WM_TRAYICON => {
            tray::handle_message(hwnd, lparam);
            LRESULT(0)
        }
        WM_COMMAND if (wparam.0 & 0xFFFF) as u16 >= tray::COMMAND_BASE => {
            tray::handle_command((wparam.0 & 0xFFFF) as u16);
            LRESULT(0)
        }
        WM_CLOSE if !REAL_QUIT.load(Ordering::SeqCst) => {
            // [X] e Alt+F4: o áudio continua; a janela vai para a bandeja.
            hide_to_tray(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            tray::remove(hwnd);
            WEB.with(|w| {
                if let Some(web) = w.borrow_mut().take() {
                    unsafe {
                        let _ = web.controller.Close();
                    }
                }
            });
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Abre a janela e roda o loop até o "Sair".
pub fn run() {
    unsafe {
        // Instância única: a 2ª acorda a 1ª e sai.
        let show_msg = RegisterWindowMessageW(&HSTRING::from(SHOW_MSG_NAME));
        WM_SHOW_INSTANCE.store(show_msg, Ordering::SeqCst);
        let mutex = CreateMutexW(None, false, &HSTRING::from(MUTEX_NAME));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(&HSTRING::from(CLASS_NAME), PCWSTR::null()) {
                let _ = PostMessageW(Some(existing), show_msg, WPARAM(0), LPARAM(0));
            }
            if let Ok(m) = mutex {
                let _ = CloseHandle(m);
            }
            return;
        }
        WM_TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::SeqCst);

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinst = win::hinstance();
        let class_name = HSTRING::from(CLASS_NAME);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: hinst,
            hIcon: win::main_icon(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(CreateSolidBrush(COLORREF(BACKGROUND)).0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        // 1280×800 (limitado a 90% da área útil), centralizada.
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let (ww, wh) = (work.right - work.left, work.bottom - work.top);
        let (w, h) = (1280.min(ww * 9 / 10), 800.min(wh * 9 / 10));
        let (x, y) = (work.left + (ww - w) / 2, work.top + (wh - h) / 2);

        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            &class_name,
            &HSTRING::from(TITLE),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            return;
        };
        tray::install(hwnd, TITLE);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        bridge::shutdown();
        CoUninitialize();
        if let Ok(m) = mutex {
            let _ = CloseHandle(m);
        }
    }
}
