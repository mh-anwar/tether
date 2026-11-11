//! Identity packets and device ids.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{packet::Packet, Error, Result};

pub const PROTOCOL_VERSION: i64 = 8;
pub const IDENTITY: &str = "kdeconnect.identity";
pub const UDP_PORT: u16 = 1716;
/// kdeconnect-kde:core/backends/lan/lanlinkprovider.h
pub const MIN_TCP_PORT: u16 = 1716;
pub const MAX_TCP_PORT: u16 = 1764;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub device_id: String,
    pub device_name: String,
    #[serde(default = "default_type")]
    pub device_type: String,
    #[serde(default)]
    pub protocol_version: i64,
    #[serde(default)]
    pub incoming_capabilities: Vec<String>,
    #[serde(default)]
    pub outgoing_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_port: Option<u16>,
}

fn default_type() -> String {
    "phone".into()
}

impl Identity {
    pub fn to_packet(&self) -> Packet {
        let body = serde_json::to_value(self).unwrap_or(Value::Null);
        Packet::new(IDENTITY, body)
    }

    pub fn from_packet(p: &Packet) -> Result<Self> {
        if p.kind != IDENTITY {
            return Err(Error::Protocol(format!(
                "expected identity, got {}",
                p.kind
            )));
        }
        let mut id: Identity = serde_json::from_value(Value::Object(p.body.clone()))?;
        if !is_valid_device_id(&id.device_id) {
            return Err(Error::Protocol("invalid device id".into()));
        }
        id.device_name = sanitize_name(&id.device_name);
        Ok(id)
    }
}

/// kdeconnect-kde:core/deviceinfo.h `isValidIdentifier`: ^[a-zA-Z0-9_-]{32,38}$
pub fn is_valid_device_id(id: &str) -> bool {
    (32..=38).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// kdeconnect-kde generates a UUID with '-' replaced by '_'.
pub fn new_device_id() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "_")
}

/// kdeconnect-kde:core/deviceinfo.h `filterName`: drop quoting/punctuation, cap at 32 chars.
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !"\"',;:.!?()[]<>".contains(*c) && !c.is_control())
        .take(32)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "Unnamed device".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let id = new_device_id();
        assert!(is_valid_device_id(&id), "{id}");
        assert!(!is_valid_device_id("short"));
        assert!(!is_valid_device_id(&"a/".repeat(18)));
    }

    #[test]
    fn identity_roundtrip_and_sanitize() {
        let id = Identity {
            device_id: new_device_id(),
            device_name: "Pixel (work)!".into(),
            device_type: "phone".into(),
            protocol_version: 8,
            incoming_capabilities: vec!["kdeconnect.ping".into()],
            outgoing_capabilities: vec![],
            tcp_port: Some(1716),
        };
        let back = Identity::from_packet(&id.to_packet()).ok();
        assert_eq!(back.map(|b| b.device_name), Some("Pixel work".into()));
    }
}
