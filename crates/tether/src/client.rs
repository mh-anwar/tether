//! Command-line client for the daemon's API.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::{unix::OwnedReadHalf, UnixStream},
};

use crate::Cmd;

struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    wr: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}

impl Client {
    async fn connect() -> Result<Self> {
        let path = crate::socket_path();
        let stream = UnixStream::connect(&path)
            .await
            .context("Tether isn't running. Start it with `tether daemon`.")?;
        let (rd, wr) = stream.into_split();
        Ok(Client {
            lines: BufReader::new(rd).lines(),
            wr,
            next: 1,
        })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))?;
        line.push(b'\n');
        self.wr.write_all(&line).await?;
        while let Some(l) = self.lines.next_line().await? {
            let v: Value = serde_json::from_str(&l)?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(e) = v.get("error").and_then(Value::as_str) {
                    bail!("{e}");
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        bail!("Tether stopped while handling the request")
    }

    /// Picks the device: by id, by name prefix, or the only paired one.
    async fn device(&mut self, wanted: &Option<String>) -> Result<String> {
        let devices = self.call("devices", json!({})).await?;
        let list = devices.as_array().cloned().unwrap_or_default();
        let field = |d: &Value, k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(w) = wanted {
            let lw = w.to_lowercase();
            return list
                .iter()
                .find(|d| field(d, "id") == *w || field(d, "name").to_lowercase().starts_with(&lw))
                .map(|d| field(d, "id"))
                .ok_or_else(|| {
                    anyhow!("No phone matches \"{w}\". Run `tether devices` to see them.")
                });
        }
        let paired: Vec<_> = list
            .iter()
            .filter(|d| field(d, "pair") == "paired")
            .collect();
        match paired.as_slice() {
            [one] => Ok(field(one, "id")),
            [] if list.len() == 1 => Ok(field(&list[0], "id")),
            [] => bail!("No paired phone yet. Run `tether devices`, then `tether pair -d <name>`."),
            _ => bail!("More than one phone is paired; choose one with -d <name>."),
        }
    }
}

pub async fn run(device: Option<String>, cmd: Cmd) -> Result<()> {
    let mut c = Client::connect().await?;
    match cmd {
        Cmd::Daemon | Cmd::Autostart { .. } => unreachable!("handled in main"),
        Cmd::Update => {
            let st = c.call("update_check", json!({})).await?;
            if let Some(e) = st.get("error").and_then(Value::as_str) {
                bail!("{e}");
            }
            if st.get("noReleases").and_then(Value::as_bool) == Some(true) {
                bail!("Couldn't see Tether's releases online.");
            }
            let Some(latest) = st.get("latest").and_then(Value::as_str) else {
                println!("Tether {} is up to date.", str_of(&st, "current"));
                return Ok(());
            };
            match str_of(&st, "kind").as_str() {
                "dev" | "store" | "other" => {
                    println!(
                        "Tether {latest} is out. Get it from {}",
                        str_of(&st, "page")
                    );
                    return Ok(());
                }
                _ => {}
            }
            println!("Downloading Tether {latest}.");
            let st = c.call("update_install", json!({})).await?;
            match st.get("installer").and_then(Value::as_str) {
                Some(file) => println!(
                    "Saved the installer to {file}. Install it with:\n  {}",
                    str_of(&st, "installCommand")
                ),
                None => println!("Installed. Restart Tether to use {latest}."),
            }
        }
        Cmd::Devices => {
            let me = c.call("identity", json!({})).await?;
            println!(
                "This computer: {} ({})",
                str_of(&me, "deviceName"),
                str_of(&me, "deviceId")
            );
            let v = c.call("devices", json!({})).await?;
            let list = v.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!(
                    "No phones found yet. Open KDE Connect on the phone, on the same network."
                );
            }
            for d in list {
                let state = match str_of(&d, "pair").as_str() {
                    "paired" => "paired",
                    "requested_by_peer" => "wants to pair",
                    "requested_by_us" => "waiting for the phone",
                    _ => "not paired",
                };
                let conn = if d.get("connected").and_then(Value::as_bool) == Some(true) {
                    "connected"
                } else {
                    "away"
                };
                print!(
                    "{}  {}, {}, {}",
                    str_of(&d, "id"),
                    str_of(&d, "name"),
                    state,
                    conn
                );
                if let Some(k) = d.get("verification_key").and_then(Value::as_str) {
                    print!(", code {k}");
                }
                println!();
            }
        }
        Cmd::Pair => {
            let d = c.device(&device).await?;
            c.call("pair", json!({"device": d})).await?;
            println!("Pairing request sent. Accept it on the phone within 30 seconds.");
        }
        Cmd::Accept => simple(&mut c, &device, "accept", "Paired.").await?,
        Cmd::Reject => simple(&mut c, &device, "reject", "Declined.").await?,
        Cmd::Unpair => simple(&mut c, &device, "unpair", "Forgotten.").await?,
        Cmd::Ping => simple(&mut c, &device, "ping", "Ping sent.").await?,
        Cmd::Ring => simple(&mut c, &device, "ring", "Ringing the phone.").await?,
        Cmd::Mute => simple(&mut c, &device, "mute", "Ringer muted.").await?,
        Cmd::Refresh => {
            c.call("refresh", json!({})).await?;
            println!("Looking for phones.");
        }
        Cmd::Connect { address } => {
            c.call("connect", json!({"address": address})).await?;
            println!("Contacting {address}. Add it to \"addresses\" in ~/.config/tether/config.json to keep it.");
        }
        Cmd::Status => {
            let d = c.device(&device).await?;
            let data = c.call("device_data", json!({"device": d})).await?;
            if let Some(b) = data.get("battery").filter(|b| !b.is_null()) {
                let charging = if b.get("charging").and_then(Value::as_bool) == Some(true) {
                    ", charging"
                } else {
                    ""
                };
                println!(
                    "Battery: {}%{charging}",
                    b.get("charge").and_then(Value::as_i64).unwrap_or(-1)
                );
            }
            if let Some(sig) = data.get("signal").and_then(Value::as_object) {
                for s in sig.values() {
                    println!(
                        "Signal: {} of 4 bars, {}",
                        s.get("strength").and_then(Value::as_i64).unwrap_or(0),
                        str_of(s, "network_type")
                    );
                }
            }
            let notes = data
                .get("notifications")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            println!("Notifications: {}", notes.len());
            for n in notes.values() {
                println!(
                    "  {}: {}  {}",
                    str_of(n, "app"),
                    str_of(n, "title"),
                    str_of(n, "text")
                );
            }
        }
        Cmd::Apps { app } if app.as_deref() == Some("doctor") => {
            let report = c.call("apps_doctor", json!({})).await?;
            println!("{}", report.as_str().unwrap_or_default());
        }
        Cmd::Apps { app } => {
            let st = c.call("apps_status", json!({})).await?;
            if st.get("device").is_none_or(Value::is_null) {
                bail!("The phone isn't connected for apps. Set it up from the Apps page in the Tether window.");
            }
            match app.as_deref() {
                Some("screen") => {
                    let r = c
                        .call("apps_open", json!({"title": "Phone screen"}))
                        .await?;
                    if r["alreadyOpen"] == true {
                        println!("The phone screen is already open.");
                    } else {
                        println!("Opening the phone screen.");
                    }
                }
                Some(want) => {
                    let list = c.call("apps_list", json!({})).await?;
                    let want_l = want.to_lowercase();
                    let apps = list.as_array().cloned().unwrap_or_default();
                    let found = apps
                        .iter()
                        .find(|a| {
                            str_of(a, "name").to_lowercase() == want_l
                                || str_of(a, "package") == want
                        })
                        .or_else(|| {
                            apps.iter()
                                .find(|a| str_of(a, "name").to_lowercase().starts_with(&want_l))
                        });
                    let Some(a) = found else {
                        bail!("No app called \"{want}\" on the phone.");
                    };
                    let name = str_of(a, "name");
                    let r = c
                        .call(
                            "apps_open",
                            json!({"package": str_of(a, "package"), "title": name}),
                        )
                        .await?;
                    if r["alreadyOpen"] == true {
                        println!("{name} is already open in its own window.");
                    } else if r["sharedScreen"] == true {
                        println!("Opening {name} in the phone screen window (Android 9 or older).");
                    } else {
                        println!("Opening {name}.");
                    }
                }
                None => {
                    let list = c.call("apps_list", json!({})).await?;
                    for a in list.as_array().cloned().unwrap_or_default() {
                        println!("{:<32} {}", str_of(&a, "name"), str_of(&a, "package"));
                    }
                }
            }
        }
        Cmd::Media { action } => {
            let d = c.device(&device).await?;
            let data = c.call("device_data", json!({"device": d})).await?;
            let players: Vec<Value> = data
                .get("media")
                .and_then(Value::as_object)
                .map(|m| m.values().cloned().collect())
                .unwrap_or_default();
            let playing = |p: &&Value| p.get("is_playing").and_then(Value::as_bool) == Some(true);
            let Some(pl) = players.iter().find(playing).or(players.first()) else {
                bail!("Nothing is playing on the phone.");
            };
            let name = str_of(pl, "name");
            match action {
                None => {
                    let what = match (str_of(pl, "title"), str_of(pl, "artist")) {
                        (t, _) if t.is_empty() => "nothing yet".to_string(),
                        (t, a) if a.is_empty() => t,
                        (t, a) => format!("{t} by {a}"),
                    };
                    let state = if playing(&pl) { "playing" } else { "paused" };
                    println!("{name}: {what} ({state})");
                }
                Some(a) => {
                    use crate::MediaAction::*;
                    let act = match a {
                        Play => "Play",
                        Pause => "Pause",
                        Toggle => "PlayPause",
                        Next => "Next",
                        Previous => "Previous",
                    };
                    c.call(
                        "media_action",
                        json!({"device": d, "player": name, "action": act}),
                    )
                    .await?;
                }
            }
        }
        Cmd::Threads => {
            let d = c.device(&device).await?;
            let v = c.call("threads", json!({"device": d})).await?;
            let list = v.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("No conversations loaded yet. They arrive a few seconds after the phone connects.");
            }
            for m in list {
                let number = m
                    .pointer("/addresses/0/address")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let name = c
                    .call("contact_name", json!({"device": d, "number": number}))
                    .await?;
                let who = name.as_str().unwrap_or(number);
                println!(
                    "{:>6}  {who}: {}",
                    m.get("thread_id").and_then(Value::as_i64).unwrap_or(0),
                    one_line(&str_of(&m, "body"))
                );
            }
        }
        Cmd::Thread { thread_id } => {
            let d = c.device(&device).await?;
            c.call(
                "request_thread",
                json!({"device": d, "thread_id": thread_id, "count": 50}),
            )
            .await?;
            // Give the phone a moment to answer with the full thread.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let v = c
                .call("thread", json!({"device": d, "thread_id": thread_id}))
                .await?;
            for m in v.as_array().cloned().unwrap_or_default() {
                let who = if m.get("type").and_then(Value::as_i64) == Some(2) {
                    "You"
                } else {
                    "Them"
                };
                println!("{who}: {}", str_of(&m, "body"));
            }
        }
        Cmd::Sms { to, message } => {
            let d = c.device(&device).await?;
            c.call(
                "sms_send",
                json!({"device": d, "addresses": to, "body": message}),
            )
            .await?;
            println!("Handed to the phone for sending.");
        }
        Cmd::Share { target } => {
            let d = c.device(&device).await?;
            if target.starts_with("http://") || target.starts_with("https://") {
                c.call("share_url", json!({"device": d, "url": target}))
                    .await?;
                println!("Link sent.");
            } else {
                let path = std::fs::canonicalize(&target)
                    .with_context(|| format!("Can't find {target}"))?;
                println!("Sending {}...", path.display());
                let r = c
                    .call("share_file", json!({"device": d, "path": path}))
                    .await?;
                println!(
                    "Sent {} bytes.",
                    r.get("bytes").and_then(Value::as_u64).unwrap_or(0)
                );
            }
        }
        Cmd::Type { text } => {
            let d = c.device(&device).await?;
            c.call("keyboard", json!({"device": d, "key": text}))
                .await?;
            println!("Sent to the phone.");
        }
        Cmd::Clip { text } => {
            let d = c.device(&device).await?;
            c.call("clipboard", json!({"device": d, "content": text}))
                .await?;
            println!("Copied to the phone.");
        }
        Cmd::Watch => {
            println!("Watching. Press Ctrl+C to stop.");
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            while let Some(l) = c.lines.next_line().await? {
                // Stop quietly when the reader goes away (e.g. piped into `head`).
                if writeln!(out, "{l}").and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn simple(c: &mut Client, device: &Option<String>, method: &str, done: &str) -> Result<()> {
    let d = c.device(device).await?;
    c.call(method, json!({"device": d})).await?;
    println!("{done}");
    Ok(())
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn one_line(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 60 {
        format!("{}...", flat.chars().take(57).collect::<String>())
    } else {
        flat
    }
}
