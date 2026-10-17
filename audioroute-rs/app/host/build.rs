//! 1. Embute o ícone (`icon.ico`) como recurso `MAINICON`.
//! 2. Embute a interface (`app/ui`: HTML, CSS e JS puros, sem build) dentro
//!    do exe: o app é um arquivo só, sem pasta ao lado.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=audioroute.rc");
    println!("cargo:rerun-if-changed=icon.ico");
    embed_resource::compile("audioroute.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("falha ao compilar audioroute.rc (rc.exe do Windows SDK)");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ui = manifest.join("..").join("ui");
    println!("cargo:rerun-if-changed={}", ui.display());
    assert!(ui.join("index.html").is_file(), "falta {}", ui.join("index.html").display());

    let mut files = Vec::new();
    collect(&ui, &mut files);
    files.sort();

    let mut code = String::from("/// Arquivos da interface: (caminho relativo, conteúdo).\npub static ASSETS: &[(&str, &[u8])] = &[\n");
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let rel = file.strip_prefix(&ui).unwrap().to_string_lossy().replace('\\', "/");
        let abs = file.canonicalize().unwrap();
        writeln!(code, "    ({rel:?}, include_bytes!({:?})),", abs.to_string_lossy()).unwrap();
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("assets.rs");
    std::fs::write(out, code).unwrap();
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}
