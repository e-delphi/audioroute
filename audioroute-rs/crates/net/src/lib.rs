//! Rede do audioroute: os dispositivos de outros computadores da rede local
//! aparecem no controlador como se fossem daqui.
//!
//! * [`discovery`] — o controlador se anuncia por UDP; o participante escuta;
//! * [`message`] — controle por TCP (uma linha JSON por mensagem);
//! * [`packet`] e [`audio`] — áudio por UDP, PCM 16 bits, 5 ms por pacote;
//! * [`NetHub`] — lado do controlador; [`NetBackend`] junta os dispositivos
//!   locais e os da rede num backend só, que o motor usa sem saber a diferença;
//! * [`Participant`] — lado do participante.
//!
//! Sem relógio comum: cada cabo compensa o drift entre os dois relógios pelo
//! nível do buffer, como já faz entre dois dispositivos locais.

pub mod audio;
pub mod discovery;
mod hub;
pub mod message;
pub mod packet;
mod participant;

use std::time::Duration;

use audioroute_core::audio::{CaptureCallback, RenderCallback, SAMPLE_RATE, StreamError};
use audioroute_core::{DeviceInfo, DeviceLister, Stream, StreamOpener, StreamRequest};
use uuid::Uuid;

pub use hub::{HubOptions, NetHub, PeerView};
pub use participant::{Participant, ParticipantOptions, ParticipantStatus};

/// Folga para a variação de chegada dos pacotes (20 ms). Entra no buffer dos
/// cabos que passam pela rede.
pub const NET_ALLOWANCE_FRAMES: u32 = SAMPLE_RATE / 50;
/// Os dois lados mandam um Ping neste intervalo...
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(2);
/// ...e desistem da conexão depois deste silêncio.
pub(crate) const PEER_TIMEOUT: Duration = Duration::from_secs(6);

/// Dispositivos locais + os dos participantes conectados.
#[derive(Clone)]
pub struct NetBackend<B> {
    pub local: B,
    pub hub: NetHub,
}

impl<B: DeviceLister> DeviceLister for NetBackend<B> {
    type Error = B::Error;

    fn list(&self) -> Result<Vec<DeviceInfo>, Self::Error> {
        let mut list = self.local.list()?;
        list.extend(self.hub.devices());
        Ok(list)
    }
}

impl<B: StreamOpener> StreamOpener for NetBackend<B> {
    fn open_capture(&self, id: Uuid, request: StreamRequest, callback: CaptureCallback) -> Result<Box<dyn Stream>, StreamError> {
        if self.hub.owns(id) {
            self.hub.open_capture(id, request, callback)
        } else {
            self.local.open_capture(id, request, callback)
        }
    }

    fn open_render(&self, id: Uuid, request: StreamRequest, callback: RenderCallback) -> Result<Box<dyn Stream>, StreamError> {
        if self.hub.owns(id) {
            self.hub.open_render(id, request, callback)
        } else {
            self.local.open_render(id, request, callback)
        }
    }
}
