//! Network packets: one JSON object per line.
//! Format: kdeconnect-kde:core/networkpacket.cpp

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::Result;

/// Packets larger than this are rejected so a hostile peer can't exhaust memory with one line.
pub const MAX_PACKET_LEN: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Packet {
    #[serde(deserialize_with = "de_id", default)]
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub body: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_transfer_info: Option<Map<String, Value>>,
}

/// Older clients sent the id as a string; accept both.
fn de_id<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<i64, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    })
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Packet {
    pub fn new(kind: impl Into<String>, body: Value) -> Self {
        let body = match body {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        Packet {
            id: now_millis(),
            kind: kind.into(),
            body,
            payload_size: None,
            payload_transfer_info: None,
        }
    }

    pub fn to_line(&self) -> Vec<u8> {
        // Serializing a struct of plain JSON values cannot fail.
        let mut v = serde_json::to_vec(self).unwrap_or_default();
        v.push(b'\n');
        v
    }

    pub fn from_line(line: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(trim_ascii(line))?)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(Value::as_str)
    }

    /// Some fields arrive as the strings "true"/"false" (telephony `isCancel`).
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.body.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
            _ => None,
        }
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        match self.body.get(key)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn payload_port(&self) -> Option<u16> {
        let port = self.payload_transfer_info.as_ref()?.get("port")?.as_u64()?;
        u16::try_from(port).ok()
    }
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(start, |e| e + 1);
    &b[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip() {
        let p = Packet::new("kdeconnect.ping", json!({"message": "hi"}));
        let line = p.to_line();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(Packet::from_line(&line).ok(), Some(p));
    }

    #[test]
    fn accepts_string_id_and_missing_body() {
        let p = Packet::from_line(br#"{"id":"123","type":"kdeconnect.ping"}"#).ok();
        assert_eq!(p.map(|p| (p.id, p.body.len())), Some((123, 0)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Packet::from_line(b"not json").is_err());
        assert!(Packet::from_line(br#"{"id":1}"#).is_err());
    }

    #[test]
    fn payload_port() {
        let p = Packet::from_line(
            br#"{"id":1,"type":"kdeconnect.share.request","body":{},"payloadSize":5,"payloadTransferInfo":{"port":1739}}"#,
        )
        .ok();
        assert_eq!(p.and_then(|p| p.payload_port()), Some(1739));
    }
}
