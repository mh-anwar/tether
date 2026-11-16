use crate::Packet;
use serde::{Deserialize, Serialize};
use serde_json::json;
pub const REQUEST: &str = "kdeconnect.systemvolume.request";
pub const TYPE: &str = "kdeconnect.systemvolume";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sink {
    pub name: String,
    pub description: String,
    pub volume: i64,
    pub muted: bool,
    pub max_volume: i64,
    pub enabled: bool,
}
#[derive(Debug, PartialEq)]
pub enum Request {
    List,
    Set {
        name: String,
        volume: Option<i64>,
        muted: Option<bool>,
        enabled: Option<bool>,
    },
}
pub fn parse(p: &Packet) -> Option<Request> {
    if p.kind != REQUEST {
        return None;
    }
    if p.body.contains_key("requestSinks") {
        return Some(Request::List);
    }
    let name = p.str("name")?;
    if name.is_empty() || name.starts_with('-') {
        return None;
    }
    Some(Request::Set {
        name: name.into(),
        volume: p.i64("volume").map(|v| v.clamp(0, 150)),
        muted: p.bool("muted"),
        enabled: p.bool("enabled"),
    })
}
pub fn list(sinks: &[Sink]) -> Packet {
    Packet::new(TYPE, json!({"sinkList":sinks}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() {
        assert_eq!(
            parse(&Packet::new(REQUEST, json!({"requestSinks":true}))),
            Some(Request::List)
        );
        assert!(parse(&Packet::new(REQUEST, json!({"name":"-bad"}))).is_none());
        assert_eq!(list(&[]).body["sinkList"], json!([]));
        assert!(matches!(
            parse(&Packet::new(REQUEST, json!({"name":"sink","volume":500}))),
            Some(Request::Set {
                volume: Some(150),
                ..
            })
        ));
    }
}
