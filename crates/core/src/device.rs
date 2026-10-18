//! Tipos neutros de dispositivo, sem dependência de plataforma.

use std::fmt;
use uuid::Uuid;

/// Se o dispositivo produz áudio (entrada) ou consome (saída).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// Microfone, loopback, stream recebido da rede.
    Input,
    /// Alto-falante, fone, stream enviado para a rede.
    Output,
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeviceKind::Input => "input",
            DeviceKind::Output => "output",
        })
    }
}

/// Um dispositivo de áudio disponível neste nó.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    /// GUID determinístico, único na rede inteira (ver [`crate::identity`]).
    pub id: Uuid,
    /// Nome dado pelo sistema operacional.
    pub name: String,
    pub kind: DeviceKind,
    /// Entrada que captura o que toca em uma saída.
    pub loopback: bool,
    /// Dispositivo padrão do sistema.
    pub is_default: bool,
    /// Computador de onde ele vem, se for de outro nó da rede. `None`: este
    /// computador.
    pub node: Option<String>,
}

/// Implementado por qualquer backend capaz de listar dispositivos.
pub trait DeviceLister {
    type Error: std::error::Error + Send + Sync + 'static;

    fn list(&self) -> Result<Vec<DeviceInfo>, Self::Error>;
}
