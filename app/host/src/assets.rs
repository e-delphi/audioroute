//! Interface embutida no exe (ver `build.rs`), servida ao WebView2 num
//! endereço `https://audioroute.app/` que só existe dentro do app.
//!
//! Desenvolvimento: com `AUDIOROUTE_UI_DIR` apontando para `app/ui`, os
//! arquivos são lidos do disco a cada pedido; basta F5 para ver a mudança.

use std::borrow::Cow;
use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Pasta da interface no disco (modo de desenvolvimento), se configurada.
pub fn dev_dir() -> Option<PathBuf> {
    std::env::var_os("AUDIOROUTE_UI_DIR").map(PathBuf::from).filter(|p| p.join("index.html").is_file())
}

/// Conteúdo e tipo MIME de um caminho como `js/graph.js`.
/// Caminho vazio é a página inicial.
pub fn get(path: &str) -> Option<(Cow<'static, [u8]>, &'static str)> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if path.split('/').any(|part| part == "..") {
        return None;
    }
    let data = match dev_dir() {
        Some(dir) => Cow::Owned(std::fs::read(dir.join(path)).ok()?),
        None => Cow::Borrowed(ASSETS.iter().find(|(p, _)| *p == path)?.1),
    };
    Some((data, mime(path)))
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
