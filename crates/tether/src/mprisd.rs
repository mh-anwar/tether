//! The phone's media player as a desktop MPRIS player, so keyboard media keys, `playerctl`, the
//! GNOME and KDE media widgets and i3/sway bar modules control music playing on the phone.
//! Spec: https://specifications.freedesktop.org/mpris-spec/latest/
//!
//! The bus name is held only while the phone reports a player, so an idle phone doesn't show up
//! as an empty player.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tether_core::{
    packet::now_millis,
    plugins::{mpris, mpris::Player, Update},
    Event, PairState, Service,
};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info};
use zbus::{fdo, interface, object_server::SignalEmitter, ObjectPath, OwnedVariant, Variant};

const NAME: &str = "org.mpris.MediaPlayer2.tether";
const PATH: &str = "/org/mpris/MediaPlayer2";

#[derive(Clone, Default)]
struct Current {
    device: String,
    device_name: String,
    player: Player,
}

type Shared = Arc<Mutex<Option<Current>>>;

fn now_of(s: &Shared) -> Option<Current> {
    s.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Where the song is now, extrapolated like the window does (the phone reports it on changes only).
fn position_ms(p: &Player) -> i64 {
    let pos = p.position_ms
        + if p.is_playing && p.position_at > 0 {
            (now_millis() - p.position_at).max(0)
        } else {
            0
        };
    if p.length_ms > 0 {
        pos.min(p.length_ms)
    } else {
        pos
    }
}

struct Root(Shared);

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        match now_of(&self.0) {
            Some(c) => format!("{} on {}", c.player.name, c.device_name),
            None => "Phone".into(),
        }
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec![]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }
}

struct PlayerIface {
    shared: Shared,
    service: Service,
}

impl PlayerIface {
    /// Sends what `make` builds; `None` means the request is to be ignored (MPRIS says so for
    /// stale track ids and out-of-range positions).
    fn send(&self, make: impl FnOnce(&Current) -> Option<tether_core::Packet>) -> fdo::Result<()> {
        let c = now_of(&self.shared).ok_or_else(|| fdo::Error::Failed("Nothing playing".into()))?;
        let Some(p) = make(&c) else { return Ok(()) };
        self.service
            .send(&c.device, p)
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    fn act(&self, action: &str) -> fdo::Result<()> {
        self.send(|c| mpris::action(&c.player.name, action))
    }

    fn get<T>(&self, f: impl FnOnce(&Player) -> T, default: T) -> T {
        now_of(&self.shared)
            .map(|c| f(&c.player))
            .unwrap_or(default)
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn next(&self) -> fdo::Result<()> {
        self.act("Next")
    }
    fn previous(&self) -> fdo::Result<()> {
        self.act("Previous")
    }
    fn pause(&self) -> fdo::Result<()> {
        self.act("Pause")
    }
    fn play_pause(&self) -> fdo::Result<()> {
        self.act("PlayPause")
    }
    fn stop(&self) -> fdo::Result<()> {
        // Phones' players rarely stop cleanly; pausing is what people mean.
        self.act("Pause")
    }
    fn play(&self) -> fdo::Result<()> {
        self.act("Play")
    }
    /// Relative, in microseconds. The phone only takes absolute positions in ms.
    fn seek(&self, offset: i64) -> fdo::Result<()> {
        self.send(|c| {
            let mut to = (position_ms(&c.player) + offset / 1000).max(0);
            if c.player.length_ms > 0 {
                if to > c.player.length_ms {
                    // Seeking past the end means "next song" in MPRIS.
                    return mpris::action(&c.player.name, "Next");
                }
                to = to.min(c.player.length_ms);
            }
            Some(mpris::set_position(&c.player.name, to))
        })
    }
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> fdo::Result<()> {
        self.send(|c| {
            // Ignore requests meant for a song that has already changed (spec requirement).
            let in_range =
                position >= 0 && (c.player.length_ms <= 0 || position / 1000 <= c.player.length_ms);
            (track_id.as_str() == track_path(&c.player) && in_range)
                .then(|| mpris::set_position(&c.player.name, position / 1000))
        })
    }
    fn open_uri(&self, _uri: &str) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Not supported".into()))
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match now_of(&self.shared) {
            Some(c) if c.player.is_playing => "Playing".into(),
            Some(_) => "Paused".into(),
            None => "Stopped".into(),
        }
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        now_of(&self.shared)
            .map(|c| metadata(&c.player))
            .unwrap_or_default()
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.get(|p| position_ms(p) * 1000, 0)
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.get(|p| p.volume.unwrap_or(100) as f64 / 100.0, 1.0)
    }
    #[zbus(property)]
    fn set_volume(&mut self, v: f64) -> zbus::Result<()> {
        let pct = (v.clamp(0.0, 1.0) * 100.0).round() as i64;
        self.send(|c| Some(mpris::set_volume(&c.player.name, pct)))
            .map_err(zbus::Error::from)
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.get(|p| p.can_go_next, false)
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.get(|p| p.can_go_previous, false)
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.get(|p| p.can_play, false)
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.get(|p| p.can_pause, false)
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.get(|p| p.can_seek, false)
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;
}

/// A track id that changes with the song, as MPRIS asks.
fn track_path(p: &Player) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&p.name, &p.title, &p.artist, &p.album).hash(&mut h);
    format!("/app/tether/track/t{:016x}", h.finish())
}

fn metadata(p: &Player) -> HashMap<String, OwnedVariant> {
    let mut m = HashMap::new();
    let mut put = |k: &str, v: Variant<'static>| {
        if let Ok(v) = OwnedVariant::try_from(v) {
            m.insert(k.to_string(), v);
        }
    };
    if let Ok(path) = ObjectPath::try_from(track_path(p)) {
        put("mpris:trackid", Variant::from(path));
    }
    put("xesam:title", Variant::from(p.title.clone()));
    if !p.artist.is_empty() {
        put("xesam:artist", Variant::from(vec![p.artist.clone()]));
    }
    if !p.album.is_empty() {
        put("xesam:album", Variant::from(p.album.clone()));
    }
    if p.length_ms > 0 {
        put("mpris:length", Variant::from(p.length_ms * 1000));
    }
    if let Some(art) = &p.art_path {
        put("mpris:artUrl", Variant::from(file_url(art)));
    }
    m
}

/// `file://` URL with the path percent-encoded, so spaces and `#` don't break it.
fn file_url(path: &str) -> String {
    let mut s = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// The player to show: the first connected phone's playing player, else one with a song.
fn pick(service: &Service) -> Option<Current> {
    for d in service.devices() {
        if !d.connected || d.pair != PairState::Paired {
            continue;
        }
        let Ok(data) = service.device_data(&d.id) else {
            continue;
        };
        let players: Vec<&Player> = data
            .media
            .values()
            .filter(|p| !p.title.is_empty())
            .collect();
        if let Some(p) = players.iter().find(|p| p.is_playing).or(players.first()) {
            return Some(Current {
                device: d.id.clone(),
                device_name: d.name.clone(),
                player: (*p).clone(),
            });
        }
    }
    None
}

pub async fn run(service: Service) {
    let shared: Shared = Arc::default();
    let conn = zbus::conn::Builder::session()
        .serve_at(PATH, Root(shared.clone()))
        .serve_at(
            PATH,
            PlayerIface {
                shared: shared.clone(),
                service: service.clone(),
            },
        )
        .build()
        .await;
    let conn = match conn {
        Ok(c) => c,
        Err(e) => {
            info!("no session bus ({e}); media keys won't reach the phone");
            return;
        }
    };
    let mut events = service.subscribe();
    let mut owned = false;
    loop {
        let next = pick(&service);
        let before = {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *g, next.clone())
        };
        if next.is_some() && !owned {
            match conn.request_name(NAME).await {
                Ok(()) => owned = true,
                Err(e) => debug!("mpris name: {e}"),
            }
        } else if next.is_none() && owned {
            let _ = conn.release_name(NAME).await;
            owned = false;
        }
        if owned {
            if let Err(e) = announce(&conn, before.as_ref(), next.as_ref()).await {
                debug!("mpris signal: {e}");
            }
        }
        // Wait for something that can change the player.
        loop {
            match events.recv().await {
                Ok(Event::Device(_))
                | Ok(Event::Update {
                    update: Update::Media { .. },
                    ..
                })
                | Err(RecvError::Lagged(_)) => break,
                Ok(_) => {}
                Err(RecvError::Closed) => return,
            }
        }
    }
}

async fn announce(
    conn: &zbus::Connection,
    before: Option<&Current>,
    now: Option<&Current>,
) -> zbus::Result<()> {
    let iface = conn
        .object_server()
        .interface::<_, PlayerIface>(PATH)
        .await?;
    let player = iface.get().await;
    let em = iface.signal_emitter();
    // Only what changed: device events (battery, signal) arrive often and change nothing here.
    let old = before.map(|c| &c.player);
    let new = now.map(|c| &c.player);
    let differs = |f: fn(&Player) -> String| old.map(f) != new.map(f);
    if differs(|p| p.is_playing.to_string()) {
        player.playback_status_changed(em).await?;
    }
    if differs(|p| {
        format!(
            "{}|{}|{}|{}|{:?}",
            p.title, p.artist, p.album, p.length_ms, p.art_path
        )
    }) {
        player.metadata_changed(em).await?;
    }
    if differs(|p| format!("{:?}", p.volume)) {
        player.volume_changed(em).await?;
    }
    if differs(|p| {
        format!(
            "{}{}{}{}{}",
            p.can_go_next, p.can_go_previous, p.can_play, p.can_pause, p.can_seek
        )
    }) {
        player.can_go_next_changed(em).await?;
        player.can_go_previous_changed(em).await?;
        player.can_play_changed(em).await?;
        player.can_pause_changed(em).await?;
        player.can_seek_changed(em).await?;
    }
    // A position that jumped within the same song was a seek; tell listeners (spec: Seeked).
    if let (Some(b), Some(n)) = (before, now) {
        let same = track_path(&b.player) == track_path(&n.player);
        if same && (position_ms(&b.player) - n.player.position_ms).abs() > 2000 {
            PlayerIface::seeked(em, n.player.position_ms * 1000).await?;
        }
    }
    if before.map(|c| (&c.player.name, &c.device_name))
        != now.map(|c| (&c.player.name, &c.device_name))
    {
        let root = conn.object_server().interface::<_, Root>(PATH).await?;
        root.get()
            .await
            .identity_changed(root.signal_emitter())
            .await?;
    }
    Ok(())
}
