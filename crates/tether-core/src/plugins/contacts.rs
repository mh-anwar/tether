//! Contacts, used to put names on threads and calls.
//! kdeconnect-android:src/org/kde/kdeconnect/Plugins/ContactsPlugin: the phone sends vCards.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::{DeviceData, Outcome, Update};
use crate::Packet;

pub const REQUEST_UIDS: &str = "kdeconnect.contacts.request_all_uids_timestamps";
pub const REQUEST_VCARDS: &str = "kdeconnect.contacts.request_vcards_by_uid";
pub const RESPONSE_UIDS: &str = "kdeconnect.contacts.response_uids_timestamps";
pub const RESPONSE_VCARDS: &str = "kdeconnect.contacts.response_vcards";

#[derive(Debug, Clone, Default)]
pub struct Contacts {
    /// Normalized number -> display name.
    by_number: HashMap<String, String>,
}

impl Contacts {
    pub fn name_for(&self, number: &str) -> Option<String> {
        let n = normalize(number);
        if n.is_empty() {
            return None;
        }
        self.by_number.get(&n).cloned().or_else(|| {
            // Match on the last 9 digits so "+1 555 123 4567" finds "5551234567".
            let tail = tail(&n);
            self.by_number
                .iter()
                .find(|(k, _)| tail.len() >= 7 && tail == super::contacts::tail(k))
                .map(|(_, v)| v.clone())
        })
    }

    pub fn len(&self) -> usize {
        self.by_number.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_number.is_empty()
    }
}

fn normalize(n: &str) -> String {
    n.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn tail(n: &str) -> &str {
    let len = n.len();
    &n[len.saturating_sub(9)..]
}

pub(super) fn handle(data: &mut DeviceData, p: &Packet, out: &mut Outcome) {
    let uids: Vec<String> = p
        .body
        .get("uids")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if p.kind == RESPONSE_UIDS {
        if !uids.is_empty() {
            out.replies
                .push(Packet::new(REQUEST_VCARDS, json!({"uids": uids})));
        }
        return;
    }
    for uid in &uids {
        if let Some(card) = p.str(uid) {
            parse_vcard(card, &mut data.contacts.by_number);
        }
    }
    out.updates.push(Update::Contacts {
        count: data.contacts.len(),
    });
}

/// Minimal vCard reading: FN and TEL lines are all we need.
fn parse_vcard(card: &str, into: &mut HashMap<String, String>) {
    let unfolded = card.replace("\r\n ", "").replace("\n ", "");
    let mut name = None;
    let mut numbers = vec![];
    for line in unfolded.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.split(';').next().unwrap_or("").to_ascii_uppercase();
        match key.as_str() {
            "FN" if !value.trim().is_empty() => name = Some(value.trim().to_string()),
            "TEL" => numbers.push(normalize(value)),
            _ => {}
        }
    }
    if let Some(name) = name {
        for n in numbers.into_iter().filter(|n| !n.is_empty()) {
            into.insert(n, name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcard_lookup() {
        let mut c = Contacts::default();
        parse_vcard(
            "BEGIN:VCARD\nFN:Sam Lee\nTEL;TYPE=CELL:+1 (555) 123-4567\nEND:VCARD",
            &mut c.by_number,
        );
        assert_eq!(c.name_for("5551234567").as_deref(), Some("Sam Lee"));
        assert_eq!(c.name_for("+15551234567").as_deref(), Some("Sam Lee"));
        assert_eq!(c.name_for("999"), None);
    }
}
