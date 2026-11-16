//! Desktop-side effects for requests from a paired phone. Core owns only packet schemas.
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    process::Stdio,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use tether_core::{
    plugins::{self, lock, mousepad, runcommand, systemvolume, Update},
    Event, Packet, PairState, Service,
};
use tokio::process::Command;
use tracing::warn;

pub static OPEN_COMMANDS: AtomicBool = AtomicBool::new(false);
pub static RINGING: AtomicBool = AtomicBool::new(false);
static RING_ID: AtomicU64 = AtomicU64::new(0);
static INPUT_WARNED: AtomicBool = AtomicBool::new(false);

pub async fn output(program: &str, args: &[&str]) -> Result<String> {
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    anyhow::ensure!(result.status.success(), "{program} failed");
    Ok(String::from_utf8_lossy(&result.stdout).trim().into())
}
fn enabled(cfg: &crate::config::Config, kind: &str) -> bool {
    match kind {
        runcommand::REQUEST => cfg.allow_commands,
        mousepad::REQUEST | plugins::presenter::TYPE => cfg.allow_input,
        lock::REQUEST => cfg.allow_lock,
        systemvolume::REQUEST => cfg.allow_volume,
        plugins::FIND_PHONE => cfg.allow_ring,
        _ => true,
    }
}
pub async fn run(service: Service) {
    let mut events = service.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    let mut previous = String::new();
    loop {
        tokio::select! {
        event=events.recv()=>match event {
        Ok(Event::Update{device_id,update:Update::Computer{packet}})=> {
        let cfg=match crate::config::Config::load(){Ok(c)=>c,Err(e)=>{warn!("computer settings: {e}");continue;}};
        if !enabled(&cfg,&packet.kind) { tracing::info!("disabled computer plugin: {}",packet.kind); continue; }
        if let Err(e)=handle(&service,&device_id,&packet,&cfg).await { warn!("computer plugin {}: {e}",packet.kind); }
        },
        Ok(Event::Device(d)) if d.connected && d.pair==PairState::Paired=>send_state(&service,Some(&d.id)).await,
        Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
        _=>{},
        },
        _=tick.tick()=> {
        let mut packets=state_packets().await;
        for packet in &mut packets { packet.id = 0; }
        let signature=serde_json::to_string(&packets).unwrap_or_default();
        if signature!=previous { previous=signature; broadcast(&service,packets,None); }
        }
        }
    }
}
fn broadcast(service: &Service, packets: Vec<Packet>, only: Option<&str>) {
    for d in service
        .devices()
        .into_iter()
        .filter(|d| d.connected && d.pair == PairState::Paired && only.is_none_or(|id| id == d.id))
    {
        for p in &packets {
            let _ = service.send(&d.id, p.clone());
        }
    }
}
async fn state_packets() -> Vec<Packet> {
    let Ok(cfg) = crate::config::Config::load() else {
        return vec![];
    };
    let mut packets = vec![];
    if cfg.allow_commands {
        packets.push(runcommand::list(&cfg.commands));
    }
    if cfg.allow_volume {
        packets.push(systemvolume::list(&sinks().await));
    }
    if cfg.allow_lock {
        if let Some(locked) = locked().await {
            packets.push(lock::state(locked));
        }
    }
    if cfg.allow_input {
        packets.push(mousepad::keyboard(crate::remoteinput::available()));
    }
    if let Some(b) = battery(std::path::Path::new("/sys/class/power_supply")) {
        packets.push(b);
    }
    packets
}
pub async fn send_state(service: &Service, only: Option<&str>) {
    broadcast(service, state_packets().await, only);
}
async fn handle(
    service: &Service,
    id: &str,
    p: &Packet,
    cfg: &crate::config::Config,
) -> Result<()> {
    match p.kind.as_str() {
        runcommand::REQUEST => match runcommand::parse(p) {
            Some(runcommand::Request::List) => service.send(id, runcommand::list(&cfg.commands))?,
            Some(runcommand::Request::Setup) => {
                OPEN_COMMANDS.store(true, Ordering::SeqCst);
                crate::tray::open_window().await;
                crate::api::ui_event(json!({"event":"computer_settings"}));
            }
            Some(runcommand::Request::Run(key)) => {
                if let Some(c) = cfg.commands.iter().find(|c| c.id == key) {
                    let c = c.clone();
                    tokio::spawn(async move {
                        use std::os::unix::process::CommandExt;
                        let mut command = std::process::Command::new("sh");
                        command
                            .args(["-c", &c.command])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null());
                        command.process_group(0);
                        match command.spawn() {
                            Ok(mut child) => {
                                let failed =
                                    tokio::task::spawn_blocking(move || child.wait()).await;
                                if !matches!(failed,Ok(Ok(s)) if s.success()) {
                                    notify(
                                        "Command failed",
                                        &format!("{} did not finish successfully.", c.name),
                                    );
                                }
                            }
                            Err(_) => {
                                notify("Command failed", &format!("{} could not start.", c.name))
                            }
                        }
                    });
                }
            }
            _ => {}
        },
        mousepad::REQUEST | plugins::presenter::TYPE => {
            let input = if p.kind == mousepad::REQUEST {
                mousepad::parse(p)
            } else {
                plugins::presenter::parse(p)
            };
            if let Some(input) = input {
                let result = crate::remoteinput::apply(input.clone()).await;
                if result.is_ok() && input.send_ack {
                    service.send(id, mousepad::echo(p))?;
                }
                if let Err(e) = result {
                    service.send(id, mousepad::keyboard(false))?;
                    if !INPUT_WARNED.swap(true, Ordering::Relaxed) {
                        notify(
                            "Remote input unavailable",
                            "Remote input needs X11 or ydotool on this computer.",
                        );
                        warn!("remote input: {e}");
                    }
                }
            }
        }
        lock::REQUEST => {
            if let Some(lock::Request::Set(true)) = lock::parse(p) {
                if output("loginctl", &["lock-session"]).await.is_err()
                    && output("xdg-screensaver", &["lock"]).await.is_err()
                {
                    if let Some(proxy) = screensaver().await {
                        let _: zbus::Result<()> = proxy.call("Lock", &()).await;
                    }
                }
            }
            if let Some(lock::Request::Set(false)) = lock::parse(p) {
                let _ = output("loginctl", &["unlock-session"]).await;
            }
            if lock::parse(p).is_some() {
                service.send(id, lock::state(locked().await.unwrap_or(false)))?;
            }
        }
        systemvolume::REQUEST => {
            if let Some(systemvolume::Request::Set {
                name,
                volume,
                muted,
                enabled,
            }) = systemvolume::parse(p)
            {
                // Only names from the current sink list can reach external programs.
                if sinks().await.iter().any(|s| s.name == name) {
                    if name == "@DEFAULT_AUDIO_SINK@" {
                        if let Some(v) = volume {
                            let _ = output(
                                "wpctl",
                                &["set-volume", &name, &format!("{}", v as f64 / 100.)],
                            )
                            .await;
                        }
                        if let Some(v) = muted {
                            let _ =
                                output("wpctl", &["set-mute", &name, if v { "1" } else { "0" }])
                                    .await;
                        }
                    } else {
                        if let Some(v) = volume {
                            let _ = output("pactl", &["set-sink-volume", &name, &format!("{v}%")])
                                .await;
                        }
                        if let Some(v) = muted {
                            let _ = output(
                                "pactl",
                                &["set-sink-mute", &name, if v { "1" } else { "0" }],
                            )
                            .await;
                        }
                        if enabled == Some(true) {
                            let _ = output("pactl", &["set-default-sink", &name]).await;
                        }
                    }
                }
            }
            send_state(service, None).await;
        }
        plugins::FIND_PHONE => toggle_ring(),
        _ => {}
    }
    Ok(())
}
fn notify(title: &str, body: &str) {
    let (title, body) = (title.to_owned(), body.to_owned());
    tokio::task::spawn_blocking(move || {
        let _ = notify_rust::Notification::new()
            .appname("Tether")
            .summary(&title)
            .body(&body)
            .show();
    });
}
async fn screensaver() -> Option<zbus::Proxy<'static>> {
    let conn = zbus::Connection::session().await.ok()?;
    zbus::Proxy::new_owned(
        conn,
        "org.freedesktop.ScreenSaver",
        "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    )
    .await
    .ok()
}
async fn locked() -> Option<bool> {
    screensaver().await?.call("GetActive", &()).await.ok()
}
async fn sinks() -> Vec<systemvolume::Sink> {
    if let Ok(text) = output("pactl", &["--format=json", "list", "sinks"]).await {
        let default = output("pactl", &["get-default-sink"])
            .await
            .unwrap_or_default();
        if let Ok(list) = serde_json::from_str::<Vec<Value>>(&text) {
            return list
                .into_iter()
                .filter_map(|v| {
                    let name = v["name"].as_str()?.to_string();
                    let volume = v["volume"]
                        .as_object()
                        .and_then(|m| m.values().next())
                        .and_then(|c| c["value_percent"].as_str())
                        .and_then(|s| s.trim_end_matches('%').parse().ok())
                        .unwrap_or(0);
                    Some(systemvolume::Sink {
                        enabled: name == default,
                        name,
                        description: v["description"].as_str().unwrap_or("").into(),
                        volume,
                        muted: v["mute"].as_bool().unwrap_or(false),
                        max_volume: 150,
                    })
                })
                .collect();
        }
    }
    if let Ok(text) = output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).await {
        let v = text
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.);
        return vec![systemvolume::Sink {
            name: "@DEFAULT_AUDIO_SINK@".into(),
            description: "Default output".into(),
            volume: (v * 100.).round() as i64,
            muted: text.contains("MUTED"),
            max_volume: 150,
            enabled: true,
        }];
    }
    vec![]
}
fn battery(root: &std::path::Path) -> Option<Packet> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        let read = |name: &str| {
            std::fs::read_to_string(p.join(name))
                .ok()
                .map(|v| v.trim().to_string())
        };
        if read("type").as_deref() != Some("Battery")
            || read("scope").as_deref() == Some("Device")
            || read("present").as_deref() == Some("0")
        {
            continue;
        }
        let Some(charge) = read("capacity").and_then(|s| s.parse::<i64>().ok()) else {
            continue;
        };
        let status = read("status").unwrap_or_default();
        return Some(plugins::battery::state(
            charge,
            status == "Charging" || status == "Full",
            status == "Discharging",
        ));
    }
    None
}
pub fn stop_ring() {
    RING_ID.fetch_add(1, Ordering::SeqCst);
    RINGING.store(false, Ordering::SeqCst);
    crate::api::ui_event(json!({"event":"computer_ring","ringing":false}));
}
pub fn toggle_ring() {
    if RINGING.swap(true, Ordering::SeqCst) {
        stop_ring();
        return;
    }
    crate::api::ui_event(json!({"event":"computer_ring","ringing":true}));
    let generation = RING_ID.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::task::spawn_blocking(move || {
        if let Ok(n) = notify_rust::Notification::new()
            .appname("Tether")
            .summary("Find this computer")
            .body("Your phone is looking for this computer.")
            .action("stop", "Found it")
            .timeout(0)
            .show()
        {
            n.wait_for_action(|a| {
                if a == "stop" && RING_ID.load(Ordering::SeqCst) == generation {
                    stop_ring();
                }
            });
        }
    });
    tokio::spawn(async move {
        while RINGING.load(Ordering::SeqCst) && RING_ID.load(Ordering::SeqCst) == generation {
            let sound = [
                "/usr/share/sounds/freedesktop/stereo/phone-incoming-call.oga",
                "/usr/share/sounds/freedesktop/stereo/complete.oga",
                "/usr/share/sounds/freedesktop/stereo/bell.oga",
            ]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists());
            let mut command = if crate::remoteinput::has("canberra-gtk-play") {
                let mut c = Command::new("canberra-gtk-play");
                c.args(["-i", "phone-incoming-call"]);
                c
            } else {
                let mut c = Command::new(if crate::remoteinput::has("paplay") {
                    "paplay"
                } else {
                    "pw-play"
                });
                if let Some(s) = sound {
                    c.arg(s);
                }
                c
            };
            if let Ok(mut child) = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
            {
                loop {
                    tokio::select! {_=child.wait()=>break,_=tokio::time::sleep(Duration::from_millis(100))=>if !RINGING.load(Ordering::SeqCst) || RING_ID.load(Ordering::SeqCst)!=generation {let _=child.kill().await;break;}}
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn battery_presence_and_state() -> Result<()> {
        let root = std::env::temp_dir().join(format!("tether-battery-{}", std::process::id()));
        std::fs::create_dir_all(root.join("BAT0"))?;
        assert!(battery(&root).is_none());
        for (name, value) in [
            ("type", "Battery"),
            ("capacity", "14"),
            ("status", "Discharging"),
        ] {
            std::fs::write(root.join("BAT0").join(name), value)?;
        }
        let packet = battery(&root).ok_or_else(|| anyhow::anyhow!("battery missing"))?;
        assert_eq!(packet.body["thresholdEvent"], 1);
        std::fs::write(root.join("BAT0/scope"), "Device")?;
        assert!(battery(&root).is_none());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn permissions_cover_all_requests() {
        let cfg = crate::config::Config {
            allow_commands: false,
            allow_input: false,
            allow_lock: false,
            allow_volume: false,
            allow_ring: false,
            ..Default::default()
        };
        for kind in [
            runcommand::REQUEST,
            mousepad::REQUEST,
            plugins::presenter::TYPE,
            lock::REQUEST,
            systemvolume::REQUEST,
            plugins::FIND_PHONE,
        ] {
            assert!(!enabled(&cfg, kind));
        }
    }
}
