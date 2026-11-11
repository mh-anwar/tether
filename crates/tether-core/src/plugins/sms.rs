//! Text messages. kdeconnect-android:src/org/kde/kdeconnect/Plugins/SMSPlugin
//! The phone answers `request_conversations` with the newest message of every thread, and
//! `request_conversation` with a page of one thread.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{DeviceData, Outcome, Update};
use crate::Packet;

pub const MESSAGES: &str = "kdeconnect.sms.messages";
pub const REQUEST: &str = "kdeconnect.sms.request";
pub const REQUEST_CONVERSATIONS: &str = "kdeconnect.sms.request_conversations";
pub const REQUEST_CONVERSATION: &str = "kdeconnect.sms.request_conversation";
pub const REQUEST_ATTACHMENT: &str = "kdeconnect.sms.request_attachment";
pub const ATTACHMENT_FILE: &str = "kdeconnect.sms.attachment_file";

/// Android `Telephony.Sms.MESSAGE_TYPE_*`.
pub const TYPE_INBOX: i64 = 1;
pub const TYPE_SENT: i64 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Address {
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    #[serde(default)]
    pub part_id: i64,
    #[serde(default)]
    pub mime_type: String,
    /// Base64 thumbnail sent inline by the phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoded_thumbnail: Option<String>,
    #[serde(default)]
    pub unique_identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    #[serde(rename = "_id", default)]
    pub id: i64,
    #[serde(default)]
    pub thread_id: i64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub addresses: Vec<Address>,
    #[serde(default)]
    pub date: i64,
    #[serde(rename = "type", default)]
    pub kind: i64,
    #[serde(default)]
    pub read: i64,
    #[serde(default)]
    pub sub_id: i64,
    #[serde(default)]
    pub event: i64,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

pub(super) fn handle(data: &mut DeviceData, p: &Packet, out: &mut Outcome) {
    let Some(list) = p.body.get("messages").and_then(Value::as_array) else {
        return;
    };
    // Parse one by one so a single odd message doesn't drop the whole batch.
    let messages: Vec<Message> = list
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    for m in &messages {
        data.threads
            .entry(m.thread_id)
            .or_default()
            .insert(m.id, m.clone());
    }
    if !messages.is_empty() {
        out.updates.push(Update::Messages { messages });
    }
}

/// Newest message of each thread, newest thread first.
pub fn thread_heads(data: &DeviceData) -> Vec<Message> {
    let mut v: Vec<Message> = data
        .threads
        .values()
        .filter_map(|t| t.values().max_by_key(|m| m.date).cloned())
        .collect();
    v.sort_by_key(|m| std::cmp::Reverse(m.date));
    v
}

pub fn thread(data: &DeviceData, thread_id: i64) -> Vec<Message> {
    let mut v: Vec<Message> = data
        .threads
        .get(&thread_id)
        .map(|t| t.values().cloned().collect())
        .unwrap_or_default();
    v.sort_by_key(|m| m.date);
    v
}

/// Sends a text. Protocol version 2 of the request uses an address list (group messages).
pub fn send(addresses: &[String], body: &str, sub_id: Option<i64>) -> Packet {
    let addrs: Vec<Value> = addresses.iter().map(|a| json!({"address": a})).collect();
    let mut b = json!({"version": 2, "addresses": addrs, "messageBody": body});
    if let (Some(id), Some(obj)) = (sub_id, b.as_object_mut()) {
        obj.insert("subID".into(), json!(id));
    }
    Packet::new(REQUEST, b)
}

pub fn request_conversations() -> Packet {
    Packet::new(REQUEST_CONVERSATIONS, json!({}))
}

/// Older messages of one thread: up to `count` messages dated before `before` (ms).
pub fn request_conversation(thread_id: i64, before: Option<i64>, count: Option<i64>) -> Packet {
    let mut b = json!({"threadID": thread_id});
    if let Some(obj) = b.as_object_mut() {
        if let Some(t) = before {
            obj.insert("rangeStartTimestamp".into(), json!(t));
        }
        if let Some(n) = count {
            obj.insert("numberToRequest".into(), json!(n));
        }
    }
    Packet::new(REQUEST_CONVERSATION, b)
}

pub fn request_attachment(part_id: i64, unique_identifier: &str) -> Packet {
    Packet::new(
        REQUEST_ATTACHMENT,
        json!({"part_id": part_id, "unique_identifier": unique_identifier}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages_and_skips_bad_ones() {
        let p = Packet::from_line(
            br#"{"id":1,"type":"kdeconnect.sms.messages","body":{"messages":[
              {"_id":5,"thread_id":2,"body":"hello","addresses":[{"address":"+1555"}],"date":1000,"type":1,"read":0},
              {"_id":"oops","thread_id":[]},
              {"_id":6,"thread_id":2,"body":"later","addresses":[{"address":"+1555"}],"date":2000,"type":2,"read":1}
            ]}}"#,
        );
        let Ok(p) = p else { panic!("parse") };
        let mut d = DeviceData::default();
        let mut out = Outcome::default();
        handle(&mut d, &p, &mut out);
        let heads = thread_heads(&d);
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].body, "later");
        assert_eq!(thread(&d, 2).len(), 2);
    }
}
