//! A stand-in Android phone for development and screenshots. It speaks the real protocol (TLS,
//! v8 identity, pairing) and serves made-up texts, contacts and notifications.
//!
//!     cargo run -p tether-core --example fake_phone -- 127.0.0.1:1716 /tmp/fake-phone
//!
//! It dials the daemon, asks to pair (accept with `tether accept`), then answers requests the way
//! kdeconnect-android does. Sent texts are echoed back as sent, and the contact replies shortly.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{json, Value};
use tether_core::{
    cert::LocalCert,
    identity::{self, Identity, PROTOCOL_VERSION},
    link,
    packet::now_millis,
    plugins,
    tls::TlsContext,
    Packet,
};
use tokio::sync::mpsc;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
struct Msg {
    id: i64,
    thread: i64,
    addrs: Vec<&'static str>,
    body: String,
    date: i64,
    sent: bool,
    read: bool,
}

impl Msg {
    fn json(&self) -> Value {
        json!({
            "_id": self.id, "thread_id": self.thread, "body": self.body, "date": self.date,
            "type": if self.sent { 2 } else { 1 }, "read": if self.read { 1 } else { 0 },
            "addresses": self.addrs.iter().map(|a| json!({"address": a})).collect::<Vec<_>>(),
            "sub_id": 1, "event": 1
        })
    }
}

struct Phone {
    msgs: Vec<Msg>,
    notifications: BTreeMap<String, Value>,
    next_id: i64,
}

const CONTACTS: &[(&str, &str, &str)] = &[
    ("c1", "Maya Chen", "+15555550101"),
    ("c2", "Jordan Alvarez", "+15555550102"),
    ("c3", "Mom", "+15555550103"),
    ("c4", "Priya Raman", "+15555550104"),
    ("c5", "Sam Okafor", "+15555550105"),
];

fn seed() -> Phone {
    let now = now_millis();
    let min = 60_000;
    let hr = 60 * min;
    let day = 24 * hr;
    // (thread, addresses, sent, body, age)
    let script: Vec<(i64, Vec<&'static str>, bool, &str, i64)> = vec![
        (
            1,
            vec!["+15555550101"],
            false,
            "Are you around this weekend?",
            2 * day + 3 * hr,
        ),
        (
            1,
            vec!["+15555550101"],
            true,
            "Mostly! Saturday is busy but Friday works",
            2 * day + 2 * hr,
        ),
        (
            1,
            vec!["+15555550101"],
            false,
            "Perfect. I finally booked the climbing gym for 6",
            day + 5 * hr,
        ),
        (
            1,
            vec!["+15555550101"],
            true,
            "Nice, I'll bring the chalk",
            day + 4 * hr,
        ),
        (
            1,
            vec!["+15555550101"],
            false,
            "Want to try the new ramen place after?",
            14 * min,
        ),
        (
            2,
            vec!["+15555550102"],
            false,
            "Running 10 min late, save me a seat",
            42 * min,
        ),
        (3, vec!["+15555550103"], true, "Boarding now", 5 * hr),
        (
            3,
            vec!["+15555550103"],
            false,
            "Safe flight! Call me when you land",
            5 * hr - 4 * min,
        ),
        (
            4,
            vec!["72045"],
            false,
            "Northside Bank: your verification code is 482913. It expires in 10 minutes.",
            3 * min,
        ),
        (
            5,
            vec!["+15555550104"],
            false,
            "Here are the slides for Thursday",
            day + 2 * hr,
        ),
        (
            5,
            vec!["+15555550104"],
            false,
            "https://example.com/q3-review",
            day + 2 * hr - min,
        ),
        (
            5,
            vec!["+15555550104"],
            true,
            "Thanks, reading them tonight",
            day + hr,
        ),
        (
            6,
            vec!["+15555550101", "+15555550102"],
            false,
            "Who's driving Saturday?",
            3 * hr,
        ),
        (
            6,
            vec!["+15555550101", "+15555550102"],
            true,
            "I can, pick-up at 9?",
            2 * hr + 50 * min,
        ),
        (
            7,
            vec!["+15555550105"],
            false,
            "did you see the game last night",
            3 * day,
        ),
        (
            7,
            vec!["+15555550105"],
            true,
            "the last two minutes were unreal",
            3 * day - 10 * min,
        ),
        (7, vec!["+15555550105"], false, "lol yes", 3 * day - 9 * min),
        (
            8,
            vec!["+15555550199"],
            false,
            "Your prescription is ready for pickup at Elm St.",
            6 * day,
        ),
    ];
    let unread_threads = [1, 2, 4];
    let mut msgs = vec![];
    for (i, (thread, addrs, sent, body, age)) in script.into_iter().enumerate() {
        msgs.push(Msg {
            id: i as i64 + 1,
            thread,
            addrs,
            body: body.into(),
            date: now - age,
            sent,
            read: sent || !unread_threads.contains(&thread),
        });
    }
    let mut notifications = BTreeMap::new();
    let mut add = |id: &str, v: Value| {
        notifications.insert(id.to_string(), v);
    };
    add(
        "0|com.whatsapp|1|leo",
        json!({"id": "0|com.whatsapp|1|leo", "appName": "WhatsApp", "title": "Leo Park",
        "text": "Are you still coming to the game tonight?", "isClearable": true, "time": (now - 6 * min).to_string(),
        "requestReplyId": "reply-leo", "actions": ["Mark as read"]}),
    );
    add(
        "0|com.google.android.gm|2|",
        json!({"id": "0|com.google.android.gm|2|", "appName": "Gmail", "title": "3 new emails",
        "text": "Flight check-in is open, Your receipt from Elm Pharmacy, Weekly digest", "isClearable": true,
        "time": (now - 25 * min).to_string()}),
    );
    add(
        "0|com.google.android.calendar|3|",
        json!({"id": "0|com.google.android.calendar|3|", "appName": "Calendar",
        "title": "Design review", "text": "In 15 minutes, Room 4B", "isClearable": true, "time": (now - 2 * min).to_string()}),
    );
    add(
        "0|com.spotify.music|4|",
        json!({"id": "0|com.spotify.music|4|", "appName": "Spotify", "title": "Now playing",
        "text": "Holocene by Bon Iver", "isClearable": false, "time": (now - 40 * min).to_string()}),
    );
    Phone {
        next_id: 100,
        msgs,
        notifications,
    }
}

#[tokio::main]
async fn main() -> Res<()> {
    let mut args = std::env::args().skip(1);
    let addr: SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:1716".into())
        .parse()?;
    let dir = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "/tmp/tether-fake-phone".into()),
    );
    std::fs::create_dir_all(&dir)?;
    let id_path = dir.join("device_id");
    let device_id = match std::fs::read_to_string(&id_path) {
        Ok(s) if identity::is_valid_device_id(s.trim()) => s.trim().to_string(),
        _ => {
            let id = identity::new_device_id();
            std::fs::write(&id_path, &id)?;
            id
        }
    };
    let cert = LocalCert::load_or_create(&dir, &device_id)?;
    let tls = TlsContext::new(&cert)?;
    // The phone's capabilities mirror the desktop's.
    let me = Identity {
        device_id,
        device_name: "Galaxy S24".into(),
        device_type: "phone".into(),
        protocol_version: PROTOCOL_VERSION,
        incoming_capabilities: plugins::outgoing_capabilities(),
        outgoing_capabilities: plugins::incoming_capabilities(),
        tcp_port: Some(1716),
    };
    let phone = Arc::new(Mutex::new(seed()));
    let paired_flag = dir.join("paired");
    loop {
        match link::connect(addr, &me, &tls).await {
            Ok(l) => {
                println!("connected to {}", l.peer.device_name);
                if let Err(e) = session(l, phone.clone(), &paired_flag).await {
                    println!("session ended: {e}");
                }
            }
            Err(e) => println!("connect failed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn session(l: link::Link, phone: Arc<Mutex<Phone>>, paired_flag: &PathBuf) -> Res<()> {
    let (rd, mut wr) = tokio::io::split(l.stream);
    let (tx, mut rx) = mpsc::channel::<Packet>(64);
    tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            if link::write_packet(&mut wr, &p).await.is_err() {
                break;
            }
        }
    });
    let mut paired = paired_flag.exists();
    if paired {
        greet(&tx).await;
    } else {
        tx.send(Packet::new(
            "kdeconnect.pair",
            json!({"pair": true, "timestamp": now_millis() / 1000}),
        ))
        .await?;
        println!("asked to pair; run `tether accept`");
    }
    let mut rd = tokio::io::BufReader::new(rd);
    while let Some(line) = link::read_line(&mut rd).await? {
        let Ok(p) = Packet::from_line(&line) else {
            continue;
        };
        if p.kind == "kdeconnect.pair" {
            if p.bool("pair") == Some(true) && !paired {
                paired = true;
                std::fs::write(paired_flag, b"1")?;
                println!("paired");
                greet(&tx).await;
            } else if p.bool("pair") == Some(false) {
                paired = false;
                let _ = std::fs::remove_file(paired_flag);
                println!("unpaired");
            }
            continue;
        }
        if !paired {
            continue;
        }
        handle(&p, &phone, &tx).await;
    }
    Ok(())
}

async fn greet(tx: &mpsc::Sender<Packet>) {
    let _ = tx
        .send(Packet::new(
            "kdeconnect.battery",
            json!({"currentCharge": 82, "isCharging": false, "thresholdEvent": 0}),
        ))
        .await;
    let _ = tx
        .send(Packet::new(
            "kdeconnect.connectivity_report",
            json!({"signalStrengths": {"1": {"networkType": "5G", "signalStrength": 3}}}),
        ))
        .await;
}

async fn handle(p: &Packet, phone: &Arc<Mutex<Phone>>, tx: &mpsc::Sender<Packet>) {
    let out: Vec<Packet> = {
        let mut ph = phone.lock().unwrap_or_else(|e| e.into_inner());
        match p.kind.as_str() {
            "kdeconnect.notification.request" if p.bool("request") == Some(true) => ph
                .notifications
                .values()
                .map(|n| {
                    let mut n = n.clone();
                    n["silent"] = json!(true);
                    Packet::new("kdeconnect.notification", n)
                })
                .collect(),
            "kdeconnect.notification.request" => {
                let id = p.str("cancel").unwrap_or("").to_string();
                ph.notifications.remove(&id);
                vec![Packet::new(
                    "kdeconnect.notification",
                    json!({"id": id, "isCancel": true}),
                )]
            }
            "kdeconnect.notification.reply" => {
                println!("reply to notification: {:?}", p.str("message"));
                vec![]
            }
            "kdeconnect.sms.request_conversations" => {
                let mut heads: BTreeMap<i64, &Msg> = BTreeMap::new();
                for m in &ph.msgs {
                    if heads.get(&m.thread).is_none_or(|h| h.date < m.date) {
                        heads.insert(m.thread, m);
                    }
                }
                let list: Vec<Value> = heads.values().map(|m| m.json()).collect();
                vec![Packet::new(
                    "kdeconnect.sms.messages",
                    json!({"messages": list, "version": 2}),
                )]
            }
            "kdeconnect.sms.request_conversation" => {
                let thread = p.i64("threadID").unwrap_or(-1);
                let before = p.i64("rangeStartTimestamp").unwrap_or(i64::MAX);
                let n = p.i64("numberToRequest").unwrap_or(i64::MAX).max(0) as usize;
                let mut list: Vec<&Msg> = ph
                    .msgs
                    .iter()
                    .filter(|m| m.thread == thread && m.date < before)
                    .collect();
                list.sort_by_key(|m| std::cmp::Reverse(m.date));
                let list: Vec<Value> = list.into_iter().take(n).map(|m| m.json()).collect();
                vec![Packet::new(
                    "kdeconnect.sms.messages",
                    json!({"messages": list, "version": 2}),
                )]
            }
            "kdeconnect.sms.request" => {
                let addrs: Vec<String> = p
                    .body
                    .get("addresses")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.get("address")?.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let body = p.str("messageBody").unwrap_or("").to_string();
                let Some(existing) = ph
                    .msgs
                    .iter()
                    .find(|m| m.addrs.iter().map(|s| s.to_string()).collect::<Vec<_>>() == addrs)
                    .cloned()
                else {
                    println!("send to unknown number {addrs:?}: ignored by the fake phone");
                    return;
                };
                ph.next_id += 1;
                let sent = Msg {
                    id: ph.next_id,
                    body,
                    date: now_millis(),
                    sent: true,
                    read: true,
                    ..existing.clone()
                };
                ph.msgs.push(sent.clone());
                // Simulate the contact answering a few seconds later.
                let (phone, tx) = (phone.clone(), tx.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    let reply = {
                        let mut ph = phone.lock().unwrap_or_else(|e| e.into_inner());
                        ph.next_id += 1;
                        let m = Msg {
                            id: ph.next_id,
                            body: "Sounds good!".into(),
                            date: now_millis(),
                            sent: false,
                            read: false,
                            ..existing
                        };
                        ph.msgs.push(m.clone());
                        m
                    };
                    let _ = tx
                        .send(Packet::new(
                            "kdeconnect.sms.messages",
                            json!({"messages": [reply.json()], "version": 2}),
                        ))
                        .await;
                });
                vec![Packet::new(
                    "kdeconnect.sms.messages",
                    json!({"messages": [sent.json()], "version": 2}),
                )]
            }
            "kdeconnect.contacts.request_all_uids_timestamps" => {
                let mut b = json!({"uids": CONTACTS.iter().map(|c| c.0).collect::<Vec<_>>()});
                for c in CONTACTS {
                    b[c.0] = json!(1_700_000_000_000i64);
                }
                vec![Packet::new(
                    "kdeconnect.contacts.response_uids_timestamps",
                    b,
                )]
            }
            "kdeconnect.contacts.request_vcards_by_uid" => {
                let mut b = json!({"uids": CONTACTS.iter().map(|c| c.0).collect::<Vec<_>>()});
                for (uid, name, tel) in CONTACTS {
                    b[*uid] = json!(format!(
                        "BEGIN:VCARD\nVERSION:2.1\nFN:{name}\nTEL;CELL:{tel}\nEND:VCARD"
                    ));
                }
                vec![Packet::new("kdeconnect.contacts.response_vcards", b)]
            }
            "kdeconnect.findmyphone.request" => {
                println!("ringing!");
                vec![]
            }
            "kdeconnect.ping" => {
                println!("ping");
                vec![]
            }
            other => {
                println!("got {other}");
                vec![]
            }
        }
    };
    for o in out {
        let _ = tx.send(o).await;
    }
}
