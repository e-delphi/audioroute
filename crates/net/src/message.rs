//! Mensagens de controle (TCP, uma linha JSON por mensagem) e o anúncio de
//! descoberta (UDP).

use std::io::{self, BufRead, Write};

use audioroute_core::{DeviceInfo, DeviceKind, ShareMode, StreamInfo, StreamRequest};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Porta UDP onde os participantes escutam os anúncios do controlador.
pub const DISCOVERY_PORT: u16 = 47800;
/// Grupo multicast dos anúncios (também vão por broadcast).
pub const DISCOVERY_GROUP: [u8; 4] = [239, 255, 77, 77];
/// Porta TCP preferida do controlador (se ocupada, usa outra e anuncia).
pub const CONTROL_PORT: u16 = 47801;
pub const APP: &str = "audioroute";
pub const PROTOCOL: u32 = 1;

/// Anúncio periódico do controlador.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Announce {
    pub app: String,
    pub protocol: u32,
    pub machine_id: String,
    pub name: String,
    pub control_port: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Input,
    Output,
}

/// Um dispositivo de um participante.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceDesc {
    pub id: Uuid,
    pub name: String,
    pub kind: Kind,
    pub loopback: bool,
}

impl DeviceDesc {
    pub fn from_info(d: &DeviceInfo) -> Self {
        Self {
            id: d.id,
            name: d.name.clone(),
            kind: match d.kind {
                DeviceKind::Input => Kind::Input,
                DeviceKind::Output => Kind::Output,
            },
            loopback: d.loopback,
        }
    }

    /// Como o controlador vê: com o nome do computador e nunca "padrão" (o
    /// padrão é do Windows de lá, não deste).
    pub fn to_info(&self, node: &str) -> DeviceInfo {
        DeviceInfo {
            id: self.id,
            name: self.name.clone(),
            kind: match self.kind {
                Kind::Input => DeviceKind::Input,
                Kind::Output => DeviceKind::Output,
            },
            loopback: self.loopback,
            is_default: false,
            node: Some(node.to_owned()),
        }
    }
}

/// O que o participante conseguiu ao abrir o dispositivo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InfoDesc {
    pub mode: ShareMode,
    pub period_frames: u32,
    pub device_latency_frames: u32,
    pub format: String,
}

impl From<&StreamInfo> for InfoDesc {
    fn from(i: &StreamInfo) -> Self {
        Self {
            mode: i.mode,
            period_frames: i.period_frames,
            device_latency_frames: i.device_latency_frames,
            format: i.native_format.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    /// Participante → controlador, ao conectar.
    #[serde(rename_all = "camelCase")]
    Hello { protocol: u32, machine_id: String, name: String, audio_port: u16, devices: Vec<DeviceDesc> },
    /// Controlador → participante, em resposta ao Hello.
    #[serde(rename_all = "camelCase")]
    Welcome { name: String, audio_port: u16 },
    /// Participante → controlador: a lista de dispositivos mudou.
    Devices { devices: Vec<DeviceDesc> },
    /// Controlador → participante: capture o dispositivo e mande o áudio.
    #[serde(rename_all = "camelCase")]
    StartCapture { stream: u32, device: Uuid, mode: ShareMode, period_frames: Option<u32> },
    /// Controlador → participante: toque no dispositivo o áudio deste stream.
    #[serde(rename_all = "camelCase")]
    StartRender { stream: u32, device: Uuid, mode: ShareMode, period_frames: Option<u32> },
    /// Participante → controlador: o stream abriu.
    Started { stream: u32, info: InfoDesc },
    /// Participante → controlador: o stream não abriu ou parou.
    StreamError { stream: u32, error: String },
    /// Controlador → participante: feche o stream.
    Stop { stream: u32 },
    /// Mantém a conexão viva (os dois lados).
    Ping,
}

impl Msg {
    pub fn start_capture(stream: u32, device: Uuid, r: StreamRequest) -> Self {
        Msg::StartCapture { stream, device, mode: r.mode, period_frames: r.period_frames }
    }

    pub fn start_render(stream: u32, device: Uuid, r: StreamRequest) -> Self {
        Msg::StartRender { stream, device, mode: r.mode, period_frames: r.period_frames }
    }
}

/// Envia uma mensagem (uma linha).
pub fn send(w: &mut impl Write, msg: &Msg) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Lê a próxima mensagem. `Ok(None)`: a conexão fechou. Linhas que não são
/// mensagens conhecidas são ignoradas (versões futuras).
pub fn recv(r: &mut impl BufRead) -> io::Result<Option<Msg>> {
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if let Ok(msg) = serde_json::from_str(line.trim_end()) {
            return Ok(Some(msg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_roundtrip() {
        let msgs = vec![
            Msg::Ping,
            Msg::start_capture(3, Uuid::from_u128(9), StreamRequest { mode: ShareMode::Exclusive, period_frames: Some(240) }),
            Msg::Started { stream: 3, info: InfoDesc { mode: ShareMode::Shared, period_frames: 480, device_latency_frames: 0, format: "x".into() } },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            send(&mut buf, m).unwrap();
        }
        buf.extend_from_slice(b"{\"type\":\"coisa_nova\"}\n");
        let mut r = io::Cursor::new(buf);
        for m in &msgs {
            assert_eq!(recv(&mut r).unwrap().as_ref(), Some(m));
        }
        assert_eq!(recv(&mut r).unwrap(), None);
    }

    #[test]
    fn wire_format_is_readable() {
        let json = serde_json::to_string(&Msg::Stop { stream: 5 }).unwrap();
        assert_eq!(json, r#"{"type":"stop","stream":5}"#);
    }
}
