//! UDP announcements on port 1716.
//! kdeconnect-kde:core/backends/lan/lanlinkprovider.cpp `broadcastToNetwork` / `udpBroadcastReceived`.

use std::net::{Ipv4Addr, SocketAddr};

use tokio::net::UdpSocket;

use crate::{identity::UDP_PORT, Result};

/// Binds the announcement socket exclusively. No SO_REUSEADDR: with it, a KDE Connect or Valent
/// still running on 1716 would silently receive some of the phone's announcements instead of us.
/// Override both discovery and TCP ports for isolated daemon tests.
pub fn port() -> Result<u16> {
    match std::env::var("TETHER_PORT") {
        Ok(v) => v
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| crate::Error::Protocol("invalid TETHER_PORT".into())),
        Err(std::env::VarError::NotPresent) => Ok(UDP_PORT),
        Err(e) => Err(crate::Error::Protocol(e.to_string())),
    }
}

pub fn bind() -> Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_broadcast(true)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port()?)).into())?;
    Ok(UdpSocket::from_std(s.into())?)
}

/// A send-only socket for when 1716 is taken: we can still announce, peers then connect to us.
pub async fn bind_send_only() -> Result<UdpSocket> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    s.set_broadcast(true)?;
    Ok(s)
}

pub fn broadcast_targets(manual: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut v = vec![SocketAddr::from((
        Ipv4Addr::BROADCAST,
        port().unwrap_or(UDP_PORT),
    ))];
    v.extend_from_slice(manual);
    v
}
