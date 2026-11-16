//! kdeconnect-kde plugins/runcommand/runcommandplugin.cpp: commandList is a JSON string containing an object keyed by id.
use crate::Packet;
use serde::{Deserialize, Serialize};
use serde_json::json;
pub const REQUEST: &str = "kdeconnect.runcommand.request";
pub const TYPE: &str = "kdeconnect.runcommand";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub command: String,
}
#[derive(Debug, PartialEq)]
pub enum Request {
    List,
    Run(String),
    Setup,
}
pub fn parse(p: &Packet) -> Option<Request> {
    if p.kind != REQUEST {
        return None;
    }
    if p.bool("requestCommandList") == Some(true) {
        Some(Request::List)
    } else if let Some(key) = p.str("key") {
        Some(Request::Run(key.into()))
    } else if p.body.contains_key("setup") {
        Some(Request::Setup)
    } else {
        None
    }
}
pub fn list(commands: &[Command]) -> Packet {
    let list: serde_json::Map<String, serde_json::Value> = commands
        .iter()
        .map(|c| (c.id.clone(), json!({"name":c.name,"command":c.command})))
        .collect();
    Packet::new(
        TYPE,
        json!({"commandList":json!(list).to_string(),"canAddCommand":true}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() -> serde_json::Result<()> {
        assert_eq!(
            parse(&Packet::new(REQUEST, json!({"key":"a"}))),
            Some(Request::Run("a".into()))
        );
        assert_eq!(
            parse(&Packet::new(REQUEST, json!({"requestCommandList":true}))),
            Some(Request::List)
        );
        assert_eq!(
            parse(&Packet::new(REQUEST, json!({"setup":true}))),
            Some(Request::Setup)
        );
        assert_eq!(parse(&Packet::new(REQUEST, json!({"key":3}))), None);
        let packet = list(&[Command {
            id: "a".into(),
            name: "Test".into(),
            command: "true".into(),
        }]);
        assert!(packet.body["commandList"].is_string());
        let decoded: serde_json::Value =
            serde_json::from_str(packet.str("commandList").unwrap_or(""))?;
        assert_eq!(decoded["a"]["name"], "Test");
        Ok(())
    }
}
