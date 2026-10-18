//! GUIDs determinísticos dos dispositivos.
//!
//! O mesmo dispositivo, na mesma máquina, sempre recebe o mesmo GUID: um
//! UUID v5 de (ID da máquina, papel, ID nativo). O algoritmo é idêntico ao da
//! versão Go, então as duas geram os mesmos GUIDs e leem o mesmo config.json.

use uuid::Uuid;

/// Namespace próprio do audioroute. NUNCA altere: mudar este valor muda o
/// GUID de todos os dispositivos e invalida os configs existentes.
const NAMESPACE: Uuid = Uuid::from_u128(0x3d6b5f0e_9c1a_4e27_8f43_2a7b1d9e6c55);

/// Papel do dispositivo: entra no hash para que o loopback de uma saída tenha
/// GUID diferente da própria saída (os dois usam o mesmo endpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Input,
    Output,
    Loopback,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Input => "input",
            Role::Output => "output",
            Role::Loopback => "loopback",
        }
    }
}

/// Gera GUIDs de dispositivos para uma máquina.
#[derive(Debug, Clone)]
pub struct IdGenerator {
    machine_id: String,
}

impl IdGenerator {
    pub fn new(machine_id: impl Into<String>) -> Self {
        Self { machine_id: machine_id.into() }
    }

    /// GUID do dispositivo com o papel e o ID nativo informados.
    pub fn device_id(&self, role: Role, native_id: &[u8]) -> Uuid {
        // O byte 0 separa os campos: ("ab","c") e ("a","bc") não colidem.
        let mut data = Vec::with_capacity(self.machine_id.len() + 10 + native_id.len());
        data.extend_from_slice(self.machine_id.as_bytes());
        data.push(0);
        data.extend_from_slice(role.as_str().as_bytes());
        data.push(0);
        data.extend_from_slice(native_id);
        Uuid::new_v5(&NAMESPACE, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let g = IdGenerator::new("machine-1");
        assert_eq!(g.device_id(Role::Input, b"ep"), g.device_id(Role::Input, b"ep"));
    }

    #[test]
    fn differs_by_role_endpoint_and_machine() {
        let g = IdGenerator::new("machine-1");
        let base = g.device_id(Role::Output, b"ep-1");
        assert_ne!(base, g.device_id(Role::Loopback, b"ep-1"));
        assert_ne!(base, g.device_id(Role::Output, b"ep-2"));
        assert_ne!(base, IdGenerator::new("machine-2").device_id(Role::Output, b"ep-1"));
    }

    /// Valor de referência calculado pela versão Go (identity.Generator):
    /// garante que as duas implementações continuam compatíveis.
    #[test]
    fn matches_go_implementation() {
        let g = IdGenerator::new("machine-1");
        let id = g.device_id(Role::Input, b"endpoint-1");
        assert_eq!(id.to_string(), GO_REFERENCE);
    }

    // Saída de identity.NewGenerator("machine-1").DeviceID(RoleInput, "endpoint-1") no Go.
    const GO_REFERENCE: &str = "825cc82a-6842-5888-884a-737286c594c0";
}
