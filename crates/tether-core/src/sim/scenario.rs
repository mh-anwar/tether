//! A stand-in Android phone for development and screenshots. It speaks the real protocol (TLS,
//! v8 identity, pairing) and serves made-up texts, contacts and notifications.
//!
//! ```text
//! cargo run -p tether-core --example fake_phone -- 127.0.0.1:1716 /tmp/fake-phone
//! ```
//!
//! It dials the daemon, asks to pair (accept with `tether accept`), then answers requests the way
//! kdeconnect-android does. Sent texts are echoed back as sent, and the contact replies shortly.
//! It also plays a made-up track in a fake "Spotify" player (MprisReceiverPlugin.kt), with
//! album art sent as a payload the same way the desktop serves files.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{packet::now_millis, payload, plugins, plugins::mpris, tls::TlsContext, Packet};
use serde_json::{json, Value};
use tokio::sync::mpsc;

const PLAYER: &str = "Spotify";

struct Track {
    title: &'static str,
    artist: &'static str,
    album: &'static str,
    length: i64,
    /// The phone's key for the art; the desktop echoes it back in `albumArtUrl` requests.
    art: &'static str,
    jpeg: &'static [u8],
}

const TRACKS: &[Track] = &[
    Track {
        title: "Blue in Green",
        artist: "Miles Davis",
        album: "Kind of Blue",
        length: 337_000,
        art: "art-kob",
        jpeg: include_bytes!("../../../../testing/fake-art/art-kob.jpg"),
    },
    Track {
        title: "Clair de Lune",
        artist: "Claude Debussy",
        album: "Suite bergamasque",
        length: 300_000,
        art: "art-debussy",
        jpeg: include_bytes!("../../../../testing/fake-art/art-debussy.jpg"),
    },
    Track {
        title: "Teardrop",
        artist: "Massive Attack",
        album: "Mezzanine",
        length: 330_000,
        art: "art-mezz",
        jpeg: include_bytes!("../../../../testing/fake-art/art-mezz.jpg"),
    },
];

/// Playback state. The position advances on the wall clock while playing, the same way the
/// real player does, so the desktop has to extrapolate between metadata packets.
struct Media {
    track: usize,
    playing: bool,
    volume: i64,
    /// Position in ms as of `anchor` (Unix ms).
    pos_ms: i64,
    anchor: i64,
}

impl Media {
    fn new(now: i64) -> Self {
        Media {
            track: 0,
            playing: true,
            volume: 60,
            pos_ms: 42_000,
            anchor: now,
        }
    }

    fn current(&self) -> &'static Track {
        &TRACKS[self.track]
    }

    fn position(&self, now: i64) -> i64 {
        let p = if self.playing {
            self.pos_ms + (now - self.anchor)
        } else {
            self.pos_ms
        };
        p.clamp(0, self.current().length)
    }

    /// Freezes the position at `now` before the play state changes.
    fn set_playing(&mut self, now: i64, playing: bool) {
        self.pos_ms = self.position(now);
        self.anchor = now;
        self.playing = playing;
    }

    fn seek(&mut self, now: i64, ms: i64) {
        self.pos_ms = ms.clamp(0, self.current().length);
        self.anchor = now;
    }

    fn step(&mut self, now: i64, delta: isize) {
        let n = TRACKS.len() as isize;
        self.track = (self.track as isize + delta).rem_euclid(n) as usize;
        self.pos_ms = 0;
        self.anchor = now;
    }
}

/// A picture that comes with an MMS. The thumbnail rides on the message; the full file is
/// fetched later with `kdeconnect.sms.request_attachment`.
struct Photo {
    part_id: i64,
    unique: &'static str,
    thumb: &'static [u8],
    jpeg: &'static [u8],
}

const PHOTOS: &[Photo] = &[
    Photo {
        part_id: 9001,
        unique: "PART_1731300000_dog.jpg",
        thumb: include_bytes!("../../../../testing/fake-art/thumb-dog.jpg"),
        jpeg: include_bytes!("../../../../testing/fake-art/photo-dog.jpg"),
    },
    Photo {
        part_id: 9002,
        unique: "PART_1731300100_cake.jpg",
        thumb: include_bytes!("../../../../testing/fake-art/thumb-cake.jpg"),
        jpeg: include_bytes!("../../../../testing/fake-art/photo-cake.jpg"),
    },
];

/// Standard base64 with padding, the encoding Android uses for thumbnails.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = |i: usize| u32::from(*chunk.get(i).unwrap_or(&0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[derive(Clone)]
struct Msg {
    id: i64,
    thread: i64,
    addrs: Vec<&'static str>,
    body: String,
    date: i64,
    sent: bool,
    read: bool,
    photo: Option<&'static Photo>,
}

impl Msg {
    fn json(&self) -> Value {
        let mut v = json!({
            "_id": self.id, "thread_id": self.thread, "body": self.body, "date": self.date,
            "type": if self.sent { 2 } else { 1 }, "read": if self.read { 1 } else { 0 },
            "addresses": self.addrs.iter().map(|a| json!({"address": a})).collect::<Vec<_>>(),
            "sub_id": 1, "event": 1
        });
        if let Some(p) = self.photo {
            v["attachments"] = json!([{
                "part_id": p.part_id, "mime_type": "image/jpeg",
                "encoded_thumbnail": base64(p.thumb), "unique_identifier": p.unique
            }]);
        }
        v
    }
}

pub(super) struct Phone {
    msgs: Vec<Msg>,
    notifications: BTreeMap<String, Value>,
    next_id: i64,
    media: Media,
}

const CONTACTS: &[(&str, &str, &str)] = &[
    ("c1", "Maya Chen", "+15555550101"),
    ("c2", "Jordan Alvarez", "+15555550102"),
    ("c3", "Mom", "+15555550103"),
    ("c4", "Priya Raman", "+15555550104"),
    ("c5", "Sam Okafor", "+15555550105"),
];

pub(super) fn seed() -> Phone {
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
            photo: None,
        });
    }
    // Maya's picture and the reply with the cake. Both are older than her latest text, so that
    // text stays the conversation preview.
    let next = msgs.len() as i64 + 1;
    msgs.push(Msg {
        id: next,
        thread: 1,
        addrs: vec!["+15555550101"],
        body: String::new(),
        date: now - 20 * min,
        sent: false,
        read: false,
        photo: Some(&PHOTOS[0]),
    });
    msgs.push(Msg {
        id: next + 1,
        thread: 1,
        addrs: vec!["+15555550101"],
        body: "Happy birthday!".into(),
        date: now - 17 * min,
        sent: true,
        read: true,
        photo: Some(&PHOTOS[1]),
    });
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
    add(
        "0|com.android.systemui|5|",
        json!({"id": "0|com.android.systemui|5|", "appName": "System UI", "title": "Screenshot saved",
        "text": "Tap to see your screenshot", "isClearable": true, "time": (now - min).to_string()}),
    );
    Phone {
        next_id: 100,
        msgs,
        notifications,
        media: Media::new(now),
    }
}

pub(super) async fn greet(tx: &mpsc::Sender<Packet>) {
    // The phone's Remote keyboard starts inactive until the user picks it (RemoteKeyboardPlugin).
    let _ = tx
        .send(Packet::new(
            plugins::mousepad::STATE,
            json!({"state": false}),
        ))
        .await;
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
    let _ = tx.send(player_list()).await;
}

fn player_list() -> Packet {
    Packet::new(
        mpris::MPRIS,
        json!({"playerList": [PLAYER], "supportAlbumArtPayload": true}),
    )
}

/// Metadata as MprisReceiverPlugin.kt sends it on every playback callback.
fn now_playing(m: &Media, now: i64) -> Packet {
    let t = m.current();
    Packet::new(
        mpris::MPRIS,
        json!({
            "player": PLAYER, "title": t.title, "artist": t.artist, "album": t.album,
            "nowPlaying": t.title, "isPlaying": m.playing,
            "canPlay": true, "canPause": true, "canGoNext": true, "canGoPrevious": true,
            "canSeek": true, "pos": m.position(now), "length": t.length,
            "volume": m.volume, "albumArtUrl": t.art
        }),
    )
}

/// Handles `kdeconnect.mpris.request` for everything except album art, which needs the payload
/// connection and is handled in `send_album_art`.
fn mpris_request(m: &mut Media, p: &Packet) -> Vec<Packet> {
    let now = now_millis();
    if p.bool("requestPlayerList") == Some(true) {
        return vec![player_list()];
    }
    if p.str("player") != Some(PLAYER) {
        println!("mpris request for an unknown player: ignored");
        return vec![];
    }
    if p.bool("requestNowPlaying") == Some(true) {
        return vec![now_playing(m, now)];
    }
    if let Some(action) = p.str("action") {
        match action {
            "PlayPause" => m.set_playing(now, !m.playing),
            "Play" => m.set_playing(now, true),
            "Pause" | "Stop" => m.set_playing(now, false),
            "Next" => m.step(now, 1),
            "Previous" => m.step(now, -1),
            other => {
                println!("mpris action {other:?} is not supported: ignored");
                return vec![];
            }
        }
    } else if let Some(v) = p.i64("setVolume") {
        m.volume = v.clamp(0, 100);
    } else if let Some(ms) = p.i64("SetPosition") {
        m.seek(now, ms);
    } else {
        return vec![];
    }
    vec![now_playing(m, now)]
}

/// Sends the current track's cover as a payload, the way `share_file` serves a file: bind a
/// payload port, announce it on the packet, then serve the bytes to the paired desktop.
async fn send_album_art(
    p: &Packet,
    phone: &Arc<Mutex<Phone>>,
    tx: &mpsc::Sender<Packet>,
    tls: &Arc<TlsContext>,
    peer_cert: &[u8],
) {
    let Some(key) = p.str("albumArtUrl") else {
        return;
    };
    let jpeg = {
        let ph = phone.lock().unwrap_or_else(|e| e.into_inner());
        let t = ph.media.current();
        (p.str("player") == Some(PLAYER) && t.art == key).then_some(t.jpeg)
    };
    // The phone only answers for its current art, so a stale request gets no reply.
    let Some(jpeg) = jpeg else {
        println!("album art {key:?} is not the current track: ignored");
        return;
    };
    let body = json!({"player": PLAYER, "transferringAlbumArt": true, "albumArtUrl": key});
    send_payload(mpris::MPRIS, body, jpeg, tx, tls, peer_cert).await;
}

/// Sends `bytes` as a payload the way `share_file` serves a file: bind a payload port, announce
/// it on the packet, then serve the bytes to the paired desktop.
async fn send_payload(
    kind: &str,
    body: Value,
    bytes: &'static [u8],
    tx: &mpsc::Sender<Packet>,
    tls: &Arc<TlsContext>,
    peer_cert: &[u8],
) {
    let Ok((listener, port)) = payload::bind_payload_listener().await else {
        println!("no free payload port for {kind}");
        return;
    };
    let mut pk = Packet::new(kind, body);
    pk.payload_size = Some(bytes.len() as i64);
    pk.payload_transfer_info = Some(
        json!({"port": port})
            .as_object()
            .cloned()
            .unwrap_or_default(),
    );
    let _ = tx.send(pk).await;
    let (tls, peer_cert) = (tls.clone(), peer_cert.to_vec());
    tokio::spawn(async move {
        if let Err(e) = payload::serve(listener, &tls, &peer_cert, bytes).await {
            println!("payload transfer failed: {e}");
        }
    });
}

/// Icons of the apps behind the notifications, made-up stand-ins for what Android sends.
const ICONS: &[(&str, &[u8])] = &[
    (
        "com.whatsapp",
        include_bytes!("../../../../testing/fake-art/icon-whatsapp.png"),
    ),
    (
        "com.spotify",
        include_bytes!("../../../../testing/fake-art/icon-spotify.png"),
    ),
];

/// Resends notifications that have an app icon with the icon as a payload, like Android does
/// (`payloadHash` = MD5 of the icon; here any stable name works).
async fn send_notification_icons(
    phone: &Arc<Mutex<Phone>>,
    tx: &mpsc::Sender<Packet>,
    tls: &Arc<TlsContext>,
    peer_cert: &[u8],
) {
    let notes: Vec<Value> = {
        let ph = phone.lock().unwrap_or_else(|e| e.into_inner());
        ph.notifications.values().cloned().collect()
    };
    for mut n in notes {
        let id = n["id"].as_str().unwrap_or("").to_string();
        let Some((pkg, bytes)) = ICONS.iter().find(|(pkg, _)| id.contains(pkg)) else {
            continue;
        };
        n["silent"] = json!(true);
        n["payloadHash"] = json!(format!("fakeicon{}", pkg.replace('.', "")));
        send_payload("kdeconnect.notification", n, bytes, tx, tls, peer_cert).await;
    }
}

/// Answers `kdeconnect.sms.request_attachment` with the picture's full file. The phone takes a
/// moment to fetch it from storage, so the reply comes a second later, as on a real phone.
async fn send_attachment(
    p: &Packet,
    tx: &mpsc::Sender<Packet>,
    tls: &Arc<TlsContext>,
    peer_cert: &[u8],
) {
    let part = p.i64("part_id");
    let Some(uid) = p.str("unique_identifier") else {
        return;
    };
    let Some(photo) = PHOTOS
        .iter()
        .find(|ph| Some(ph.part_id) == part && ph.unique == uid)
    else {
        println!("attachment {uid:?} is not one of the fake phone's pictures: ignored");
        return;
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let body = json!({"filename": photo.unique});
    send_payload(
        plugins::sms::ATTACHMENT_FILE,
        body,
        photo.jpeg,
        tx,
        tls,
        peer_cert,
    )
    .await;
}

pub(super) async fn handle(
    p: &Packet,
    phone: &Arc<Mutex<Phone>>,
    tx: &mpsc::Sender<Packet>,
    tls: &Arc<TlsContext>,
    peer_cert: &[u8],
) {
    if p.kind == mpris::REQUEST && p.str("albumArtUrl").is_some() {
        send_album_art(p, phone, tx, tls, peer_cert).await;
        return;
    }
    if p.kind == "kdeconnect.notification.request" && p.bool("request") == Some(true) {
        send_notification_icons(phone, tx, tls, peer_cert).await;
    }
    if p.kind == plugins::sms::REQUEST_ATTACHMENT {
        // Spawned so the one-second delay doesn't hold up the rest of the session.
        let (p, tx, tls, peer_cert) = (p.clone(), tx.clone(), tls.clone(), peer_cert.to_vec());
        tokio::spawn(async move { send_attachment(&p, &tx, &tls, &peer_cert).await });
        return;
    }
    if let Some(reply) = record_mousepad(p) {
        let _ = tx.send(reply).await;
        return;
    }
    let out: Vec<Packet> = {
        let mut ph = phone.lock().unwrap_or_else(|e| e.into_inner());
        match p.kind.as_str() {
            mpris::REQUEST => mpris_request(&mut ph.media, p),
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
            // SftpPlugin.kt answer. FAKE_SFTP="ip:port:user:root" points at a test SFTP server
            // (see docs/testing.md); without it the phone stays silent, like file access turned off.
            "kdeconnect.sftp.request" => match std::env::var("FAKE_SFTP") {
                Ok(v) => {
                    let f: Vec<&str> = v.splitn(4, ':').collect();
                    match f.as_slice() {
                        [ip, port, user, root] => vec![Packet::new(
                            "kdeconnect.sftp",
                            json!({"ip": ip, "port": port.parse::<i64>().unwrap_or(0), "user": user,
                                   "password": "fake-one-time", "path": root,
                                   "multiPaths": [root], "pathNames": ["Internal storage"]}),
                        )],
                        _ => vec![],
                    }
                }
                Err(_) => vec![],
            },
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
                    photo: None,
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
                            photo: None,
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

/// Keys this computer types on the phone: logged to `$FAKE_MOUSEPAD_LOG` for the window tests and
/// acknowledged the way RemoteKeyboardPlugin does when `sendAck` is set.
fn record_mousepad(p: &Packet) -> Option<Packet> {
    if p.kind != plugins::mousepad::REQUEST {
        return None;
    }
    if let Ok(path) = std::env::var("FAKE_MOUSEPAD_LOG") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
        {
            let _ = writeln!(file, "{}", Value::Object(p.body.clone()));
        }
    }
    let mut body = p.body.clone();
    body.remove("sendAck");
    body.insert("isAck".into(), json!(true));
    Some(Packet::new(plugins::mousepad::ECHO, Value::Object(body)))
}

#[cfg(test)]
mod mousepad_tests {
    use super::*;
    #[test]
    fn mousepad_ack_adds_a_new_field_without_panicking() {
        let p = Packet::new(
            plugins::mousepad::REQUEST,
            json!({"key": "test", "sendAck": true}),
        );
        let ack = record_mousepad(&p).unwrap_or_else(|| Packet::new("none", json!({})));
        assert_eq!(ack.bool("isAck"), Some(true));
        assert_eq!(ack.str("key"), Some("test"));
        assert!(record_mousepad(&Packet::new("other", json!({}))).is_none());
    }
}
