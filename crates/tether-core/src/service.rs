//! The running service: listens, announces, keeps links alive, handles pairing and routes packets.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::json;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    cert::{self, LocalCert},
    discovery,
    identity::{self, Identity, MAX_TCP_PORT, MIN_TCP_PORT, PROTOCOL_VERSION, UDP_PORT},
    link::{self, Link},
    packet::Packet,
    payload,
    plugins::{self, sms, DeviceData, Update},
    store::{Store, TrustedDevice},
    tls::TlsContext,
    Error, Result,
};

pub const PAIR: &str = "kdeconnect.pair";
/// kdeconnect-kde:core/pairinghandler.cpp: requests expire after 30 s.
const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
/// kdeconnect-kde:core/pairinghandler.cpp: pair timestamps more than 30 min off are refused.
const PAIR_MAX_SKEW_SECS: i64 = 1800;
const TICK: Duration = Duration::from_secs(5);
/// While a paired device is away, announce and dial its last address this often.
const RECONNECT_EVERY: Duration = Duration::from_secs(15);
/// Otherwise announce occasionally so new phones find us.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub device_name: String,
    pub device_type: String,
    /// Addresses to announce to directly, for networks without broadcast (VPNs, some Wi-Fi).
    pub manual_addresses: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairState {
    Unpaired,
    RequestedByUs,
    RequestedByPeer,
    Paired,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub pair: PairState,
    pub connected: bool,
    pub address: Option<String>,
    /// Shown on both screens while pairing.
    pub verification_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Device(DeviceSummary),
    Update { device_id: String, update: Update },
}

enum Pair {
    None,
    RequestedByUs { ts: i64, at: Instant },
    RequestedByPeer { ts: i64, at: Instant },
    Paired,
}

struct LinkHandle {
    gen: u64,
    tx: mpsc::Sender<Packet>,
    addr: SocketAddr,
    cert: Vec<u8>,
    tasks: [JoinHandle<()>; 2],
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        // Aborting both halves drops the socket, which closes the connection.
        for t in &self.tasks {
            t.abort();
        }
    }
}

struct Dev {
    identity: Identity,
    trusted_cert: Option<Vec<u8>>,
    last_address: Option<SocketAddr>,
    link: Option<LinkHandle>,
    pair: Pair,
    data: DeviceData,
    last_dial: Option<Instant>,
}

struct Inner {
    config: Config,
    identity: Identity,
    cert: LocalCert,
    tls: TlsContext,
    store: Store,
    devices: Mutex<HashMap<String, Dev>>,
    events: broadcast::Sender<Event>,
    next_gen: AtomicU64,
    udp: UdpSocket,
    announce_now: tokio::sync::Notify,
}

#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}

fn now_secs() -> i64 {
    crate::packet::now_millis() / 1000
}

impl Service {
    pub async fn start(config: Config) -> Result<Service> {
        std::fs::create_dir_all(&config.data_dir)?;
        let device_id = load_or_create_id(&config.data_dir)?;
        let cert = LocalCert::load_or_create(&config.data_dir, &device_id)?;
        let tls = TlsContext::new(&cert)?;

        let (listener, tcp_port) = bind_tcp().await?;
        let (udp, udp_listening) = match discovery::bind() {
            Ok(s) => (s, true),
            Err(e) => {
                warn!("UDP {UDP_PORT} is busy ({e}); is KDE Connect or Valent running? Announcing only.");
                (discovery::bind_send_only().await?, false)
            }
        };

        let identity = Identity {
            device_id,
            device_name: identity::sanitize_name(&config.device_name),
            device_type: config.device_type.clone(),
            protocol_version: PROTOCOL_VERSION,
            incoming_capabilities: plugins::incoming_capabilities(),
            outgoing_capabilities: plugins::outgoing_capabilities(),
            tcp_port: Some(tcp_port),
        };
        info!("device id {} on tcp {tcp_port}", identity.device_id);

        let store = Store::new(config.data_dir.join("devices"));
        let mut devices = HashMap::new();
        for t in store.load_all() {
            let last_address = t.last_address.as_deref().and_then(|a| a.parse().ok());
            devices.insert(
                t.identity.device_id.clone(),
                Dev {
                    trusted_cert: t.cert_der(),
                    identity: t.identity,
                    last_address,
                    link: None,
                    pair: Pair::Paired,
                    data: DeviceData::default(),
                    last_dial: None,
                },
            );
        }

        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new(Inner {
            config,
            identity,
            cert,
            tls,
            store,
            devices: Mutex::new(devices),
            events,
            next_gen: AtomicU64::new(1),
            udp,
            announce_now: tokio::sync::Notify::new(),
        });

        tokio::spawn(accept_loop(inner.clone(), listener));
        if udp_listening {
            tokio::spawn(udp_loop(inner.clone()));
        }
        tokio::spawn(maintenance_loop(inner.clone()));
        Ok(Service { inner })
    }

    pub fn identity(&self) -> &Identity {
        &self.inner.identity
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Announce now, e.g. after the network changed or the computer woke up.
    pub fn refresh(&self) {
        self.inner.announce_now.notify_one();
    }

    /// Reaches a device by address: announces to it and also dials it directly, because on VPNs
    /// and NATed setups the device may hear us but be unable to connect back.
    pub fn add_manual_address(&self, addr: SocketAddr) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _ = inner
                .udp
                .send_to(&inner.identity.to_packet().to_line(), addr)
                .await;
            match link::connect(addr, &inner.identity, &inner.tls).await {
                Ok(l) => inner.on_link_at(l, Some(addr)),
                Err(e) => debug!("direct connect to {addr} failed: {e}"),
            }
        });
    }

    pub fn devices(&self) -> Vec<DeviceSummary> {
        let devs = self.inner.lock();
        let mut v: Vec<_> = devs.values().map(|d| self.inner.summary(d)).collect();
        v.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
        v
    }

    pub fn device_data(&self, id: &str) -> Result<DeviceData> {
        let devs = self.inner.lock();
        devs.get(id)
            .map(|d| d.data.clone())
            .ok_or_else(|| Error::UnknownDevice(id.into()))
    }

    pub fn threads(&self, id: &str) -> Result<Vec<sms::Message>> {
        let devs = self.inner.lock();
        let d = devs
            .get(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        Ok(sms::thread_heads(&d.data))
    }

    pub fn thread(&self, id: &str, thread_id: i64) -> Result<Vec<sms::Message>> {
        let devs = self.inner.lock();
        let d = devs
            .get(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        Ok(sms::thread(&d.data, thread_id))
    }

    pub fn contacts(&self, id: &str) -> Result<Vec<(String, String)>> {
        let devs = self.inner.lock();
        let d = devs
            .get(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        Ok(d.data.contacts.all())
    }

    pub fn contact_name(&self, id: &str, number: &str) -> Option<String> {
        let devs = self.inner.lock();
        devs.get(id).and_then(|d| d.data.contacts.name_for(number))
    }

    pub fn pair(&self, id: &str) -> Result<()> {
        let ts = now_secs();
        let mut devs = self.inner.lock();
        let d = devs
            .get_mut(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        if matches!(d.pair, Pair::Paired) {
            return Ok(());
        }
        let link = d
            .link
            .as_ref()
            .ok_or_else(|| Error::NotConnected(id.into()))?;
        let _ = link
            .tx
            .try_send(Packet::new(PAIR, json!({"pair": true, "timestamp": ts})));
        d.pair = Pair::RequestedByUs {
            ts,
            at: Instant::now(),
        };
        let s = self.inner.summary(d);
        drop(devs);
        self.inner.emit(Event::Device(s));
        Ok(())
    }

    pub fn accept_pair(&self, id: &str) -> Result<()> {
        let mut devs = self.inner.lock();
        let d = devs
            .get_mut(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        if !matches!(d.pair, Pair::RequestedByPeer { .. }) {
            return Err(Error::Protocol(
                "no pairing request from this device".into(),
            ));
        }
        if let Some(l) = &d.link {
            let _ = l.tx.try_send(Packet::new(PAIR, json!({"pair": true})));
        }
        self.inner.become_paired(d);
        let s = self.inner.summary(d);
        drop(devs);
        self.inner.emit(Event::Device(s));
        Ok(())
    }

    pub fn reject_pair(&self, id: &str) -> Result<()> {
        self.unpair(id)
    }

    pub fn unpair(&self, id: &str) -> Result<()> {
        let mut devs = self.inner.lock();
        let d = devs
            .get_mut(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        if let Some(l) = &d.link {
            let _ = l.tx.try_send(Packet::new(PAIR, json!({"pair": false})));
        }
        self.inner.become_unpaired(d);
        let s = self.inner.summary(d);
        let gone = d.link.is_none();
        if gone {
            devs.remove(id);
        }
        drop(devs);
        self.inner.emit(Event::Device(s));
        Ok(())
    }

    /// Sends a packet to a paired, connected device.
    pub fn send(&self, id: &str, p: Packet) -> Result<()> {
        let devs = self.inner.lock();
        let d = devs
            .get(id)
            .ok_or_else(|| Error::UnknownDevice(id.into()))?;
        if !matches!(d.pair, Pair::Paired) {
            return Err(Error::NotPaired(id.into()));
        }
        let l = d
            .link
            .as_ref()
            .ok_or_else(|| Error::NotConnected(id.into()))?;
        l.tx.try_send(p).map_err(|_| Error::NotConnected(id.into()))
    }

    /// Sends a file. Resolves once the phone has finished downloading it.
    pub async fn share_file(&self, id: &str, path: &Path) -> Result<u64> {
        let file = tokio::fs::File::open(path).await?;
        let size = file.metadata().await?.len();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let cert = {
            let devs = self.inner.lock();
            let d = devs
                .get(id)
                .ok_or_else(|| Error::UnknownDevice(id.into()))?;
            d.link
                .as_ref()
                .map(|l| l.cert.clone())
                .ok_or_else(|| Error::NotConnected(id.into()))?
        };
        let (listener, port) = payload::bind_payload_listener().await?;
        let mut p = Packet::new(plugins::SHARE, json!({"filename": name, "open": false}));
        p.payload_size = Some(size as i64);
        p.payload_transfer_info = Some(
            json!({"port": port})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        );
        self.send(id, p)?;
        payload::serve(listener, &self.inner.tls, &cert, file).await
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Dev>> {
        // A poisoned lock only means a panic elsewhere; the map itself is still usable.
        self.devices.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn summary(&self, d: &Dev) -> DeviceSummary {
        let (pair, ts) = match d.pair {
            Pair::None => (PairState::Unpaired, None),
            Pair::RequestedByUs { ts, .. } => (PairState::RequestedByUs, Some(ts)),
            Pair::RequestedByPeer { ts, .. } => (PairState::RequestedByPeer, Some(ts)),
            Pair::Paired => (PairState::Paired, None),
        };
        let verification_key = ts.and_then(|ts| {
            let cert = d.link.as_ref()?.cert.clone();
            cert::verification_key(&self.cert.cert_der, &cert, ts)
        });
        DeviceSummary {
            id: d.identity.device_id.clone(),
            name: d.identity.device_name.clone(),
            kind: d.identity.device_type.clone(),
            pair,
            connected: d.link.is_some(),
            address: d.link.as_ref().map(|l| l.addr.ip().to_string()),
            verification_key,
        }
    }

    fn become_paired(&self, d: &mut Dev) {
        d.pair = Pair::Paired;
        if let Some(l) = &d.link {
            d.trusted_cert = Some(l.cert.clone());
            for p in plugins::on_connected() {
                let _ = l.tx.try_send(p);
            }
        }
        self.persist(d);
    }

    fn become_unpaired(&self, d: &mut Dev) {
        d.pair = Pair::None;
        d.trusted_cert = None;
        d.data = DeviceData::default();
        if let Err(e) = self.store.remove(&d.identity.device_id) {
            warn!("could not forget device: {e}");
        }
    }

    fn persist(&self, d: &Dev) {
        let Some(cert) = &d.trusted_cert else { return };
        let t = TrustedDevice {
            identity: d.identity.clone(),
            certificate: hex::encode(cert),
            last_address: d.last_address.map(|a| a.to_string()),
        };
        if let Err(e) = self.store.save(&t) {
            warn!("could not save device: {e}");
        }
    }

    fn on_link(self: &Arc<Self>, link: Link) {
        self.on_link_at(link, None)
    }

    /// `dialed` is the address we connected to; it beats the announced port when they differ
    /// (port forwarding, containers), so redials go to the address that actually worked.
    fn on_link_at(self: &Arc<Self>, link: Link, dialed: Option<SocketAddr>) {
        let Link {
            peer,
            peer_cert,
            addr,
            stream,
        } = link;
        let id = peer.device_id.clone();
        if id == self.identity.device_id {
            return;
        }
        let gen = self.next_gen.fetch_add(1, Ordering::Relaxed);
        let mut devs = self.lock();
        if let Some(d) = devs.get(&id) {
            if let Some(trusted) = &d.trusted_cert {
                if *trusted != peer_cert {
                    // Someone else is using this id, or the phone was reset. Never silently re-trust.
                    warn!(
                        "{} presented a different certificate than at pairing; refusing",
                        d.identity.device_name
                    );
                    return;
                }
            }
        }

        let (rd, mut wr) = tokio::io::split(stream);
        let (tx, mut rx) = mpsc::channel::<Packet>(256);
        let writer = tokio::spawn({
            let inner = self.clone();
            let id = id.clone();
            async move {
                while let Some(p) = rx.recv().await {
                    if let Err(e) = link::write_packet(&mut wr, &p).await {
                        debug!("write failed: {e}");
                        break;
                    }
                }
                inner.link_lost(&id, gen);
            }
        });
        let reader = tokio::spawn({
            let inner = self.clone();
            let id = id.clone();
            async move {
                let mut rd = tokio::io::BufReader::new(rd);
                loop {
                    match link::read_line(&mut rd).await {
                        Ok(Some(line)) => match Packet::from_line(&line) {
                            Ok(p) => inner.on_packet(&id, gen, p),
                            Err(e) => debug!("skipping malformed packet: {e}"),
                        },
                        Ok(None) => break,
                        Err(e) => {
                            debug!("read failed: {e}");
                            break;
                        }
                    }
                }
                inner.link_lost(&id, gen);
            }
        });
        let handle = LinkHandle {
            gen,
            tx,
            addr,
            cert: peer_cert,
            tasks: [writer, reader],
        };

        let tcp_addr = dialed.or_else(|| {
            peer.tcp_port
                .filter(|p| (MIN_TCP_PORT..=MAX_TCP_PORT).contains(p))
                .map(|p| SocketAddr::new(addr.ip(), p))
        });
        let d = devs.entry(id.clone()).or_insert_with(|| Dev {
            identity: peer.clone(),
            trusted_cert: None,
            last_address: None,
            link: None,
            pair: Pair::None,
            data: DeviceData::default(),
            last_dial: None,
        });
        d.identity = peer;
        if tcp_addr.is_some() {
            d.last_address = tcp_addr;
        }
        let paired = matches!(d.pair, Pair::Paired);
        if paired {
            for p in plugins::on_connected() {
                let _ = handle.tx.try_send(p);
            }
        }
        // Replacing drops the old link, which closes it.
        d.link = Some(handle);
        if paired {
            self.persist(d);
        }
        info!("connected to {}", d.identity.device_name);
        let s = self.summary(d);
        drop(devs);
        self.emit(Event::Device(s));
    }

    fn link_lost(&self, id: &str, gen: u64) {
        let mut devs = self.lock();
        let Some(d) = devs.get_mut(id) else { return };
        if d.link.as_ref().map(|l| l.gen) != Some(gen) {
            return; // an older link; a newer one already replaced it
        }
        d.link = None;
        if !matches!(d.pair, Pair::Paired) {
            let s = DeviceSummary {
                connected: false,
                ..self.summary(d)
            };
            devs.remove(id);
            drop(devs);
            self.emit(Event::Device(s));
            return;
        }
        info!("lost {}", d.identity.device_name);
        let s = self.summary(d);
        drop(devs);
        self.emit(Event::Device(s));
        self.announce_now.notify_one();
    }

    fn on_packet(self: &Arc<Self>, id: &str, gen: u64, p: Packet) {
        let mut devs = self.lock();
        let Some(d) = devs.get_mut(id) else { return };
        if d.link.as_ref().map(|l| l.gen) != Some(gen) {
            return;
        }
        if p.kind == PAIR {
            self.on_pair_packet(d, &p);
            let s = self.summary(d);
            drop(devs);
            self.emit(Event::Device(s));
            return;
        }
        if !matches!(d.pair, Pair::Paired) {
            // kdeconnect-kde:core/device.cpp: packets from unpaired devices are dropped and the
            // peer is told we are not paired, so it can update its own state.
            debug!("dropping {} from unpaired device", p.kind);
            if let Some(l) = &d.link {
                let _ = l.tx.try_send(Packet::new(PAIR, json!({"pair": false})));
            }
            return;
        }

        let Some(link) = d.link.as_ref() else { return };
        let (addr, cert) = (link.addr, link.cert.clone());

        // Payload-bearing packets need the network; start the transfer outside the lock.
        if let Some(port) = p.payload_port() {
            let size = p.payload_size.unwrap_or(-1);
            let target = SocketAddr::new(addr.ip(), port);
            match p.kind.as_str() {
                plugins::SHARE => {
                    let name = p.str("filename").unwrap_or("file").to_string();
                    tokio::spawn(self.clone().receive_file(
                        id.to_string(),
                        target,
                        cert,
                        size,
                        name,
                    ));
                    return;
                }
                plugins::notification::NOTIFICATION => {
                    if let Some(hash) = p.str("payloadHash").filter(|h| is_safe_name(h)) {
                        let path = self
                            .config
                            .data_dir
                            .join("icons")
                            .join(format!("{hash}.png"));
                        if !path.exists() {
                            tokio::spawn(self.clone().receive_icon(
                                id.to_string(),
                                p.str("id").unwrap_or("").to_string(),
                                target,
                                cert,
                                size,
                                path,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        let out = plugins::handle(&mut d.data, &p);
        // Fill in icons we already have on disk.
        if p.kind == plugins::notification::NOTIFICATION {
            if let Some(n) = p
                .str("id")
                .and_then(|nid| d.data.notifications.get_mut(nid))
            {
                if let Some(hash) = n.icon_hash.as_deref().filter(|h| is_safe_name(h)) {
                    let path = self
                        .config
                        .data_dir
                        .join("icons")
                        .join(format!("{hash}.png"));
                    if path.exists() {
                        n.icon_path = Some(path.to_string_lossy().into());
                    }
                }
            }
        }
        if let Some(l) = &d.link {
            for r in out.replies {
                let _ = l.tx.try_send(r);
            }
        }
        let mut updates = out.updates;
        if p.kind == plugins::notification::NOTIFICATION {
            // Re-read so the update carries the icon path filled in above.
            if let Some(n) = p.str("id").and_then(|nid| d.data.notifications.get(nid)) {
                for u in &mut updates {
                    if let Update::Notification(x) = u {
                        *x = n.clone();
                    }
                }
            }
        }
        drop(devs);
        for u in updates {
            self.emit(Event::Update {
                device_id: id.to_string(),
                update: u,
            });
        }
    }

    fn on_pair_packet(&self, d: &mut Dev, p: &Packet) {
        let wants = p.bool("pair").unwrap_or(false);
        let link_tx = d.link.as_ref().map(|l| l.tx.clone());
        let reply = |pair: bool| {
            if let Some(tx) = &link_tx {
                let _ = tx.try_send(Packet::new(PAIR, json!({"pair": pair})));
            }
        };
        if !wants {
            if !matches!(d.pair, Pair::None) {
                info!("{} unpaired", d.identity.device_name);
                self.become_unpaired(d);
            }
            return;
        }
        match d.pair {
            // kdeconnect-android:PairingHandler.kt: a phone that lost our pairing (reset, reinstall)
            // sends a fresh request with a timestamp. Replying pair:true would make it unpair, so
            // drop our trust and treat it as a new request. Without a timestamp it's noise.
            Pair::Paired if p.i64("timestamp").is_none() => {}
            Pair::Paired => {
                info!("{} asked to pair again", d.identity.device_name);
                self.become_unpaired(d);
                self.on_pair_request(d, p, reply);
            }
            Pair::RequestedByUs { .. } => {
                info!("paired with {}", d.identity.device_name);
                self.become_paired(d);
            }
            Pair::None | Pair::RequestedByPeer { .. } => self.on_pair_request(d, p, reply),
        }
    }

    fn on_pair_request(&self, d: &mut Dev, p: &Packet, reply: impl Fn(bool)) {
        let ts = p.i64("timestamp").unwrap_or(0);
        if (now_secs() - ts).abs() > PAIR_MAX_SKEW_SECS {
            warn!(
                "pair request from {} refused: clocks differ by over 30 minutes",
                d.identity.device_name
            );
            reply(false);
            return;
        }
        d.pair = Pair::RequestedByPeer {
            ts,
            at: Instant::now(),
        };
    }

    async fn receive_file(
        self: Arc<Self>,
        id: String,
        target: SocketAddr,
        cert: Vec<u8>,
        size: i64,
        name: String,
    ) {
        let dir = self.config.downloads_dir.clone();
        let result: Result<PathBuf> = async {
            tokio::fs::create_dir_all(&dir).await?;
            let path = unique_path(&dir, &safe_file_name(&name));
            let partial = path.with_extension("part");
            let mut f = tokio::fs::File::create(&partial).await?;
            match payload::fetch(target, &self.tls, &cert, size, &mut f).await {
                Ok(_) => {
                    tokio::fs::rename(&partial, &path).await?;
                    Ok(path)
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&partial).await;
                    Err(e)
                }
            }
        }
        .await;
        let update = match result {
            Ok(path) => Update::FileReceived {
                path: path.to_string_lossy().into(),
            },
            Err(e) => Update::TransferFailed {
                name,
                reason: e.to_string(),
            },
        };
        self.emit(Event::Update {
            device_id: id,
            update,
        });
    }

    async fn receive_icon(
        self: Arc<Self>,
        id: String,
        notif_id: String,
        target: SocketAddr,
        cert: Vec<u8>,
        size: i64,
        path: PathBuf,
    ) {
        let ok: Result<()> = async {
            if let Some(dir) = path.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            let tmp = path.with_extension("part");
            let mut f = tokio::fs::File::create(&tmp).await?;
            payload::fetch(target, &self.tls, &cert, size, &mut f).await?;
            tokio::fs::rename(&tmp, &path).await?;
            Ok(())
        }
        .await;
        if let Err(e) = ok {
            debug!("icon download failed: {e}");
            return;
        }
        let mut devs = self.lock();
        let Some(n) = devs
            .get_mut(&id)
            .and_then(|d| d.data.notifications.get_mut(&notif_id))
        else {
            return;
        };
        n.icon_path = Some(path.to_string_lossy().into());
        let n = n.clone();
        drop(devs);
        self.emit(Event::Update {
            device_id: id,
            update: Update::Notification(Notification { silent: true, ..n }),
        });
    }

    /// Expires pairing requests and redials paired devices that went away.
    fn tick(self: &Arc<Self>) -> bool {
        let mut changed = vec![];
        let mut dial = vec![];
        let mut any_missing = false;
        let mut devs = self.lock();
        for d in devs.values_mut() {
            let expired = match d.pair {
                Pair::RequestedByUs { at, .. } | Pair::RequestedByPeer { at, .. } => {
                    at.elapsed() > PAIR_TIMEOUT
                }
                _ => false,
            };
            if expired {
                // kdeconnect-kde:core/backends/pairinghandler.cpp `pairingTimeout` tells the peer.
                if let Some(l) = &d.link {
                    let _ = l.tx.try_send(Packet::new(PAIR, json!({"pair": false})));
                }
                d.pair = Pair::None;
                changed.push(self.summary(d));
            }
            if matches!(d.pair, Pair::Paired) && d.link.is_none() {
                any_missing = true;
                let due = d.last_dial.is_none_or(|t| t.elapsed() >= RECONNECT_EVERY);
                if let (true, Some(addr)) = (due, d.last_address) {
                    d.last_dial = Some(Instant::now());
                    dial.push(addr);
                }
            }
        }
        drop(devs);
        for s in changed {
            self.emit(Event::Device(s));
        }
        for addr in dial {
            let inner = self.clone();
            tokio::spawn(async move {
                // Unicast our announcement too: the phone may be reachable but not listening on TCP
                // (Android stops its listener in some power states but still hears UDP).
                let _ = inner
                    .udp
                    .send_to(
                        &inner.identity.to_packet().to_line(),
                        SocketAddr::new(addr.ip(), UDP_PORT),
                    )
                    .await;
                match link::connect(addr, &inner.identity, &inner.tls).await {
                    Ok(l) => inner.on_link_at(l, Some(addr)),
                    Err(e) => debug!("redial {addr} failed: {e}"),
                }
            });
        }
        any_missing
    }

    async fn announce(&self) {
        let line = self.identity.to_packet().to_line();
        for target in discovery::broadcast_targets(&self.config.manual_addresses) {
            if let Err(e) = self.udp.send_to(&line, target).await {
                debug!("announce to {target} failed: {e}");
            }
        }
    }
}

use plugins::Notification;

async fn bind_tcp() -> Result<(TcpListener, u16)> {
    for port in MIN_TCP_PORT..=MAX_TCP_PORT {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", port)).await {
            return Ok((l, port));
        }
    }
    Err(Error::Protocol("no free TCP port in 1716-1764".into()))
}

async fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    loop {
        let (tcp, addr) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let inner = inner.clone();
        tokio::spawn(async move {
            match link::accept(tcp, addr, &inner.identity, &inner.tls).await {
                Ok(l) => inner.on_link(l),
                Err(e) => debug!("incoming from {addr} failed: {e}"),
            }
        });
    }
}

async fn udp_loop(inner: Arc<Inner>) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut recent: HashMap<String, Instant> = HashMap::new();
    loop {
        let (n, from) = match inner.udp.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                warn!("udp receive failed: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let Ok(p) = Packet::from_line(&buf[..n]) else {
            continue;
        };
        let Ok(peer) = Identity::from_packet(&p) else {
            continue;
        };
        if peer.device_id == inner.identity.device_id || peer.protocol_version < PROTOCOL_VERSION {
            continue;
        }
        let Some(port) = peer
            .tcp_port
            .filter(|p| (MIN_TCP_PORT..=MAX_TCP_PORT).contains(p))
        else {
            continue;
        };
        let connected = inner
            .lock()
            .get(&peer.device_id)
            .is_some_and(|d| d.link.is_some());
        // Phones announce in bursts; one connection attempt per device every few seconds is plenty.
        let now = Instant::now();
        recent.retain(|_, t| now.duration_since(*t) < Duration::from_secs(5));
        if connected || recent.contains_key(&peer.device_id) {
            continue;
        }
        recent.insert(peer.device_id.clone(), now);
        let addr = SocketAddr::new(from.ip(), port);
        let inner = inner.clone();
        tokio::spawn(async move {
            match link::connect(addr, &inner.identity, &inner.tls).await {
                Ok(l) => inner.on_link(l),
                Err(e) => debug!("connect to {addr} failed: {e}"),
            }
        });
    }
}

async fn maintenance_loop(inner: Arc<Inner>) {
    inner.announce().await;
    let mut last_announce = Instant::now();
    let mut interval = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = inner.announce_now.notified() => {
                inner.announce().await;
                last_announce = Instant::now();
            }
        }
        let missing = inner.tick();
        let every = if missing {
            RECONNECT_EVERY
        } else {
            ANNOUNCE_EVERY
        };
        if last_announce.elapsed() >= every {
            inner.announce().await;
            last_announce = Instant::now();
        }
    }
}

fn load_or_create_id(dir: &Path) -> Result<String> {
    let path = dir.join("device_id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if identity::is_valid_device_id(&s) {
            return Ok(s);
        }
    }
    let id = identity::new_device_id();
    std::fs::write(&path, &id)?;
    Ok(id)
}

fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// File names come from the phone: keep only the last path component and drop control characters.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "file".into()
    } else {
        cleaned.chars().take(200).collect()
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..10_000)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_cannot_escape() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("..\\x.txt"), "x.txt");
        assert_eq!(safe_file_name(".bashrc"), "bashrc");
        assert_eq!(safe_file_name(""), "file");
        assert_eq!(safe_file_name("photo.jpg"), "photo.jpg");
    }
}
