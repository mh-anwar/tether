//! Take a photo with the phone's camera. The desktop sends an empty `kdeconnect.photo.request`;
//! the phone opens its camera and answers `kdeconnect.photo` with the picture as a payload, or
//! with `{"cancel": true}` when the user backs out (kdeconnect-kde plugins/photo/photoplugin.cpp,
//! v23.08). Newer Android releases dropped their PhotoPlugin, so the phone only offers this when
//! `kdeconnect.photo.request` is among its incoming capabilities.

use serde_json::json;

use crate::Packet;

pub const REQUEST: &str = "kdeconnect.photo.request";
pub const PHOTO: &str = "kdeconnect.photo";

pub fn request() -> Packet {
    Packet::new(REQUEST, json!({}))
}

/// The user closed the camera without taking a picture.
pub fn is_cancel(p: &Packet) -> bool {
    p.kind == PHOTO && p.bool("cancel") == Some(true)
}

/// What to call the file: the phone's name for it when it sends one, else a plain default.
pub fn file_name(p: &Packet) -> String {
    p.str("filename")
        .filter(|n| !n.trim().is_empty())
        .unwrap_or("Photo from your phone.jpg")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets() {
        let r = request();
        assert_eq!(r.kind, "kdeconnect.photo.request");
        assert!(r.body.is_empty());
        assert!(is_cancel(&Packet::new(PHOTO, json!({"cancel": true}))));
        assert!(!is_cancel(&Packet::new(PHOTO, json!({}))));
        assert!(!is_cancel(&Packet::new("kdeconnect.ping", json!({"cancel": true}))));
        assert_eq!(
            file_name(&Packet::new(PHOTO, json!({}))),
            "Photo from your phone.jpg"
        );
        assert_eq!(
            file_name(&Packet::new(PHOTO, json!({"filename": "IMG_1.jpg"}))),
            "IMG_1.jpg"
        );
    }
}
