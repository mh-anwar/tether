use crate::Packet;
use serde_json::json;
pub const REQUEST: &str = "kdeconnect.lock.request";
pub const TYPE: &str = "kdeconnect.lock";
#[derive(Debug, PartialEq)]
pub enum Request {
    State,
    Set(bool),
}
pub fn parse(p: &Packet) -> Option<Request> {
    if p.kind != REQUEST {
        return None;
    }
    if let Some(v) = p.bool("setLocked") {
        Some(Request::Set(v))
    } else if p.body.contains_key("requestLocked") {
        Some(Request::State)
    } else {
        None
    }
}
pub fn state(locked: bool) -> Packet {
    Packet::new(TYPE, json!({"isLocked":locked}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() {
        assert_eq!(
            parse(&Packet::new(REQUEST, json!({"setLocked":true}))),
            Some(Request::Set(true))
        );
        assert_eq!(
            parse(&Packet::new(REQUEST, json!({"requestLocked":true}))),
            Some(Request::State)
        );
        assert_eq!(state(false).body["isLocked"], false);
        assert_eq!(parse(&Packet::new(REQUEST, json!({}))), None);
    }
}
