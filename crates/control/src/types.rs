//! Tipos que atravessam a fronteira com a interface (JSON, IDs em texto).

use serde::Serialize;

/// Um dispositivo como a interface o vê.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    /// "input" ou "output" ("unknown" para ausentes sem cabo).
    pub kind: String,
    pub loopback: bool,
    pub is_default: bool,
    /// Presente agora (plugado, ou o computador dele conectado).
    pub available: bool,
    /// Computador da rede de onde ele vem (None: este computador).
    pub node: Option<String>,
    pub description: String,
    pub delay_ms: u32,
    /// Modo pedido: "shared" ou "exclusive".
    pub mode: String,
    /// Período pedido em ms (None = padrão do modo).
    pub period_ms: Option<f64>,
    /// O que está rodando de fato, se o dispositivo estiver aberto.
    pub stream: Option<StreamView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamView {
    pub mode: String,
    pub period_ms: f64,
    pub format: String,
}

/// Um cabo.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub from: String,
    pub to: String,
    pub gain: f64,
    pub mute: bool,
    /// Ligado no motor (os dois lados presentes e abertos).
    pub active: bool,
    /// Por que não está ligado (ex.: dispositivo em uso exclusivo por outro programa).
    pub error: Option<String>,
}

/// Estado de um cabo ligado.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStats {
    pub from: String,
    pub to: String,
    pub level_ms: f64,
    pub target_ms: f64,
    /// Latência estimada de ponta a ponta (driver de entrada + cabo + driver de saída).
    pub latency_ms: f64,
    pub drift_ppm: f64,
    pub underruns: u64,
    pub overruns: u64,
    pub resyncs: u64,
}
