//! Desktop MPRIS players exported to the phone. Wire fields follow
//! kdeconnect-kde/plugins/mpriscontrol/mpriscontrolplugin.cpp and Android plugins/mpris/MprisPlugin.kt.
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use tether_core::{
    plugins::{mpris, telephony::CallState, Update},
    Event, Packet, PairState, Service,
};
use zbus::{Connection, OwnedVariant, Proxy};
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
type Props = HashMap<String, OwnedVariant>;
fn text(p: &Props, k: &str) -> String {
    p.get(k)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or_default()
        .into()
}
fn boolean(p: &Props, k: &str) -> bool {
    p.get(k)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}
fn integer(p: &Props, k: &str) -> i64 {
    p.get(k).and_then(|v| i64::try_from(v).ok()).unwrap_or(0)
}
async fn proxy<'a>(c: &'a Connection, bus: &'a str) -> zbus::Result<Proxy<'a>> {
    Proxy::new(c, bus, PATH, PLAYER).await
}
async fn properties(c: &Connection, bus: &str) -> zbus::Result<Props> {
    Proxy::new(c, bus, PATH, "org.freedesktop.DBus.Properties")
        .await?
        .call("GetAll", &(PLAYER,))
        .await
}
fn packet(name: &str, p: &Props) -> Packet {
    let meta: Props = p
        .get("Metadata")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Props::try_from(v).ok())
        .unwrap_or_default();
    let artists = meta
        .get("xesam:artist")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default()
        .join(", ");
    let volume = p
        .get("Volume")
        .and_then(|v| f64::try_from(v).ok())
        .unwrap_or(1.0);
    let mut body = json!({"player": name, "title": text(&meta,"xesam:title"), "artist": artists,
        "album": text(&meta,"xesam:album"), "albumArtUrl": text(&meta,"mpris:artUrl"), "length": integer(&meta,"mpris:length") / 1000,
        "pos": integer(p,"Position") / 1000, "isPlaying": text(p,"PlaybackStatus") == "Playing", "volume": (volume * 100.0).round() as i64,
        "loopStatus": text(p,"LoopStatus"), "shuffle": boolean(p,"Shuffle")});
    for (wire, prop) in [
        ("canPause", "CanPause"),
        ("canPlay", "CanPlay"),
        ("canGoNext", "CanGoNext"),
        ("canGoPrevious", "CanGoPrevious"),
        ("canSeek", "CanSeek"),
    ] {
        body[wire] = json!(boolean(p, prop));
    }
    Packet::new(mpris::MPRIS, body)
}
fn list(players: &BTreeMap<String, (String, Value)>) -> Packet {
    Packet::new(
        mpris::MPRIS,
        json!({"playerList": players.keys().collect::<Vec<_>>(), "supportAlbumArtPayload": true}),
    )
}
fn broadcast(s: &Service, p: Packet) {
    for d in s.devices() {
        if d.connected && d.pair == PairState::Paired {
            let _ = s.send(&d.id, p.clone());
        }
    }
}
async fn scan(c: &Connection) -> BTreeMap<String, (String, Value)> {
    let mut players = BTreeMap::new();
    let Ok(dbus) = zbus::fdo::DBusProxy::new(c).await else {
        return players;
    };
    for bus in dbus.list_names().await.unwrap_or_default() {
        let bus = bus.as_str();
        if !bus.starts_with("org.mpris.MediaPlayer2.")
            || bus == "org.mpris.MediaPlayer2.tether"
            || bus.starts_with("org.mpris.MediaPlayer2.kdeconnect")
        {
            continue;
        }
        if let Ok(p) = properties(c, bus).await {
            let identity = match Proxy::new(c, bus, PATH, "org.mpris.MediaPlayer2").await {
                Ok(p) => p
                    .get_property::<String>("Identity")
                    .await
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            let mut name = if identity.is_empty() {
                bus.to_string()
            } else {
                identity
            };
            if players.contains_key(&name) {
                name = format!("{name} ({bus})");
            }
            players.insert(
                name.clone(),
                (bus.into(), Value::Object(packet(&name, &p).body)),
            );
        }
    }
    players
}
async fn request(
    c: &Connection,
    s: &Service,
    id: &str,
    p: &Packet,
    players: &BTreeMap<String, (String, Value)>,
) -> anyhow::Result<()> {
    if p.bool("requestPlayerList") == Some(true) {
        s.send(id, list(players))?;
    }
    let Some((bus, _)) = p.str("player").and_then(|n| players.get(n)) else {
        s.send(id, list(players))?;
        return Ok(());
    };
    let player = proxy(c, bus).await?;
    if let Some(url) = p.str("albumArtUrl") {
        let current = packet(
            p.str("player").unwrap_or_default(),
            &properties(c, bus).await?,
        );
        if current.str("albumArtUrl") == Some(url) {
            if let Ok(url) = url::Url::parse(url) {
                if let Ok(path) = url.to_file_path() {
                    let reply = Packet::new(
                        mpris::MPRIS,
                        json!({"player": p.str("player"), "albumArtUrl": p.str("albumArtUrl"), "transferringAlbumArt": true}),
                    );
                    let svc = s.clone();
                    let id = id.to_string();
                    tokio::spawn(async move {
                        let _ = svc.send_payload(&id, reply, &path).await;
                    });
                }
            }
        }
        return Ok(());
    }
    if let Some(action) = p
        .str("action")
        .filter(|a| ["Play", "Pause", "PlayPause", "Next", "Previous", "Stop"].contains(a))
    {
        player.call::<_, _, ()>(action, &()).await?;
    }
    if let Some(v) = p.i64("setVolume") {
        player
            .set_property("Volume", (v.clamp(0, 100) as f64) / 100.0)
            .await?;
    }
    if let Some(v) = p
        .str("setLoopStatus")
        .filter(|v| ["None", "Track", "Playlist"].contains(v))
    {
        player.set_property("LoopStatus", v).await?;
    }
    if let Some(v) = p.bool("setShuffle") {
        player.set_property("Shuffle", v).await?;
    }
    if let Some(v) = p.i64("Seek") {
        player.call::<_, _, ()>("Seek", &(v,)).await?;
    }
    if let Some(v) = p.i64("SetPosition") {
        let pos = player.get_property::<i64>("Position").await?;
        player
            .call::<_, _, ()>(
                "Seek",
                &(v.max(0).saturating_mul(1000).saturating_sub(pos),),
            )
            .await?;
    }
    if p.bool("requestNowPlaying") == Some(true) || p.bool("requestVolume") == Some(true) {
        s.send(
            id,
            packet(
                p.str("player").unwrap_or_default(),
                &properties(c, bus).await?,
            ),
        )?;
    }
    Ok(())
}
pub async fn run(s: Service) {
    let builder = zbus::conn::Builder::session();
    let Ok(c) = builder
        .method_timeout(std::time::Duration::from_secs(2))
        .build()
        .await
    else {
        return;
    };
    // A single stream watches player arrival, properties and Seeked (Position deliberately emits no property change).
    let Ok(rule) = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .build()
    else {
        return;
    };
    let Ok(mut signals) =
        zbus::MessageStream::for_match_rule(rule, &c, std::num::NonZeroUsize::new(128)).await
    else {
        return;
    };
    let mut events = s.subscribe();
    let mut previous = BTreeMap::new();
    let mut paused: HashMap<String, Option<f64>> = HashMap::new();
    let mut calls = HashSet::new();
    loop {
        let players = match tokio::time::timeout(std::time::Duration::from_secs(4), scan(&c)).await
        {
            Ok(p) => p,
            Err(_) => previous.clone(),
        };
        if players.keys().collect::<Vec<_>>() != previous.keys().collect::<Vec<_>>() {
            broadcast(&s, list(&players));
        }
        for (name, (_, body)) in &players {
            if previous.get(name).map(|(_, v)| v) != Some(body) {
                broadcast(&s, Packet::new(mpris::MPRIS, body.clone()));
            }
        }
        previous = players;
        tokio::select! {
            msg = signals.next() => {
                let Some(Ok(msg)) = msg else { return };
                let h = msg.header();
                if !matches!(h.member().map(|m|m.as_str()),Some("NameOwnerChanged"|"PropertiesChanged"|"Seeked")) { continue; }
            }
            ev = events.recv() => {
                match ev {
                    Ok(Event::Update { device_id, update: Update::DesktopRequest { packet } }) if packet.kind == mpris::REQUEST => {
                        if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5),request(&c,&s,&device_id,&packet,&previous)).await { tracing::debug!("desktop media timed out: {e}"); }
                    }
                    Ok(Event::Device(d)) => { if d.connected && d.pair == PairState::Paired { let _ = s.send(&d.id,list(&previous)); } else { calls.remove(&d.id); } }
                    Ok(Event::Update { device_id, update: Update::Call(call) }) => {
                        let cfg = crate::config::Config::load().unwrap_or_default();
                        let trigger = match call.state { CallState::Ringing => cfg.pause_media == "ringing", CallState::Talking => cfg.pause_media != "off", _ => false };
                        if trigger {
                            calls.insert(device_id);
                            for (bus,body) in previous.values() {
                                if paused.contains_key(bus) || body.get("isPlaying") != Some(&json!(true)) { continue; }
                                let Ok(p) = proxy(&c,bus).await else { continue };
                                if cfg.mute_media { let v = p.get_property::<f64>("Volume").await.unwrap_or(1.0); if p.set_property("Volume",0.0f64).await.is_ok() { paused.insert(bus.clone(),Some(v)); } }
                                else if p.call::<_,_,()>(if body.get("canPause") == Some(&json!(true)) { "Pause" } else { "Stop" },&()).await.is_ok() { paused.insert(bus.clone(),None); }
                            }
                        }
                    }
                    Ok(Event::Update { device_id, update: Update::CallEnded }) => { calls.remove(&device_id); }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    _ => {}
                }
                if calls.is_empty() { for (bus,volume) in paused.drain() { if let Ok(p) = proxy(&c,&bus).await { if let Some(v) = volume { let _ = p.set_property("Volume",v).await; } else { let _ = p.call::<_,_,()>("Play",&()).await; } } } }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_units_and_capabilities() {
        let p = HashMap::from([
            ("Position".into(), OwnedVariant::from(1234000i64)),
            ("Volume".into(), OwnedVariant::from(0.42f64)),
            ("CanSeek".into(), OwnedVariant::from(true)),
        ]);
        let packet = packet("Test", &p);
        assert_eq!(packet.i64("pos"), Some(1234));
        assert_eq!(packet.i64("volume"), Some(42));
        assert_eq!(packet.bool("canSeek"), Some(true));
        assert_eq!(
            list(&BTreeMap::new()).bool("supportAlbumArtPayload"),
            Some(true)
        );
    }
}
