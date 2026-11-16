//! Remote keyboard wire format: Android plugins/remotekeyboard{,ime} and KDE
//! plugins/mousepad/x11remoteinput.cpp (SpecialKeysMap).
use crate::Packet;
use serde::Deserialize;
use serde_json::json;

pub const REQUEST: &str = "kdeconnect.mousepad.request";
pub const STATE: &str = "kdeconnect.mousepad.keyboardstate";
pub const ECHO: &str = "kdeconnect.mousepad.echo";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Key {
    pub key: String,
    #[serde(rename = "specialKey")]
    pub special_key: Option<u8>,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    #[serde(rename = "super")]
    pub super_key: bool,
}

pub fn request(k: &Key) -> Option<Packet> {
    if k.special_key.is_some_and(|s| !(1..=32).contains(&s))
        || (k.key.is_empty() && k.special_key.is_none())
        || k.key.len() > 65536
    {
        return None;
    }
    let mut body = json!({"key": k.key, "shift": k.shift, "ctrl": k.ctrl,
        "alt": k.alt, "super": k.super_key, "sendAck": true});
    if let Some(s) = k.special_key {
        body["specialKey"] = json!(s);
    }
    Some(Packet::new(REQUEST, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_text_and_modified_special_keys() {
        let text = request(&Key {
            key: "héllo".into(),
            ..Key::default()
        })
        .unwrap();
        assert_eq!(text.kind, REQUEST);
        assert_eq!(text.str("key"), Some("héllo"));
        assert_eq!(text.bool("sendAck"), Some(true));
        let key = request(&Key {
            special_key: Some(12),
            ctrl: true,
            ..Key::default()
        })
        .unwrap();
        assert_eq!(key.i64("specialKey"), Some(12));
        assert_eq!(key.bool("ctrl"), Some(true));
        assert!(request(&Key::default()).is_none());
        assert!(request(&Key {
            special_key: Some(33),
            ..Key::default()
        })
        .is_none());
    }
    #[test]
    fn state_and_ack_are_validated() {
        let mut data = super::super::DeviceData::default();
        assert!(
            super::super::handle(&mut data, &Packet::new(STATE, json!({})))
                .updates
                .is_empty()
        );
        let out = super::super::handle(&mut data, &Packet::new(STATE, json!({"state": true})));
        assert!(data.keyboard_active == Some(true) && out.updates.len() == 1);
        assert!(
            super::super::handle(&mut data, &Packet::new(ECHO, json!({"isAck": false})))
                .updates
                .is_empty()
        );
        assert_eq!(
            super::super::handle(
                &mut data,
                &Packet::new(ECHO, json!({"isAck": true, "key": "a"}))
            )
            .updates
            .len(),
            1
        );
    }
}
