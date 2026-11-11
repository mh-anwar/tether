//! Payload transfers (files, attachments, icons) run on a separate TLS connection.
//! kdeconnect-kde:core/backends/lan/landevicelink.cpp + uploadjob.cpp: the sender listens on a
//! port in 1739..=1764 and acts as TLS server; the receiver connects and acts as TLS client.

use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::{tls::TlsContext, Error, Result};

pub const MIN_PAYLOAD_PORT: u16 = 1739;
pub const MAX_PAYLOAD_PORT: u16 = 1764;
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);
const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn bind_payload_listener() -> Result<(TcpListener, u16)> {
    for port in MIN_PAYLOAD_PORT..=MAX_PAYLOAD_PORT {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", port)).await {
            return Ok((l, port));
        }
    }
    Err(Error::Protocol("no free payload port".into()))
}

/// Serves one payload to the device whose pinned certificate is `peer_cert`, then closes.
pub async fn serve<R: AsyncRead + Unpin>(
    listener: TcpListener,
    tls: &TlsContext,
    peer_cert: &[u8],
    mut data: R,
) -> Result<u64> {
    let (tcp, _) = timeout(ACCEPT_TIMEOUT, listener.accept()).await??;
    let mut stream = timeout(
        IO_TIMEOUT,
        TlsAcceptor::from(tls.server.clone()).accept(tcp),
    )
    .await??;
    let got = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.to_vec());
    if got.as_deref() != Some(peer_cert) {
        return Err(Error::Protocol(
            "payload peer is not the paired device".into(),
        ));
    }
    let n = copy_with_idle_timeout(&mut data, &mut stream, None).await?;
    stream.shutdown().await.ok();
    Ok(n)
}

/// Downloads a payload of `size` bytes (or until EOF when the size is unknown, i.e. -1).
pub async fn fetch<W: AsyncWrite + Unpin>(
    addr: SocketAddr,
    tls: &TlsContext,
    peer_cert: &[u8],
    size: i64,
    out: &mut W,
) -> Result<u64> {
    let tcp = timeout(IO_TIMEOUT, TcpStream::connect(addr)).await??;
    let mut stream = timeout(
        IO_TIMEOUT,
        TlsConnector::from(tls.client.clone()).connect(TlsContext::server_name(), tcp),
    )
    .await??;
    let got = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.to_vec());
    if got.as_deref() != Some(peer_cert) {
        return Err(Error::Protocol(
            "payload peer is not the paired device".into(),
        ));
    }
    let limit = u64::try_from(size).ok();
    let n = copy_with_idle_timeout(&mut stream, out, limit).await?;
    if let Some(expected) = limit {
        if n != expected {
            return Err(Error::Protocol(format!(
                "payload truncated: {n} of {expected} bytes"
            )));
        }
    }
    out.flush().await?;
    Ok(n)
}

/// Like `tokio::io::copy`, but a transfer that stalls for `IO_TIMEOUT` fails instead of hanging.
async fn copy_with_idle_timeout<R, W>(r: &mut R, w: &mut W, limit: Option<u64>) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let want = match limit {
            Some(l) if total >= l => break,
            Some(l) => buf.len().min((l - total) as usize),
            None => buf.len(),
        };
        let n = match timeout(IO_TIMEOUT, r.read(&mut buf[..want])).await? {
            Ok(n) => n,
            // TLS peers often close without close_notify once the payload is complete.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
            Err(e) => return Err(e.into()),
        };
        if n == 0 {
            break;
        }
        timeout(IO_TIMEOUT, w.write_all(&buf[..n])).await??;
        total += n as u64;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cert::LocalCert, identity::new_device_id};

    #[tokio::test]
    async fn transfer_roundtrip() -> Result<()> {
        let (ca, cb) = (
            LocalCert::generate(&new_device_id())?,
            LocalCert::generate(&new_device_id())?,
        );
        let (ta, tb) = (TlsContext::new(&ca)?, TlsContext::new(&cb)?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let data: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let sent = data.clone();
        let cb_der = cb.cert_der.clone();
        let server = tokio::spawn(async move { serve(listener, &ta, &cb_der, &sent[..]).await });
        let mut out = Vec::new();
        let n = fetch(addr, &tb, &ca.cert_der, data.len() as i64, &mut out).await?;
        server.await.map_err(|e| Error::Protocol(e.to_string()))??;
        assert_eq!(n as usize, data.len());
        assert_eq!(out, data);
        Ok(())
    }
}
