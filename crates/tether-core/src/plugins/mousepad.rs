//! Special key numbers match kdeconnect-kde plugins/mousepad/x11remoteinput.cpp.
use crate::Packet;
use serde::{Deserialize, Serialize};
use serde_json::json;
pub const REQUEST: &str = "kdeconnect.mousepad.request";
pub const STATE: &str = "kdeconnect.mousepad.keyboardstate";
pub const ECHO: &str = "kdeconnect.mousepad.echo";
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Input {
    pub dx: f64,
    pub dy: f64,
    pub scroll: bool,
    pub singleclick: bool,
    pub doubleclick: bool,
    pub middleclick: bool,
    pub rightclick: bool,
    pub singlehold: bool,
    pub singlerelease: bool,
    pub key: String,
    pub special_key: u32,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub super_key: bool,
    pub send_ack: bool,
}
pub fn parse(p: &Packet) -> Option<Input> {
    if p.kind != REQUEST {
        return None;
    }
    let mut i: Input = serde_json::from_value(json!(p.body)).ok()?;
    i.super_key = p.bool("super").unwrap_or(false);
    if !i.dx.is_finite() || !i.dy.is_finite() || i.key.len() > 4096 {
        return None;
    }
    i.dx = i.dx.clamp(-32767., 32767.);
    i.dy = i.dy.clamp(-32767., 32767.);
    Some(i)
}
pub fn keyboard(available: bool) -> Packet {
    Packet::new(STATE, json!({"state":available}))
}
pub fn echo(p: &Packet) -> Packet {
    let mut b = p.body.clone();
    b.remove("sendAck");
    b.insert("isAck".into(), json!(true));
    Packet::new(ECHO, json!(b))
}
pub fn special_key(code: u32) -> Option<u32> {
    const MAP: [u32; 33] = [
        0, 0xff08, 0xff09, 0xff0a, 0xff51, 0xff52, 0xff53, 0xff54, 0xff55, 0xff56, 0xff50, 0xff57,
        0xff0d, 0xffff, 0xff1b, 0xff15, 0xff14, 0xffe3, 0xffe9, 0xffe1, 0xffeb, 0xffbe, 0xffbf,
        0xffc0, 0xffc1, 0xffc2, 0xffc3, 0xffc4, 0xffc5, 0xffc6, 0xffc7, 0xffc8, 0xffc9,
    ];
    MAP.get(code as usize).copied().filter(|v| *v != 0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() {
        let p = Packet::new(
            REQUEST,
            json!({"dx":1.5,"key":"é","super":true,"sendAck":true}),
        );
        assert!(parse(&p).is_some_and(|i| i.super_key && i.send_ack && i.dx == 1.5));
        assert_eq!(echo(&p).body["isAck"], true);
        assert!(!echo(&p).body.contains_key("sendAck"));
        assert_eq!(keyboard(false).body["state"], false);
        assert!(parse(&Packet::new(REQUEST, json!({"dx":"bad"}))).is_none());
    }
    #[test]
    fn special_keys() {
        assert_eq!(special_key(0), None);
        assert_eq!(special_key(33), None);
        for code in 1..=32 {
            assert!(special_key(code).is_some());
        }
        assert_eq!(special_key(8), Some(0xff55));
        assert_eq!(special_key(32), Some(0xffc9));
    }
}
