//! Establishing an authenticated link over TCP + TLS.
//!
//! kdeconnect-kde:core/backends/lan/lanlinkprovider.cpp: the side that *opens* the TCP
//! connection (in response to a UDP announcement) sends its identity in plain text and then acts
//! as the TLS **server**; the side that accepted the connection reads that identity and acts as
//! the TLS **client**. With protocol v8 both sides then send their identity again inside TLS,
//! and only that copy is trusted.

use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::{
    cert,
    identity::{Identity, PROTOCOL_VERSION},
    packet::{Packet, MAX_PACKET_LEN},
    tls::TlsContext,
    Error, Result,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_IDENTITY_LEN: usize = 64 * 1024;

pub struct Link {
    pub peer: Identity,
    pub peer_cert: Vec<u8>,
    pub addr: SocketAddr,
    /// Buffered so bytes that arrived right after the identity aren't lost.
    pub stream: BufReader<TlsStream<TcpStream>>,
}

/// We opened the connection (after hearing the peer's UDP announcement).
pub async fn connect(addr: SocketAddr, local: &Identity, tls: &TlsContext) -> Result<Link> {
    let _slot = tls.handshake_slot()?;
    timeout(HANDSHAKE_TIMEOUT, async {
        let mut tcp = TcpStream::connect(addr).await?;
        configure(&tcp);
        tcp.write_all(&local.to_packet().to_line()).await?;
        let stream = TlsAcceptor::from(tls.server.clone()).accept(tcp).await?;
        finish(TlsStream::Server(stream), addr, local, None).await
    })
    .await?
}

/// The peer opened the connection to our TCP listener.
pub async fn accept(
    tcp: TcpStream,
    addr: SocketAddr,
    local: &Identity,
    tls: &TlsContext,
) -> Result<Link> {
    accept_with_slot(tcp, addr, local, tls, tls.handshake_slot()?).await
}

pub(crate) async fn accept_with_slot(
    mut tcp: TcpStream,
    addr: SocketAddr,
    local: &Identity,
    tls: &TlsContext,
    _slot: tokio::sync::OwnedSemaphorePermit,
) -> Result<Link> {
    timeout(HANDSHAKE_TIMEOUT, async {
        configure(&tcp);
        let line = read_plain_line(&mut tcp).await?;
        let announced = Identity::from_packet(&Packet::from_line(&line)?)?;
        let stream = TlsConnector::from(tls.client.clone())
            .connect(TlsContext::server_name(), tcp)
            .await?;
        finish(TlsStream::Client(stream), addr, local, Some(announced)).await
    })
    .await?
}

async fn finish(
    stream: TlsStream<TcpStream>,
    addr: SocketAddr,
    local: &Identity,
    announced: Option<Identity>,
) -> Result<Link> {
    let peer_cert = {
        let (_, conn) = stream.get_ref();
        conn.peer_certificates()
            .and_then(|c| c.first())
            .map(|c| c.to_vec())
            .ok_or_else(|| Error::Protocol("peer sent no certificate".into()))?
    };

    let mut stream = BufReader::new(stream);
    write_packet(&mut stream, &local.to_packet()).await?;
    let line = read_line(&mut stream)
        .await?
        .ok_or_else(|| Error::Protocol("closed before identity".into()))?;
    let peer = Identity::from_packet(&Packet::from_line(&line)?)?;

    if peer.protocol_version < PROTOCOL_VERSION {
        return Err(Error::Protocol(format!(
            "unsupported protocol version {}",
            peer.protocol_version
        )));
    }
    if let Some(a) = announced {
        if a.device_id != peer.device_id {
            return Err(Error::Protocol("device id changed during handshake".into()));
        }
    }
    // The certificate is what gets pinned, so it must belong to the id it claims.
    if cert::common_name(&peer_cert).as_deref() != Some(peer.device_id.as_str()) {
        return Err(Error::Protocol(
            "certificate does not match device id".into(),
        ));
    }
    Ok(Link {
        peer,
        peer_cert,
        addr,
        stream,
    })
}

fn configure(tcp: &TcpStream) {
    let _ = tcp.set_nodelay(true);
    // Keepalive notices a phone that vanished (Wi-Fi off, out of range) without waiting for a write.
    let sock = socket2::SockRef::from(tcp);
    let ka = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(10))
        .with_interval(Duration::from_secs(5));
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let ka = ka.with_retries(3);
    let _ = sock.set_tcp_keepalive(&ka);
}

/// Reads one byte at a time so nothing past the newline (the TLS handshake) is consumed.
async fn read_plain_line(tcp: &mut TcpStream) -> Result<Vec<u8>> {
    let mut line = Vec::with_capacity(512);
    loop {
        let b = tcp.read_u8().await?;
        if b == b'\n' {
            return Ok(line);
        }
        if line.len() >= MAX_IDENTITY_LEN {
            return Err(Error::Protocol("identity too long".into()));
        }
        line.push(b);
    }
}

/// One packet line, or `None` on a clean end of stream. Lines over `MAX_PACKET_LEN` are an error.
pub async fn read_line<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let n = r.take(MAX_PACKET_LEN).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        if n as u64 >= MAX_PACKET_LEN {
            return Err(Error::Protocol("packet too large".into()));
        }
        return Ok(None); // connection closed mid-line
    }
    Ok(Some(buf))
}

pub async fn write_packet<W: AsyncWrite + Unpin>(w: &mut W, p: &Packet) -> Result<()> {
    w.write_all(&p.to_line()).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cert::LocalCert, identity::new_device_id};
    use tokio::net::TcpListener;

    fn ident(id: &str) -> Identity {
        Identity {
            device_id: id.into(),
            device_name: "test".into(),
            device_type: "desktop".into(),
            protocol_version: 8,
            incoming_capabilities: vec![],
            outgoing_capabilities: vec![],
            tcp_port: Some(1716),
        }
    }

    #[tokio::test]
    async fn handshake_both_roles() -> Result<()> {
        let (ida, idb) = (new_device_id(), new_device_id());
        let (ca, cb) = (LocalCert::generate(&ida)?, LocalCert::generate(&idb)?);
        let (ta, tb) = (TlsContext::new(&ca)?, TlsContext::new(&cb)?);
        let (a, b) = (ident(&ida), ident(&idb));

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (tcp, peer) = listener.accept().await?;
            accept(tcp, peer, &b, &tb).await
        });
        let la = connect(addr, &a, &ta).await?;
        let lb = server.await.map_err(|e| Error::Protocol(e.to_string()))??;
        assert_eq!(la.peer.device_id, idb);
        assert_eq!(lb.peer.device_id, ida);
        assert_eq!(lb.peer_cert, ca.cert_der);
        assert_eq!(la.peer_cert, cb.cert_der);
        Ok(())
    }

    #[tokio::test]
    async fn rejects_cert_for_other_id() -> Result<()> {
        let (ida, idb) = (new_device_id(), new_device_id());
        // A's certificate claims a different id than A announces.
        let ca = LocalCert::generate(&new_device_id())?;
        let cb = LocalCert::generate(&idb)?;
        let (ta, tb) = (TlsContext::new(&ca)?, TlsContext::new(&cb)?);
        let (a, b) = (ident(&ida), ident(&idb));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (tcp, peer) = listener.accept().await?;
            accept(tcp, peer, &b, &tb).await
        });
        let _ = connect(addr, &a, &ta).await;
        let res = server.await.map_err(|e| Error::Protocol(e.to_string()))?;
        assert!(res.is_err());
        Ok(())
    }
}
