//! Plugin logic: turns packets from a paired device into typed updates and replies.
//! Desktop effects are performed by the daemon after the service checks pairing.

pub mod battery;
pub mod findmyphone;
pub mod lock;
pub mod mousepad;
pub mod presenter;
pub mod runcommand;
pub mod systemvolume;

pub mod contacts;
pub mod mpris;
pub mod notification;
pub mod sftp;
pub mod sms;
pub mod telephony;

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::json;

use crate::Packet;

pub use notification::Notification;
pub use sms::Message;
pub use telephony::Call;

pub const PING: &str = "kdeconnect.ping";
pub const BATTERY: &str = "kdeconnect.battery";
pub const CLIPBOARD: &str = "kdeconnect.clipboard";
pub const CLIPBOARD_CONNECT: &str = "kdeconnect.clipboard.connect";
pub const SHARE: &str = "kdeconnect.share.request";
pub const FIND_PHONE: &str = "kdeconnect.findmyphone.request";
pub const CONNECTIVITY: &str = "kdeconnect.connectivity_report";

/// Packet types we accept. The phone enables a plugin only when our capabilities match its own.
pub fn incoming_capabilities() -> Vec<String> {
    [
        runcommand::REQUEST,
        mousepad::REQUEST,
        presenter::TYPE,
        lock::REQUEST,
        systemvolume::REQUEST,
        FIND_PHONE,
        PING,
        BATTERY,
        CLIPBOARD,
        CLIPBOARD_CONNECT,
        SHARE,
        SHARE_UPDATE,
        CONNECTIVITY,
        notification::NOTIFICATION,
        sms::MESSAGES,
        sms::ATTACHMENT_FILE,
        telephony::TELEPHONY,
        contacts::RESPONSE_UIDS,
        contacts::RESPONSE_VCARDS,
        mpris::MPRIS,
        mpris::REQUEST,
        notification::REQUEST,
        sftp::SFTP,
        mousepad::STATE,
        mousepad::ECHO,
    ]
    .map(String::from)
    .to_vec()
}

/// Packet types we send.
pub fn outgoing_capabilities() -> Vec<String> {
    [
        runcommand::TYPE,
        mousepad::STATE,
        mousepad::ECHO,
        lock::TYPE,
        systemvolume::TYPE,
        BATTERY,
        PING,
        CLIPBOARD,
        CLIPBOARD_CONNECT,
        SHARE,
        SHARE_UPDATE,
        FIND_PHONE,
        notification::REQUEST,
        notification::REPLY,
        notification::ACTION,
        sms::REQUEST,
        sms::REQUEST_CONVERSATIONS,
        sms::REQUEST_CONVERSATION,
        sms::REQUEST_ATTACHMENT,
        telephony::REQUEST_MUTE,
        contacts::REQUEST_UIDS,
        contacts::REQUEST_VCARDS,
        mpris::REQUEST,
        mpris::MPRIS,
        notification::NOTIFICATION,
        sftp::REQUEST,
        mousepad::REQUEST,
    ]
    .map(String::from)
    .to_vec()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Battery {
    pub charge: i64,
    pub charging: bool,
    pub low: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Signal {
    pub network_type: String,
    /// 0..=4 bars.
    pub strength: i64,
}

/// Everything we know about one paired device. Lives in memory; the phone is the source of truth
/// and resends it on every connection.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceData {
    pub incoming_capabilities: Vec<String>,
    pub outgoing_capabilities: Vec<String>,
    pub readiness: Vec<crate::readiness::FeatureReadiness>,

    pub keyboard_active: Option<bool>,
    pub battery: Option<Battery>,
    #[serde(skip)]
    pub share_count: Option<i64>,
    #[serde(skip)]
    pub share_total: Option<i64>,
    pub signal: BTreeMap<String, Signal>,
    pub notifications: BTreeMap<String, Notification>,
    /// thread id -> message id -> message.
    #[serde(skip)]
    pub threads: BTreeMap<i64, BTreeMap<i64, Message>>,
    pub call: Option<Call>,
    #[serde(skip)]
    pub contacts: contacts::Contacts,
    /// Media players on the phone, by name.
    pub media: BTreeMap<String, mpris::Player>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Update {
    KeyboardState {
        active: bool,
    },
    KeyboardEcho {
        key: String,
        special_key: Option<i64>,
    },
    Computer {
        packet: Packet,
    },
    Battery(Battery),
    Signal {
        signal: BTreeMap<String, Signal>,
    },
    Notification(Notification),
    NotificationRemoved {
        id: String,
    },
    Messages {
        messages: Vec<Message>,
    },
    Call(Call),
    CallEnded,
    Ping {
        message: Option<String>,
    },
    Clipboard {
        content: String,
    },
    SharedText {
        text: String,
    },
    SharedUrl {
        url: String,
    },
    FileReceived {
        path: String,
        open: bool,
    },
    TransferProgress {
        name: String,
        sending: bool,
        bytes: u64,
        total: i64,
        number_of_files: i64,
        total_payload_size: i64,
    },
    DesktopRequest {
        packet: Packet,
    },
    TransferFailed {
        name: String,
        reason: String,
    },
    Contacts {
        count: usize,
    },
    Media {
        players: Vec<mpris::Player>,
    },
    /// A picture message's full file arrived.
    Attachment {
        unique_identifier: String,
        path: String,
    },
}

/// Packets sent whenever a paired device connects, so our view is fresh. Battery and signal have
/// no request packet; the phone sends them on its own when the link comes up.
pub fn on_connected() -> Vec<Packet> {
    vec![
        Packet::new(notification::REQUEST, json!({"request": true})),
        Packet::new(sms::REQUEST_CONVERSATIONS, json!({})),
        Packet::new(contacts::REQUEST_UIDS, json!({})),
        mpris::request_player_list(),
    ]
}

/// What came out of handling one packet.
#[derive(Default)]
pub struct Outcome {
    pub updates: Vec<Update>,
    pub replies: Vec<Packet>,
}

/// Handles packets that carry no payload work. Payload-bearing packets (shared files, icons,
/// attachments) are routed by the service, which owns the network side.
pub fn handle(data: &mut DeviceData, p: &Packet) -> Outcome {
    let mut out = Outcome::default();
    match p.kind.as_str() {
        mousepad::STATE => {
            if let Some(active) = p.bool("state") {
                data.keyboard_active = Some(active);
                out.updates.push(Update::KeyboardState { active });
            }
        }
        mousepad::ECHO => {
            if p.bool("isAck") == Some(true) {
                out.updates.push(Update::KeyboardEcho {
                    key: p.str("key").unwrap_or("").into(),
                    special_key: p.i64("specialKey"),
                });
            }
        }
        runcommand::REQUEST
        | mousepad::REQUEST
        | presenter::TYPE
        | lock::REQUEST
        | systemvolume::REQUEST
        | FIND_PHONE => out.updates.push(Update::Computer { packet: p.clone() }),
        PING => out.updates.push(Update::Ping {
            message: p.str("message").map(String::from),
        }),
        BATTERY => {
            let b = Battery {
                charge: p.i64("currentCharge").unwrap_or(-1),
                charging: p.bool("isCharging").unwrap_or(false),
                // thresholdEvent 1 = battery low (kdeconnect-android BatteryPlugin).
                low: p.i64("thresholdEvent") == Some(1),
            };
            data.battery = Some(b.clone());
            out.updates.push(Update::Battery(b));
        }
        CONNECTIVITY => {
            let mut sig = BTreeMap::new();
            if let Some(map) = p.body.get("signalStrengths").and_then(|v| v.as_object()) {
                for (sub, v) in map {
                    sig.insert(
                        sub.clone(),
                        Signal {
                            network_type: v
                                .get("networkType")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .into(),
                            strength: v
                                .get("signalStrength")
                                .and_then(|x| x.as_i64())
                                .unwrap_or(0)
                                .clamp(0, 4),
                        },
                    );
                }
            }
            data.signal = sig.clone();
            out.updates.push(Update::Signal { signal: sig });
        }
        CLIPBOARD | CLIPBOARD_CONNECT => {
            if let Some(c) = p.str("content") {
                out.updates.push(Update::Clipboard { content: c.into() });
            }
        }
        SHARE => {
            if let Some(t) = p.str("text") {
                out.updates.push(Update::SharedText { text: t.into() });
            } else if let Some(u) = p.str("url") {
                out.updates.push(Update::SharedUrl { url: u.into() });
            }
        }
        notification::NOTIFICATION => notification::handle(data, p, &mut out),
        sms::MESSAGES => sms::handle(data, p, &mut out),
        telephony::TELEPHONY => telephony::handle(data, p, &mut out),
        contacts::RESPONSE_UIDS | contacts::RESPONSE_VCARDS => contacts::handle(data, p, &mut out),
        mpris::MPRIS => mpris::handle(&mut data.media, p, &mut out),
        _ => {}
    }
    out
}

pub fn ping() -> Packet {
    Packet::new(PING, json!({}))
}

pub fn ring_phone() -> Packet {
    Packet::new(FIND_PHONE, json!({}))
}

pub fn clipboard(content: &str) -> Packet {
    Packet::new(CLIPBOARD, json!({"content": content}))
}

pub fn share_text(text: &str) -> Packet {
    Packet::new(SHARE, json!({"text": text}))
}

pub fn share_url(url: &str) -> Packet {
    Packet::new(SHARE, json!({"url": url}))
}

pub const SHARE_UPDATE: &str = "kdeconnect.share.request.update";

/// Announces a batch before its files (Android SharePlugin shows one progress for the lot).
pub fn share_batch(count: usize, total: u64) -> Packet {
    Packet::new(
        SHARE_UPDATE,
        json!({"numberOfFiles": count, "totalPayloadSize": total}),
    )
}

/// One file of a batch; the payload size and port are added when it is sent.
pub fn share_file_packet(name: &str, count: usize, total: u64) -> Packet {
    Packet::new(
        SHARE,
        json!({"filename": name, "open": false, "numberOfFiles": count, "totalPayloadSize": total}),
    )
}

/// Name lookup for a phone number, used to label threads and calls.
pub fn contact_name(data: &DeviceData, number: &str) -> Option<String> {
    data.contacts.name_for(number)
}

pub type ContactMap = HashMap<String, String>;

#[cfg(test)]
mod media_share_tests {
    use super::*;
    #[test]
    fn desktop_plugins_negotiate_both_directions() {
        for kind in [
            mpris::MPRIS,
            mpris::REQUEST,
            notification::NOTIFICATION,
            notification::REQUEST,
            SHARE,
            SHARE_UPDATE,
        ] {
            assert!(incoming_capabilities().iter().any(|k| k == kind));
            assert!(outgoing_capabilities().iter().any(|k| k == kind));
        }
    }
    #[test]
    fn batch_packets_carry_count_and_total() {
        let batch = share_batch(3, 9000);
        assert_eq!(batch.kind, "kdeconnect.share.request.update");
        assert_eq!(
            (batch.i64("numberOfFiles"), batch.i64("totalPayloadSize")),
            (Some(3), Some(9000))
        );
        let file = share_file_packet("a.jpg", 3, 9000);
        assert_eq!(file.kind, SHARE);
        assert_eq!(file.str("filename"), Some("a.jpg"));
        assert_eq!(file.bool("open"), Some(false));
        assert_eq!(
            (file.i64("numberOfFiles"), file.i64("totalPayloadSize")),
            (Some(3), Some(9000))
        );
    }
    #[test]
    fn shared_text_and_url_parse_without_becoming_clipboard_sync() {
        let mut data = DeviceData::default();
        let text = handle(&mut data, &share_text("hello"));
        assert!(matches!(&text.updates[..], [Update::SharedText { text }] if text == "hello"));
        let url = handle(&mut data, &share_url("https://example.org"));
        assert!(
            matches!(&url.updates[..], [Update::SharedUrl { url }] if url == "https://example.org")
        );
        assert!(handle(&mut data, &Packet::new(SHARE, json!({"text": 12})))
            .updates
            .is_empty());
    }
}
