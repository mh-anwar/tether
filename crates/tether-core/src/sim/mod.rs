//! Scriptable Android protocol peer for integration tests and screenshots.
//! Each connection records packets before scenario handling, including payload bytes.
mod scenario;

use crate::{
    cert::LocalCert,
    identity::{self, PROTOCOL_VERSION},
    link, payload, plugins,
    tls::TlsContext,
    Identity, Packet,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UdpSocket, UnixListener},
    sync::{mpsc, Notify},
    task::JoinHandle,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, Serialize)]
pub struct Received {
    pub packet: Packet,
    pub payload: Vec<u8>,
    pub payload_error: Option<String>,
}

struct State {
    tx: Option<mpsc::Sender<Packet>>,
    task: Option<JoinHandle<()>>,
    received: Vec<Received>,
    address: Option<SocketAddr>,
    peer_cert: Vec<u8>,
}
struct Inner {
    identity: Identity,
    tls: Arc<TlsContext>,
    dir: PathBuf,
    phone: Arc<Mutex<scenario::Phone>>,
    state: Mutex<State>,
    changed: Notify,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .task
            .take()
        {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct Simulator(Arc<Inner>);
impl Simulator {
    pub async fn new(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("device_id");
        let id = std::fs::read_to_string(&path)
            .ok()
            .filter(|s| identity::is_valid_device_id(s.trim()))
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(identity::new_device_id);
        std::fs::write(path, &id)?;
        let cert = LocalCert::load_or_create(&dir, &id)?;
        let identity = Identity {
            device_id: id,
            device_name: "Galaxy S24".into(),
            device_type: "phone".into(),
            protocol_version: PROTOCOL_VERSION,
            incoming_capabilities: plugins::outgoing_capabilities(),
            outgoing_capabilities: plugins::incoming_capabilities(),
            tcp_port: None,
        };
        Ok(Self(Arc::new(Inner {
            identity,
            tls: Arc::new(TlsContext::new(&cert)?),
            dir,
            phone: Arc::new(Mutex::new(scenario::seed())),
            state: Mutex::new(State {
                tx: None,
                task: None,
                received: vec![],
                address: None,
                peer_cert: vec![],
            }),
            changed: Notify::new(),
        })))
    }
    pub fn identity(&self) -> &Identity {
        &self.0.identity
    }
    pub fn connected(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tx
            .as_ref()
            .is_some_and(|tx| !tx.is_closed())
    }
    pub async fn connect(&self, addr: SocketAddr) -> Result<()> {
        self.disconnect();
        let link = link::connect(addr, &self.0.identity, &self.0.tls).await?;
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .address = Some(addr);
        self.attach(link).await;
        Ok(())
    }
    /// Advertise a listener, then accept the daemon's dial (also exercises the opposite TLS role).
    pub async fn discover(&self, target: SocketAddr) -> Result<()> {
        self.discover_using(target, false).await
    }
    /// Exercise DNS-SD discovery without UDP identity announcements.
    pub async fn discover_mdns(&self, target: SocketAddr) -> Result<()> {
        self.discover_using(target, true).await
    }
    async fn discover_using(&self, target: SocketAddr, mdns: bool) -> Result<()> {
        self.disconnect();
        // LAN discovery intentionally rejects ports outside the protocol's 1716..1764 range.
        let mut listener = None;
        for port in 1717..=1738 {
            if let Ok(socket) = TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(socket);
                break;
            }
        }
        let listener = listener.ok_or("no free simulator discovery port")?;
        let mut identity = self.0.identity.clone();
        identity.tcp_port = Some(listener.local_addr()?.port());
        let udp = UdpSocket::bind("0.0.0.0:0").await?;
        udp.set_broadcast(true)?;
        let announcement = if mdns {
            crate::mdns::announcement(&identity, std::net::Ipv4Addr::LOCALHOST)
        } else {
            identity.to_packet().to_line()
        };
        udp.send_to(&announcement, target).await?;
        let (tcp, addr) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await??;
        self.attach(link::accept(tcp, addr, &identity, &self.0.tls).await?)
            .await;
        Ok(())
    }
    async fn attach(&self, l: link::Link) {
        {
            let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            state.peer_cert = l.peer_cert.clone();
            if let Some(port) = l.peer.tcp_port {
                state.address = Some(SocketAddr::new(l.addr.ip(), port));
            }
            state.received.push(Received {
                packet: l.peer.to_packet(),
                payload: vec![],
                payload_error: None,
            });
        }
        self.0.changed.notify_waiters();
        let (rd, mut wr) = tokio::io::split(l.stream);
        let mut lines = BufReader::new(rd).lines();
        let (tx, mut rx) = mpsc::channel::<Packet>(128);
        let weak = Arc::downgrade(&self.0);
        let task_tx = tx.clone();
        let task = tokio::spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let mut paired = inner.dir.join("paired").exists();
            let mut requested = false;
            let mut incoming = false;
            if paired {
                scenario::greet(&task_tx).await;
            }
            drop(inner);
            loop {
                tokio::select! {
                    out = rx.recv() => {
                        let Some(out) = out else { break };
                        if out.kind == "kdeconnect.pair" {
                            requested = out.bool("pair") == Some(true);
                            paired = requested && incoming;
                            if paired { scenario::greet(&task_tx).await; }
                        }
                        if link::write_packet(&mut wr, &out).await.is_err() { break; }
                    }
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let Ok(p) = Packet::from_line(line.as_bytes()) else { continue };
                        let Some(inner) = weak.upgrade() else { break };
                        let mut received = Received { packet: p.clone(), payload: vec![], payload_error: None };
                        if let (Some(size), Some(info)) = (p.payload_size, &p.payload_transfer_info) {
                            if let Some(port) = info.get("port").and_then(Value::as_u64).and_then(|v| u16::try_from(v).ok()) {
                                if let Err(e) = payload::fetch(SocketAddr::new(l.addr.ip(), port), &inner.tls, &l.peer_cert, size, &mut received.payload).await { received.payload_error = Some(e.to_string()); }
                            }
                        }
                        inner.state.lock().unwrap_or_else(|e| e.into_inner()).received.push(received);
                        inner.changed.notify_waiters();
                        if p.kind == "kdeconnect.pair" {
                            incoming = p.bool("pair") == Some(true);
                            paired = incoming && requested;
                            if paired {
                                let _ = std::fs::write(inner.dir.join("paired"), b"1");
                                scenario::greet(&task_tx).await;
                            } else if !incoming { let _ = std::fs::remove_file(inner.dir.join("paired")); }
                        } else if paired {
                            scenario::handle(&p, &inner.phone, &task_tx, &inner.tls, &l.peer_cert).await;
                        }
                    }
                }
            }
        });
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.tx = Some(tx);
        state.task = Some(task);
    }
    pub fn disconnect(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = state.task.take() {
            task.abort();
        }
        state.tx = None;
    }
    pub async fn reconnect(&self) -> Result<()> {
        let addr = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .address
            .ok_or("no previous dial address")?;
        self.connect(addr).await
    }
    pub async fn send(&self, packet: Packet) -> Result<()> {
        let tx = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tx
            .clone()
            .ok_or("disconnected")?;
        tx.send(packet).await?;
        Ok(())
    }
    pub async fn pair(&self) -> Result<()> {
        self.send(Packet::new(
            "kdeconnect.pair",
            json!({"pair":true,"timestamp":crate::packet::now_millis()/1000}),
        ))
        .await
    }
    /// Accept a desktop-initiated pairing request.
    pub async fn accept(&self) -> Result<()> {
        std::fs::write(self.0.dir.join("paired"), b"1")?;
        self.pair().await
    }
    pub async fn unpair(&self) -> Result<()> {
        let _ = std::fs::remove_file(self.0.dir.join("paired"));
        self.send(Packet::new("kdeconnect.pair", json!({"pair":false})))
            .await
    }
    pub async fn reject(&self) -> Result<()> {
        self.unpair().await
    }
    pub fn received(&self) -> Vec<Received> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .received
            .clone()
    }
    /// Search history starting at `after`. `body` is a shallow subset match; null matches any body.
    pub async fn wait_for(
        &self,
        kind: &str,
        body: Value,
        after: usize,
        timeout: Duration,
    ) -> Result<Received> {
        Ok(tokio::time::timeout(timeout, async {
            loop {
                let notified = self.0.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(p) = self.received().into_iter().skip(after).find(|r| {
                    r.packet.kind == kind
                        && body
                            .as_object()
                            .is_none_or(|m| m.iter().all(|(k, v)| r.packet.body.get(k) == Some(v)))
                }) {
                    return p;
                }
                notified.await;
            }
        })
        .await?)
    }
    pub async fn send_payload(&self, mut packet: Packet, bytes: Vec<u8>) -> Result<()> {
        // The daemon's certificate is pinned by the main TLS handshake. A new short handshake
        // isn't needed: retain it in the connection state for payload authentication.
        let peer = self.wait_peer_cert()?;
        let (listener, port) = payload::bind_payload_listener().await?;
        packet.payload_size = Some(bytes.len() as i64);
        packet.payload_transfer_info = Some(serde_json::from_value(json!({"port":port}))?);
        self.send(packet).await?;
        let tls = self.0.tls.clone();
        tokio::spawn(async move {
            let _ = payload::serve(listener, &tls, &peer, &bytes[..]).await;
        });
        Ok(())
    }
    fn wait_peer_cert(&self) -> Result<Vec<u8>> {
        Ok(self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .peer_cert
            .clone())
    }
    pub async fn control_socket(&self, path: impl AsRef<Path>) -> Result<()> {
        let listener = UnixListener::bind(path)?;
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let Some(inner) = weak.upgrade() else { break };
                tokio::spawn(async move {
                    let sim = Simulator(inner);
                    let (rd, mut wr) = stream.into_split();
                    let mut lines = BufReader::new(rd).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let response = match serde_json::from_str::<Value>(&line) {
                            Ok(cmd) => match sim.command(cmd).await {
                                Ok(v) => json!({"result":v}),
                                Err(e) => json!({"error":e.to_string()}),
                            },
                            Err(e) => json!({"error":e.to_string()}),
                        };
                        if wr
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        Ok(())
    }
    pub async fn command(&self, cmd: Value) -> Result<Value> {
        match cmd["command"].as_str().ok_or("missing command")? {
            "identity" => return Ok(serde_json::to_value(self.identity())?),
            "connect" => {
                self.connect(cmd["address"].as_str().ok_or("missing address")?.parse()?)
                    .await?
            }
            "discover" => {
                self.discover(cmd["address"].as_str().ok_or("missing address")?.parse()?)
                    .await?
            }
            "send" => {
                self.send(serde_json::from_value(cmd["packet"].clone())?)
                    .await?
            }
            "send_payload" => {
                self.send_payload(
                    serde_json::from_value(cmd["packet"].clone())?,
                    serde_json::from_value(cmd["bytes"].clone())?,
                )
                .await?
            }
            "pair" => self.pair().await?,
            "accept" => self.accept().await?,
            "reject" => self.reject().await?,
            "unpair" => self.unpair().await?,
            "disconnect" => self.disconnect(),
            "reconnect" => self.reconnect().await?,
            "received" => return Ok(serde_json::to_value(self.received())?),
            "wait_for" => {
                return Ok(serde_json::to_value(
                    self.wait_for(
                        cmd["type"].as_str().ok_or("missing type")?,
                        cmd["match"].clone(),
                        cmd["after"].as_u64().unwrap_or(0) as usize,
                        Duration::from_millis(cmd["timeout"].as_u64().unwrap_or(5000)),
                    )
                    .await?,
                )?)
            }
            _ => return Err("unknown command".into()),
        }
        Ok(json!(true))
    }
}
