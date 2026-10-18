//! Descoberta na rede local.
//!
//! O controlador anuncia a cada segundo por UDP (multicast e broadcast, para
//! funcionar mesmo em roteadores que bloqueiam um dos dois). O participante
//! escuta na [`DISCOVERY_PORT`] e conecta no endereço de onde o anúncio veio.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::message::{APP, Announce, DISCOVERY_GROUP, DISCOVERY_PORT, PROTOCOL};

const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(1);

/// Anuncia o controlador até ser solto (drop).
pub struct Announcer {
    stop: Arc<AtomicBool>,
}

impl Announcer {
    pub fn start(machine_id: String, name: String, control_port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        socket.set_broadcast(true)?;
        socket.set_multicast_ttl_v4(1)?; // só a rede local
        let announce = Announce { app: APP.into(), protocol: PROTOCOL, machine_id, name, control_port };
        let payload = serde_json::to_vec(&announce).map_err(io::Error::other)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new().name("net-announce".into()).spawn(move || {
            let group = SocketAddrV4::new(Ipv4Addr::from(DISCOVERY_GROUP), DISCOVERY_PORT);
            let broadcast = SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT);
            while !flag.load(Ordering::SeqCst) {
                let _ = socket.send_to(&payload, group);
                let _ = socket.send_to(&payload, broadcast);
                std::thread::sleep(ANNOUNCE_INTERVAL);
            }
        })?;
        Ok(Self { stop })
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Escuta os anúncios dos controladores.
pub struct Discovery {
    socket: UdpSocket,
}

impl Discovery {
    pub fn listen() -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))?;
        // Sem multicast (ex.: nenhuma rede ativa) ainda dá para receber broadcast.
        let _ = socket.join_multicast_v4(&Ipv4Addr::from(DISCOVERY_GROUP), &Ipv4Addr::UNSPECIFIED);
        Ok(Self { socket })
    }

    /// Próximo anúncio válido, esperando no máximo `timeout`. Devolve o
    /// anúncio e o endereço de controle (IP de origem + porta anunciada).
    pub fn next(&self, timeout: Duration) -> Option<(Announce, SocketAddr)> {
        self.socket.set_read_timeout(Some(timeout)).ok()?;
        let mut buf = [0u8; 1500];
        let (n, from) = self.socket.recv_from(&mut buf).ok()?;
        let a: Announce = serde_json::from_slice(&buf[..n]).ok()?;
        (a.app == APP && a.protocol == PROTOCOL).then(|| {
            let addr = SocketAddr::new(from.ip(), a.control_port);
            (a, addr)
        })
    }
}
