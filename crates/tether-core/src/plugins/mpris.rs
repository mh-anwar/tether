//! What's playing on the phone, and remote control for it.
//! Phone side: kdeconnect-android plugins/mprisreceiver/MprisReceiverPlugin.kt.
//! Desktop reference: kdeconnect-kde plugins/mprisremote/mprisremoteplugin.cpp.
//!
//! The phone sends `playerList` when media sessions change and per-player metadata on every
//! playback callback; there is no periodic position update, so the UI extrapolates from
//! `position_at`. Positions are milliseconds. Album art arrives as a JPEG payload on a
//! `kdeconnect.mpris` packet with `transferringAlbumArt` (handled in the service).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::json;

use super::{Outcome, Update};
use crate::{packet::now_millis, Packet};

pub const MPRIS: &str = "kdeconnect.mpris";
pub const REQUEST: &str = "kdeconnect.mpris.request";

/// Actions MprisReceiverPlugin.kt accepts.
pub const ACTIONS: &[&str] = &["Play", "Pause", "PlayPause", "Next", "Previous", "Stop"];

#[derive(Debug, Clone, Default, Serialize)]
pub struct Player {
    pub name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub is_playing: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_seek: bool,
    pub position_ms: i64,
    pub length_ms: i64,
    /// When `position_ms` was reported (Unix ms, our clock).
    pub position_at: i64,
    /// 0..=100, when the phone reports it.
    pub volume: Option<i64>,
    /// The phone's key for the current art; empty when there is none.
    pub art_url: String,
    pub art_path: Option<String>,
    #[serde(skip)]
    pub art_requested: bool,
}

pub fn handle(players: &mut BTreeMap<String, Player>, p: &Packet, out: &mut Outcome) {
    if p.bool("transferringAlbumArt") == Some(true) {
        return; // the service downloads the payload and fills in art_path
    }
    let mut changed = false;
    if let Some(list) = p.body.get("playerList").and_then(|v| v.as_array()) {
        let names: Vec<String> = list
            .iter()
            .filter_map(|v| v.as_str())
            .map(String::from)
            .collect();
        players.retain(|k, _| names.contains(k));
        // A phone has a handful of players; don't let a bad list grow without bound.
        for n in names.into_iter().take(32) {
            if !players.contains_key(&n) {
                players.insert(
                    n.clone(),
                    Player {
                        name: n.clone(),
                        ..Default::default()
                    },
                );
                out.replies.push(request_now_playing(&n));
            }
        }
        changed = true;
    }
    if let Some(name) = p.str("player") {
        // Metadata for a player the list hasn't mentioned yet still counts.
        let pl = players.entry(name.to_string()).or_insert_with(|| Player {
            name: name.to_string(),
            ..Default::default()
        });
        let s = |k: &str, f: &mut String| {
            if let Some(v) = p.str(k) {
                *f = v.to_string();
            }
        };
        s("title", &mut pl.title);
        s("artist", &mut pl.artist);
        s("album", &mut pl.album);
        let b = |k: &str, f: &mut bool| {
            if let Some(v) = p.bool(k) {
                *f = v;
            }
        };
        b("isPlaying", &mut pl.is_playing);
        b("canPlay", &mut pl.can_play);
        b("canPause", &mut pl.can_pause);
        b("canGoNext", &mut pl.can_go_next);
        b("canGoPrevious", &mut pl.can_go_previous);
        b("canSeek", &mut pl.can_seek);
        if let Some(pos) = p.i64("pos") {
            pl.position_ms = pos.max(0);
            pl.position_at = now_millis();
        }
        if let Some(len) = p.i64("length") {
            pl.length_ms = len.max(0);
        }
        if let Some(v) = p.i64("volume") {
            pl.volume = Some(v.clamp(0, 100));
        }
        if let Some(url) = p.str("albumArtUrl") {
            if url != pl.art_url {
                pl.art_url = url.to_string();
                pl.art_path = None;
                pl.art_requested = false;
            }
        }
        changed = true;
    }
    if changed {
        out.updates.push(media_update(players));
    }
}

pub fn media_update(players: &BTreeMap<String, Player>) -> Update {
    Update::Media {
        players: players.values().cloned().collect(),
    }
}

pub fn request_player_list() -> Packet {
    Packet::new(REQUEST, json!({"requestPlayerList": true}))
}

pub fn request_now_playing(player: &str) -> Packet {
    Packet::new(
        REQUEST,
        json!({"player": player, "requestNowPlaying": true, "requestVolume": true}),
    )
}

/// The phone answers only when the URL matches its current art, so it can't go stale.
pub fn request_art(player: &str, art_url: &str) -> Packet {
    Packet::new(REQUEST, json!({"player": player, "albumArtUrl": art_url}))
}

pub fn action(player: &str, action: &str) -> Option<Packet> {
    ACTIONS
        .contains(&action)
        .then(|| Packet::new(REQUEST, json!({"player": player, "action": action})))
}

pub fn set_volume(player: &str, volume: i64) -> Packet {
    Packet::new(
        REQUEST,
        json!({"player": player, "setVolume": volume.clamp(0, 100)}),
    )
}

/// Absolute position in ms. The phone ignores relative `Seek`.
pub fn set_position(player: &str, ms: i64) -> Packet {
    Packet::new(REQUEST, json!({"player": player, "SetPosition": ms.max(0)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(body: serde_json::Value) -> Packet {
        Packet::new(MPRIS, body)
    }

    #[test]
    fn player_list_adds_and_removes_players() {
        let mut players = BTreeMap::new();
        let mut out = Outcome::default();
        handle(
            &mut players,
            &pkt(json!({"playerList": ["Spotify", "YouTube"]})),
            &mut out,
        );
        assert_eq!(players.len(), 2);
        // New players are asked what they're playing.
        assert_eq!(out.replies.len(), 2);
        let mut out = Outcome::default();
        handle(
            &mut players,
            &pkt(json!({"playerList": ["YouTube"]})),
            &mut out,
        );
        assert_eq!(players.keys().collect::<Vec<_>>(), ["YouTube"]);
        assert!(out.replies.is_empty());
    }

    #[test]
    fn metadata_is_partial_and_art_change_resets_path() {
        let mut players = BTreeMap::new();
        let mut out = Outcome::default();
        handle(
            &mut players,
            &pkt(
                json!({"player": "Spotify", "title": "Song", "isPlaying": true, "pos": 5000,
                "length": 200000, "volume": 140, "albumArtUrl": "a1"}),
            ),
            &mut out,
        );
        let pl = players.get_mut("Spotify").unwrap();
        assert_eq!(
            (pl.title.as_str(), pl.is_playing, pl.volume),
            ("Song", true, Some(100))
        );
        pl.art_path = Some("/x.jpg".into());
        handle(
            &mut players,
            &pkt(json!({"player": "Spotify", "isPlaying": false})),
            &mut out,
        );
        let pl = &players["Spotify"];
        assert_eq!((pl.title.as_str(), pl.is_playing), ("Song", false));
        assert_eq!(pl.art_path.as_deref(), Some("/x.jpg"));
        handle(
            &mut players,
            &pkt(json!({"player": "Spotify", "albumArtUrl": "a2"})),
            &mut out,
        );
        assert_eq!(players["Spotify"].art_path, None);
    }

    #[test]
    fn unknown_actions_are_refused() {
        assert!(action("Spotify", "PlayPause").is_some());
        assert!(action("Spotify", "rm -rf").is_none());
    }
}
