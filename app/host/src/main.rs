//! App desktop do audioroute: o motor de áudio em Rust e a interface Vue num
//! WebView2, na mesma janela Win32 criada à mão (sem Tauri).
//!
//! * [`ui`] — janela, WebView2 e o loop de mensagens;
//! * [`tray`] — ícone na bandeja (o [X] esconde, o áudio continua);
//! * [`bridge`] — comandos e eventos trocados com a interface;
//! * [`firewall`] — verifica e libera o audioroute no Firewall do Windows;
//! * [`assets`] — a interface compilada, embutida no exe.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod bridge;
mod firewall;
mod mainthread;
mod tray;
mod ui;
mod win;

fn main() {
    bridge::start();
    ui::run();
}
