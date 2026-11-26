use super::*;
use std::io::{BufRead, BufReader};
use zbus::{interface, Variant};

fn phone(name: &str, address: &str, connected: bool) -> BtDevice {
    BtDevice {
        address: address.into(),
        name: name.into(),
        paired: true,
        connected,
        hfp: true,
    }
}

fn gateway(address: &str, calls: Vec<RawCall>) -> Gateway {
    Gateway {
        path: OwnedObjectPath::try_from(format!("{ROOT}/ag1")).unwrap(),
        address: address.into(),
        transport: "idle".into(),
        calls,
    }
}

fn raw(path: &str, number: &str, state: &str) -> RawCall {
    RawCall {
        path: path.into(),
        number: number.into(),
        name: String::new(),
        state: state.into(),
    }
}

#[test]
fn blockers_in_order() {
    let names = vec!["Galaxy S23".to_string()];
    let none = Bluetooth::Devices(vec![]);
    assert_eq!(summarize(None, &none, &names).0, Some("service"));
    assert_eq!(
        summarize(Some(&[]), &Bluetooth::Missing, &names).0,
        Some("bluetooth")
    );
    assert_eq!(
        summarize(Some(&[]), &Bluetooth::Off, &names).0,
        Some("adapter_off")
    );
    assert_eq!(summarize(Some(&[]), &none, &names).0, Some("pair"));
    // A headset is paired but isn't a phone: still nothing to call with.
    let headset = Bluetooth::Devices(vec![BtDevice {
        hfp: false,
        ..phone("Buds", "11:11", true)
    }]);
    assert_eq!(summarize(Some(&[]), &headset, &names).0, Some("pair"));
    let paired = Bluetooth::Devices(vec![phone("Galaxy S23", "AA:BB", false)]);
    let (b, name, gw) = summarize(Some(&[]), &paired, &names);
    assert_eq!(
        (b, name.as_deref(), gw),
        (Some("connect"), Some("Galaxy S23"), None)
    );
}

#[test]
fn gateway_wins_and_matches_the_phone() {
    let names = vec!["galaxy s23".to_string()];
    let bt = Bluetooth::Devices(vec![
        phone("Pixel", "11:22", true),
        phone("Galaxy  S23", "AA:BB", true),
    ]);
    let gws = [
        gateway("11:22", vec![]),
        Gateway {
            path: OwnedObjectPath::try_from(format!("{ROOT}/ag2")).unwrap(),
            ..gateway("aa:bb", vec![])
        },
    ];
    let (b, name, gw) = summarize(Some(&gws), &bt, &names);
    assert_eq!(b, None);
    assert_eq!(name.as_deref(), Some("Galaxy  S23"));
    assert_eq!(gw.unwrap().address, "aa:bb");
    // BlueZ unreadable: the gateway still works, just without a name.
    let (b, name, gw) = summarize(Some(&gws[..1]), &Bluetooth::Missing, &names);
    assert_eq!(
        (b, name, gw.map(|g| g.address.as_str())),
        (None, None, Some("11:22"))
    );
}

#[test]
fn tracker_keeps_direction_and_answer_time() {
    let mut t = Tracker::default();
    let lookup = |n: &str| (n == "+15550001").then(|| "Alice".to_string());
    let calls = t.calls(&[raw("/c1", "+15550001", "incoming")], &lookup, 10);
    assert_eq!(
        (calls[0].incoming, calls[0].since, calls[0].name.as_deref()),
        (true, None, Some("Alice"))
    );
    let calls = t.calls(&[raw("/c1", "+15550001", "active")], &lookup, 20);
    assert_eq!((calls[0].incoming, calls[0].since), (true, Some(20)));
    // Held and resumed keeps the first answer time.
    t.calls(&[raw("/c1", "+15550001", "held")], &lookup, 30);
    let calls = t.calls(&[raw("/c1", "+15550001", "active")], &lookup, 40);
    assert_eq!(calls[0].since, Some(20));
    // Disconnected calls disappear, and a new call on the same path starts fresh.
    assert!(t
        .calls(&[raw("/c1", "+15550001", "disconnected")], &lookup, 50)
        .is_empty());
    let calls = t.calls(&[raw("/c1", "123", "dialing")], &lookup, 60);
    assert_eq!(
        (calls[0].incoming, calls[0].since, calls[0].name.clone()),
        (false, None, None)
    );
}

#[test]
fn numbers() {
    assert_eq!(clean_number(" (555) 010-9999 ").unwrap(), "5550109999");
    assert_eq!(clean_number("+44 20 7946 0958").unwrap(), "+442079460958");
    assert_eq!(clean_number("*#06#").unwrap(), "*#06#");
    assert!(clean_number("").is_err());
    assert!(clean_number("+").is_err());
    assert!(clean_number("555;ATH").is_err());
    assert!(clean_number("1+2").is_err());
    assert!(clean_number(&"1".repeat(31)).is_err());
}

// ---------- fake PipeWire telephony and BlueZ on a private bus ----------

struct Bus(std::process::Child, String);

impl Bus {
    fn start() -> Option<Bus> {
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let mut line = String::new();
        BufReader::new(child.stdout.take()?)
            .read_line(&mut line)
            .ok()?;
        Some(Bus(child, line.trim().to_string()))
    }
    async fn conn(&self) -> Connection {
        zbus::conn::Builder::address(self.1.as_str())
            .method_timeout(Duration::from_secs(5))
            .build()
            .await
            .unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

type Tree = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, Variant<'static>>>>;

fn path(p: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p.to_string()).unwrap()
}

#[derive(Default)]
struct Fake {
    gateway: bool,
    transport: String,
    calls: Vec<(String, String, String)>, // path, number, state
    dialed: Vec<String>,
    next: u32,
}

type Shared = Arc<Mutex<Fake>>;
const AG1: &str = "/org/pipewire/Telephony/ag1";

struct Manager(Shared);

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Manager {
    fn get_managed_objects(&self) -> Tree {
        let f = self.0.lock().unwrap();
        let mut t = Tree::new();
        if f.gateway {
            let ag = HashMap::from([("Address".to_string(), Variant::from("AA:BB:CC:DD:EE:FF"))]);
            let tr = HashMap::from([("State".to_string(), Variant::from(f.transport.clone()))]);
            t.insert(
                path(AG1),
                HashMap::from([(AG.to_string(), ag), (AG_TRANSPORT.to_string(), tr)]),
            );
        }
        t
    }
}

struct GatewayCalls(Shared);

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl GatewayCalls {
    fn get_managed_objects(&self) -> Tree {
        let f = self.0.lock().unwrap();
        f.calls
            .iter()
            .map(|(p, number, state)| {
                let props = HashMap::from([
                    (
                        "LineIdentification".to_string(),
                        Variant::from(number.clone()),
                    ),
                    ("State".to_string(), Variant::from(state.clone())),
                    ("Name".to_string(), Variant::from("")),
                    ("Multiparty".to_string(), Variant::from(false)),
                ]);
                (path(p), HashMap::from([(CALL.to_string(), props)]))
            })
            .collect()
    }
}

/// Any signal from the service makes the watcher re-read, so one generic change notice is enough.
async fn changed(conn: &Connection, at: &str) {
    let props: HashMap<&str, Variant> = HashMap::new();
    conn.emit_signal(
        None::<&str>,
        at,
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        &(CALL, props, Vec::<&str>::new()),
    )
    .await
    .unwrap();
}

async fn add_call(conn: &Connection, fake: &Shared, number: &str, state: &str) {
    let p = {
        let mut f = fake.lock().unwrap();
        f.next += 1;
        let p = format!("{AG1}/call{}", f.next);
        f.calls.push((p.clone(), number.into(), state.into()));
        p
    };
    conn.object_server()
        .at(p.as_str(), FakeCall(fake.clone(), p.clone()))
        .await
        .unwrap();
    changed(conn, AG1).await;
}

struct AudioGateway(Shared);

#[interface(name = "org.pipewire.Telephony.AudioGateway1")]
impl AudioGateway {
    async fn dial(&self, number: String, #[zbus(connection)] conn: &Connection) {
        self.0.lock().unwrap().dialed.push(number.clone());
        add_call(conn, &self.0, &number, "dialing").await;
    }
    async fn hangup_all(&self, #[zbus(connection)] conn: &Connection) {
        let gone: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .calls
            .drain(..)
            .map(|c| c.0)
            .collect();
        for p in gone {
            let _ = conn.object_server().remove::<FakeCall, _>(p.as_str()).await;
        }
        changed(conn, AG1).await;
    }
}

struct Transport(Shared);

#[interface(name = "org.pipewire.Telephony.AudioGatewayTransport1")]
impl Transport {
    async fn activate(&self, #[zbus(connection)] conn: &Connection) {
        self.0.lock().unwrap().transport = "active".into();
        changed(conn, AG1).await;
    }
}

struct FakeCall(Shared, String);

#[interface(name = "org.pipewire.Telephony.Call1")]
impl FakeCall {
    async fn answer(&self, #[zbus(connection)] conn: &Connection) -> zbus::fdo::Result<()> {
        {
            let mut f = self.0.lock().unwrap();
            let c = f
                .calls
                .iter_mut()
                .find(|c| c.0 == self.1)
                .ok_or_else(|| zbus::fdo::Error::Failed("gone".into()))?;
            if c.2 != "incoming" {
                return Err(zbus::fdo::Error::Failed("Not incoming".into()));
            }
            c.2 = "active".into();
        }
        changed(conn, &self.1).await;
        Ok(())
    }
    async fn hangup(&self, #[zbus(connection)] conn: &Connection) {
        self.0.lock().unwrap().calls.retain(|c| c.0 != self.1);
        changed(conn, AG1).await;
    }
}

struct Bluez;

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Bluez {
    fn get_managed_objects(&self) -> Tree {
        let adapter = HashMap::from([("Powered".to_string(), Variant::from(true))]);
        let device = HashMap::from([
            ("Address".to_string(), Variant::from("AA:BB:CC:DD:EE:FF")),
            ("Alias".to_string(), Variant::from("Galaxy S23")),
            ("Paired".to_string(), Variant::from(true)),
            ("Connected".to_string(), Variant::from(true)),
            (
                "UUIDs".to_string(),
                Variant::from(vec![HFP_AG_UUID.to_string()]),
            ),
        ]);
        HashMap::from([
            (
                path("/org/bluez/hci0"),
                HashMap::from([(ADAPTER.to_string(), adapter)]),
            ),
            (
                path("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"),
                HashMap::from([(DEVICE.to_string(), device)]),
            ),
        ])
    }
}

async fn until(
    rx: &mut tokio::sync::watch::Receiver<Status>,
    what: &str,
    ok: impl Fn(&Status) -> bool,
) -> Status {
    let found = match tokio::time::timeout(Duration::from_secs(4), rx.wait_for(|s| ok(s))).await {
        Ok(Ok(s)) => Some(s.clone()),
        _ => None,
    };
    found.unwrap_or_else(|| {
        panic!(
            "timed out waiting for {what}; last status {:?}",
            *rx.borrow()
        )
    })
}

#[tokio::test]
async fn drives_a_fake_phone_over_dbus() {
    let Some(bus) = Bus::start() else {
        eprintln!("dbus-daemon not installed; skipping");
        return;
    };
    let fake: Shared = Arc::new(Mutex::new(Fake {
        transport: "idle".into(),
        ..Default::default()
    }));
    let bluez = bus.conn().await;
    bluez.object_server().at("/", Bluez).await.unwrap();
    bluez.request_name(BLUEZ).await.unwrap();

    let lookup: Lookup = Box::new(|n| (n == "+15551234567").then(|| "Alice".to_string()));
    let names: Names = Box::new(|| vec!["Galaxy S23".to_string()]);
    // The repo's stand-in wpctl, with its microphone state in a folder of this test's own.
    let dir = std::env::temp_dir().join(format!("tether-calls-mic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = dir.join("muted");
    let wpctl = dir.join("wpctl");
    let stand_in = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testing/calls/wpctl");
    std::fs::write(
        &wpctl,
        format!(
            "#!/bin/sh\nFAKE_WPCTL_STATE={} exec {stand_in} \"$@\"\n",
            state.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wpctl, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mic_state = || std::fs::read_to_string(&state).unwrap_or_default();
    let w = Arc::new(Watcher::new(
        Some(bus.conn().await),
        Some(bus.conn().await),
        lookup,
        names,
        mic::Mic::Wpctl(wpctl.clone()),
    ));
    let mut rx = w.subscribe();
    let bg = w.clone();
    let task = tokio::spawn(async move { bg.watch().await });

    // Nothing serves org.pipewire.Telephony yet.
    w.refresh().await;
    assert_eq!(w.status().blocker, Some("service"));

    // Objects first, then the name, as PipeWire does: a call reaching a name whose objects aren't
    // served yet can go unanswered.
    let tel = bus.conn().await;
    let os = tel.object_server();
    os.at(ROOT, Manager(fake.clone())).await.unwrap();
    os.at(AG1, GatewayCalls(fake.clone())).await.unwrap();
    os.at(AG1, AudioGateway(fake.clone())).await.unwrap();
    os.at(AG1, Transport(fake.clone())).await.unwrap();
    tel.request_name(TELEPHONY).await.unwrap();
    // The service appearing has no signal of its own; the watcher finds it on its next round.
    w.poke.notify_one();
    let s = until(&mut rx, "connect", |s| s.blocker == Some("connect")).await;
    assert_eq!(s.phone.as_deref(), Some("Galaxy S23"));

    fake.lock().unwrap().gateway = true;
    changed(&tel, ROOT).await;
    until(&mut rx, "ready", |s| s.blocker.is_none()).await;

    add_call(&tel, &fake, "+15551234567", "incoming").await;
    let s = until(&mut rx, "ringing", |s| s.calls.len() == 1).await;
    let c = &s.calls[0];
    assert_eq!(
        (c.state.as_str(), c.incoming, c.name.as_deref()),
        ("incoming", true, Some("Alice"))
    );

    w.answer(None).await.unwrap();
    let s = until(&mut rx, "answered", |s| {
        s.calls.first().is_some_and(|c| c.state == "active")
    })
    .await;
    assert!(s.calls[0].since.is_some());
    // The phone refusing shows its reason rather than a generic failure.
    let err = w
        .answer(Some(&s.calls[0].id))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Not incoming"), "{err}");

    w.audio_here().await.unwrap();
    let s = until(&mut rx, "audio", |s| s.audio_here).await;
    assert!(s.can_mute && !s.muted);

    w.mute(true).await.unwrap();
    until(&mut rx, "muted", |s| s.muted).await;
    assert_eq!(mic_state().trim(), "1");

    w.hangup(None).await.unwrap();
    let s = until(&mut rx, "hung up", |s| s.calls.is_empty()).await;
    assert!(!s.muted);
    // Tether muted it for the call, so it gives the microphone back.
    assert_eq!(mic_state().trim(), "0");
    assert!(w.mute(true).await.is_err(), "no call to mute");

    // A microphone the desktop muted before the call shows as muted and stays muted afterwards.
    std::fs::write(&state, "1").unwrap();
    add_call(&tel, &fake, "+15551234567", "incoming").await;
    until(&mut rx, "pre-muted call", |s| s.calls.len() == 1 && s.muted).await;
    w.hangup(None).await.unwrap();
    until(&mut rx, "pre-muted hung up", |s| s.calls.is_empty()).await;
    assert_eq!(mic_state().trim(), "1");
    std::fs::write(&state, "0").unwrap();

    w.dial("(555) 010-9999").await.unwrap();
    let s = until(&mut rx, "dialing", |s| s.calls.len() == 1).await;
    assert_eq!(fake.lock().unwrap().dialed, vec!["5550109999".to_string()]);
    assert_eq!(
        (s.calls[0].state.as_str(), s.calls[0].incoming),
        ("dialing", false)
    );
    w.hangup(Some(&s.calls[0].id)).await.unwrap();
    until(&mut rx, "outgoing ended", |s| s.calls.is_empty()).await;

    assert!(w.dial("call me").await.is_err());
    assert!(w
        .hangup(Some("/org/pipewire/Telephony/ag1/call99"))
        .await
        .is_err());

    // PipeWire going away (restart, Bluetooth off) reports the setup state again.
    drop(tel);
    w.poke.notify_one();
    until(&mut rx, "service gone", |s| s.blocker == Some("service")).await;
    task.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
