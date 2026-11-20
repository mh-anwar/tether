//! Phone calls through this computer: answer, decline, hang up and dial, with the call's sound on the
//! computer's speakers and microphone. KDE Connect only reports calls, so this goes over Bluetooth
//! hands-free (HFP) with the computer as the headset, the way Phone Link does it.
//!
//! PipeWire 1.4+ speaks HFP itself and publishes each connected phone on the session bus as
//! `org.pipewire.Telephony` (pipewire spa/plugins/bluez5/telephony.c): an ObjectManager at
//! `/org/pipewire/Telephony` lists audio gateways (`.../agN`, AudioGateway1 + AudioGatewayTransport1),
//! and each gateway is an ObjectManager for its calls (`.../agN/callM`, Call1). BlueZ on the system
//! bus says whether the phone is paired at all, so the window can explain what is missing.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tether_core::{PairState, Service};
use tokio::sync::Notify;
use tracing::{debug, info};
use zbus::{Connection, OwnedObjectPath, OwnedVariant};

const TELEPHONY: &str = "org.pipewire.Telephony";
const ROOT: &str = "/org/pipewire/Telephony";
const AG: &str = "org.pipewire.Telephony.AudioGateway1";
const AG_TRANSPORT: &str = "org.pipewire.Telephony.AudioGatewayTransport1";
const CALL: &str = "org.pipewire.Telephony.Call1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
/// Handsfree Audio Gateway: the phone's side of HFP, listed in a phone's BlueZ UUIDs.
const HFP_AG_UUID: &str = "0000111f-0000-1000-8000-00805f9b34fb";
/// PipeWire parses ATD with `%30[^;]` (backend-native.c), so longer numbers never reach the phone.
const MAX_NUMBER: usize = 30;

type Props = HashMap<String, OwnedVariant>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

/// What the window shows. `blocker` is None when calls work; otherwise a code the window turns into
/// setup steps: "service" (no PipeWire telephony), "bluetooth" (no Bluetooth service or adapter),
/// "adapter_off", "pair" (phone not paired over Bluetooth), "connect" (paired, not connected for calls).
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Status {
    pub blocker: Option<&'static str>,
    /// The phone's Bluetooth name, once one is known.
    pub phone: Option<String>,
    /// True while the call's sound runs through this computer (the HFP audio link is up).
    pub audio_here: bool,
    pub calls: Vec<Call>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Call {
    /// The call's D-Bus path; the window hands it back to answer or hang up this call.
    pub id: String,
    pub number: String,
    pub name: Option<String>,
    /// incoming, waiting, dialing, alerting, active or held (PipeWire's call states).
    pub state: String,
    pub incoming: bool,
    /// Unix milliseconds when the call was answered, for the window's timer.
    pub since: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
struct Gateway {
    path: OwnedObjectPath,
    address: String,
    transport: String,
    calls: Vec<RawCall>,
}

#[derive(Debug, Clone, PartialEq)]
struct RawCall {
    path: String,
    number: String,
    name: String,
    state: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Bluetooth {
    /// BlueZ isn't on the system bus, or there is no adapter.
    Missing,
    Off,
    Devices(Vec<BtDevice>),
}

#[derive(Debug, Clone, PartialEq)]
struct BtDevice {
    address: String,
    name: String,
    paired: bool,
    connected: bool,
    hfp: bool,
}

fn text(p: &Props, k: &str) -> String {
    p.get(k)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or_default()
        .to_owned()
}

fn flag(p: &Props, k: &str) -> bool {
    p.get(k)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Same-name matching between the KDE Connect name and the Bluetooth name: Samsung and most phones
/// use the device name for both, but spacing and case can differ.
fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && a == b
}

/// Picks the phone among paired Bluetooth devices: one named like a KDE Connect phone first, then
/// any phone offering HFP, connected ones before the rest.
fn pick_phone<'a>(devices: &'a [BtDevice], names: &[String]) -> Option<&'a BtDevice> {
    let phones: Vec<_> = devices.iter().filter(|d| d.paired && d.hfp).collect();
    phones
        .iter()
        .find(|d| names.iter().any(|n| same_name(n, &d.name)))
        .or_else(|| phones.iter().find(|d| d.connected))
        .or_else(|| phones.first())
        .copied()
}

/// What's in the way, the phone's name and which gateway to use. A gateway means HFP is up, which
/// beats anything BlueZ says.
fn summarize<'a>(
    gateways: Option<&'a [Gateway]>,
    bt: &Bluetooth,
    names: &[String],
) -> (Option<&'static str>, Option<String>, Option<&'a Gateway>) {
    let devices = match bt {
        Bluetooth::Devices(d) => d.as_slice(),
        _ => &[],
    };
    let phone = pick_phone(devices, names);
    if let Some(gws) = gateways.filter(|g| !g.is_empty()) {
        let gw = phone
            .and_then(|p| {
                gws.iter()
                    .find(|g| g.address.eq_ignore_ascii_case(&p.address))
            })
            .or_else(|| {
                // A gateway BlueZ didn't list (or BlueZ unreadable): match it by the KDE Connect name.
                gws.iter().find(|g| {
                    devices
                        .iter()
                        .find(|d| d.address.eq_ignore_ascii_case(&g.address))
                        .is_some_and(|d| names.iter().any(|n| same_name(n, &d.name)))
                })
            })
            .unwrap_or(&gws[0]);
        let name = devices
            .iter()
            .find(|d| d.address.eq_ignore_ascii_case(&gw.address))
            .map(|d| d.name.clone());
        return (None, name, Some(gw));
    }
    let blocker = if gateways.is_none() {
        "service"
    } else {
        match bt {
            Bluetooth::Missing => "bluetooth",
            Bluetooth::Off => "adapter_off",
            Bluetooth::Devices(_) if phone.is_none() => "pair",
            Bluetooth::Devices(_) => "connect",
        }
    };
    (Some(blocker), phone.map(|p| p.name.clone()), None)
}

/// Remembers per call what PipeWire doesn't say: its direction and when it was answered.
#[derive(Default)]
struct Tracker {
    seen: HashMap<String, (bool, Option<i64>)>,
}

impl Tracker {
    fn calls(
        &mut self,
        raw: &[RawCall],
        lookup: &dyn Fn(&str) -> Option<String>,
        now: i64,
    ) -> Vec<Call> {
        let live: Vec<_> = raw.iter().filter(|c| c.state != "disconnected").collect();
        self.seen
            .retain(|path, _| live.iter().any(|c| &c.path == path));
        live.into_iter()
            .map(|c| {
                let entry = self.seen.entry(c.path.clone()).or_insert_with(|| {
                    // A call first seen already answered (Tether started mid-call) has an unknown
                    // direction; "outgoing" keeps the window from offering Answer.
                    (matches!(c.state.as_str(), "incoming" | "waiting"), None)
                });
                if entry.1.is_none() && matches!(c.state.as_str(), "active" | "held") {
                    entry.1 = Some(now);
                }
                let name =
                    lookup(&c.number).or_else(|| (!c.name.is_empty()).then(|| c.name.clone()));
                Call {
                    id: c.path.clone(),
                    number: c.number.clone(),
                    name,
                    state: c.state.clone(),
                    incoming: entry.0,
                    since: entry.1,
                }
            })
            .collect()
    }
}

/// Keeps the dial string to what HFP's ATD accepts, so a pasted "(555) 123-4567" works and nothing
/// else rides along into the AT command.
pub fn clean_number(input: &str) -> Result<String> {
    let mut out = String::new();
    for c in input.trim().chars() {
        match c {
            '0'..='9' | '*' | '#' => out.push(c),
            '+' if out.is_empty() => out.push(c),
            ' ' | '-' | '(' | ')' | '.' | '/' | '\u{a0}' => {}
            _ => bail!("That isn't a phone number"),
        }
    }
    if out.is_empty() || out == "+" {
        bail!("Enter a phone number");
    }
    if out.len() > MAX_NUMBER {
        bail!("That number is too long");
    }
    Ok(out)
}

type Lookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
type Names = Box<dyn Fn() -> Vec<String> + Send + Sync>;

struct Inner {
    status: Status,
    tracker: Tracker,
    gateway: Option<OwnedObjectPath>,
}

pub struct Watcher {
    session: Option<Connection>,
    system: Option<Connection>,
    lookup: Lookup,
    names: Names,
    inner: Mutex<Inner>,
    poke: Notify,
    changed: tokio::sync::watch::Sender<Status>,
}

static WATCHER: OnceLock<Arc<Watcher>> = OnceLock::new();

async fn connect(builder: zbus::conn::Builder<'static>) -> Option<Connection> {
    builder
        .method_timeout(Duration::from_secs(5))
        .build()
        .await
        .ok()
}

pub async fn run(svc: Service) {
    let session = connect(zbus::conn::Builder::session()).await;
    let system = connect(zbus::conn::Builder::system()).await;
    let (a, b) = (svc.clone(), svc);
    let lookup: Lookup = Box::new(move |number| {
        a.devices()
            .iter()
            .filter(|d| matches!(d.pair, PairState::Paired))
            .find_map(|d| a.contact_name(&d.id, number))
    });
    let names: Names = Box::new(move || {
        b.devices()
            .into_iter()
            .filter(|d| matches!(d.pair, PairState::Paired))
            .map(|d| d.name)
            .collect()
    });
    let w = Arc::new(Watcher::new(session, system, lookup, names));
    let _ = WATCHER.set(w.clone());
    let mut changes = w.subscribe();
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            let mut v = serde_json::to_value(&*changes.borrow_and_update()).unwrap_or(Value::Null);
            v["event"] = json!("calls");
            crate::api::ui_event(v);
        }
    });
    w.watch().await;
}

fn watcher() -> Result<&'static Arc<Watcher>> {
    WATCHER.get().context("Calls aren't ready yet")
}

pub fn status() -> Value {
    WATCHER
        .get()
        .map(|w| serde_json::to_value(w.status()).unwrap_or(Value::Null))
        .unwrap_or_else(|| {
            json!(Status {
                blocker: Some("service"),
                ..Default::default()
            })
        })
}

pub async fn answer(id: Option<&str>) -> Result<()> {
    watcher()?.answer(id).await
}

pub async fn hangup(id: Option<&str>) -> Result<()> {
    watcher()?.hangup(id).await
}

pub async fn dial(number: &str) -> Result<()> {
    watcher()?.dial(number).await
}

pub async fn audio_here() -> Result<()> {
    watcher()?.audio_here().await
}

impl Watcher {
    fn new(
        session: Option<Connection>,
        system: Option<Connection>,
        lookup: Lookup,
        names: Names,
    ) -> Self {
        Self {
            session,
            system,
            lookup,
            names,
            inner: Mutex::new(Inner {
                status: Status {
                    blocker: Some("service"),
                    ..Default::default()
                },
                tracker: Tracker::default(),
                gateway: None,
            }),
            poke: Notify::new(),
            changed: tokio::sync::watch::channel(Status::default()).0,
        }
    }

    fn subscribe(&self) -> tokio::sync::watch::Receiver<Status> {
        self.changed.subscribe()
    }

    pub fn status(&self) -> Status {
        self.inner
            .lock()
            .map(|i| i.status.clone())
            .unwrap_or_default()
    }

    /// Re-reads everything on any signal from PipeWire's telephony service or BlueZ, and every few
    /// seconds besides: the service can appear later (PipeWire restarts, Bluetooth turned on), and a
    /// full re-read can't drift the way applying single signals can.
    async fn watch(&self) {
        let mut tel = signal_stream(self.session.as_ref(), TELEPHONY).await;
        let mut bt = signal_stream(self.system.as_ref(), BLUEZ).await;
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            self.refresh().await;
            tokio::select! {
                Some(_) = next(&mut tel) => {}
                Some(_) = next(&mut bt) => {}
                _ = tick.tick() => {}
                _ = self.poke.notified() => {}
            }
        }
    }

    async fn refresh(&self) {
        let gateways = match &self.session {
            Some(c) => read_gateways(c).await,
            None => None,
        };
        let bt = match &self.system {
            Some(c) => read_bluetooth(c).await,
            None => Bluetooth::Missing,
        };
        let names = (self.names)();
        let (blocker, phone, gw) = summarize(gateways.as_deref(), &bt, &names);
        let now = now_ms();
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let calls = inner.tracker.calls(
            gw.map(|g| g.calls.as_slice()).unwrap_or(&[]),
            &*self.lookup,
            now,
        );
        let status = Status {
            blocker,
            phone,
            audio_here: gw.is_some_and(|g| g.transport == "active"),
            calls,
        };
        inner.gateway = gw.map(|g| g.path.clone());
        if status != inner.status {
            if status.blocker != inner.status.blocker {
                info!("calls: {}", status.blocker.unwrap_or("ready"));
            }
            inner.status = status.clone();
            drop(inner);
            self.changed.send_replace(status);
        }
    }

    fn session(&self) -> Result<&Connection> {
        self.session.as_ref().context("No desktop session bus")
    }

    fn gateway(&self) -> Result<OwnedObjectPath> {
        self.inner
            .lock()
            .ok()
            .and_then(|i| i.gateway.clone())
            .ok_or_else(|| anyhow!("Your phone isn't connected for calls"))
    }

    /// The call to act on: the one asked for, which must be live, or else the first matching `pick`.
    fn find(&self, id: Option<&str>, pick: impl Fn(&Call) -> bool) -> Result<Call> {
        let status = self.status();
        match id {
            Some(id) => status.calls.into_iter().find(|c| c.id == id),
            None => status.calls.into_iter().find(|c| pick(c)),
        }
        .ok_or_else(|| anyhow!("That call has ended"))
    }

    async fn call_method(&self, path: &str, iface: &str, method: &str) -> Result<()> {
        self.session()?
            .call_method(Some(TELEPHONY), path, Some(iface), method, &())
            .await
            .map_err(friendly)?;
        self.poke.notify_one();
        Ok(())
    }

    pub async fn answer(&self, id: Option<&str>) -> Result<()> {
        let call = self.find(id, |c| matches!(c.state.as_str(), "incoming" | "waiting"))?;
        if call.state == "waiting" {
            // A second call while talking: HFP answers it by holding the first (AT+CHLD=2).
            let gw = self.gateway()?;
            return self.call_method(gw.as_str(), AG, "HoldAndAnswer").await;
        }
        self.call_method(&call.id, CALL, "Answer").await
    }

    /// Hangs up one call (declining it if it's ringing), or every call when none is named.
    pub async fn hangup(&self, id: Option<&str>) -> Result<()> {
        if id.is_none() {
            let gw = self.gateway()?;
            return self.call_method(gw.as_str(), AG, "HangupAll").await;
        }
        let call = self.find(id, |_| true)?;
        self.call_method(&call.id, CALL, "Hangup").await
    }

    pub async fn dial(&self, number: &str) -> Result<()> {
        let number = clean_number(number)?;
        let gw = self.gateway()?;
        self.session()?
            .call_method(
                Some(TELEPHONY),
                gw.as_str(),
                Some(AG),
                "Dial",
                &(number.as_str(),),
            )
            .await
            .map_err(friendly)?;
        self.poke.notify_one();
        Ok(())
    }

    /// Moves the call's sound from the phone to this computer (PipeWire sends AT+BCC).
    pub async fn audio_here(&self) -> Result<()> {
        let gw = self.gateway()?;
        self.call_method(gw.as_str(), AG_TRANSPORT, "Activate")
            .await
    }
}

fn friendly(e: zbus::Error) -> anyhow::Error {
    debug!("telephony call failed: {e}");
    // PipeWire replies with org.freedesktop.DBus.Error.Failed and the phone's AT error, which zbus
    // turns into an fdo error; other names arrive as MethodError.
    let reason = match &e {
        zbus::Error::MethodError(_, msg, _) => msg.clone(),
        zbus::Error::FDO(f) => zbus::DBusError::description(f.as_ref()).map(str::to_owned),
        _ => None,
    };
    match reason.filter(|m| !m.is_empty()) {
        Some(msg) => anyhow!("Your phone said no: {msg}"),
        None => anyhow!("Your phone didn't respond. Check that it's connected over Bluetooth."),
    }
}

async fn signal_stream(conn: Option<&Connection>, sender: &str) -> Option<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(sender)
        .build()
        .ok()?;
    zbus::MessageStream::for_match_rule(rule, conn?, std::num::NonZeroUsize::new(64))
        .await
        .ok()
}

async fn next(s: &mut Option<zbus::MessageStream>) -> Option<()> {
    match s {
        Some(s) => s.next().await.map(|_| ()),
        // Never fires, so select! waits on the others.
        None => std::future::pending().await,
    }
}

async fn managed(conn: &Connection, dest: &str, path: &str) -> zbus::Result<Objects> {
    // Plain method calls: a Proxy would also fetch and watch properties these objects don't have.
    conn.call_method(
        Some(dest),
        path,
        Some(OBJECT_MANAGER),
        "GetManagedObjects",
        &(),
    )
    .await?
    .body()
    .deserialize()
}

/// None when PipeWire's telephony service isn't on the bus (old PipeWire, or turned off).
async fn read_gateways(conn: &Connection) -> Option<Vec<Gateway>> {
    let objects = managed(conn, TELEPHONY, ROOT).await.ok()?;
    let mut gateways = Vec::new();
    for (path, ifaces) in objects {
        let Some(ag) = ifaces.get(AG) else { continue };
        let transport = ifaces
            .get(AG_TRANSPORT)
            .map(|t| text(t, "State"))
            .unwrap_or_default();
        let mut calls: Vec<RawCall> = managed(conn, TELEPHONY, path.as_str())
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(p, i)| {
                let c = i.get(CALL)?;
                Some(RawCall {
                    path: p.to_string(),
                    number: text(c, "LineIdentification"),
                    name: text(c, "Name"),
                    state: text(c, "State"),
                })
            })
            .collect();
        calls.sort_by(|a, b| a.path.cmp(&b.path));
        gateways.push(Gateway {
            path,
            address: text(ag, "Address"),
            transport,
            calls,
        });
    }
    gateways.sort_by(|a, b| a.path.cmp(&b.path));
    Some(gateways)
}

async fn read_bluetooth(conn: &Connection) -> Bluetooth {
    let Ok(objects) = managed(conn, BLUEZ, "/").await else {
        return Bluetooth::Missing;
    };
    let adapters: Vec<_> = objects.values().filter_map(|i| i.get(ADAPTER)).collect();
    if adapters.is_empty() {
        return Bluetooth::Missing;
    }
    if !adapters.iter().any(|a| flag(a, "Powered")) {
        return Bluetooth::Off;
    }
    let mut devices: Vec<BtDevice> = objects
        .values()
        .filter_map(|i| i.get(DEVICE))
        .map(|d| {
            let uuids = d
                .get("UUIDs")
                .and_then(|v| v.try_clone().ok())
                .and_then(|v| Vec::<String>::try_from(v).ok())
                .unwrap_or_default();
            let name = Some(text(d, "Alias"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| text(d, "Name"));
            BtDevice {
                address: text(d, "Address"),
                name,
                paired: flag(d, "Paired"),
                connected: flag(d, "Connected"),
                hfp: uuids.iter().any(|u| u.eq_ignore_ascii_case(HFP_AG_UUID)),
            }
        })
        .collect();
    devices.sort_by(|a, b| a.address.cmp(&b.address));
    Bluetooth::Devices(devices)
}

#[cfg(test)]
mod tests;
