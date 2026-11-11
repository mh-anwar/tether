//! Call events. kdeconnect-android:src/org/kde/kdeconnect/Plugins/TelephonyPlugin
//! This only reports calls; audio and answering go over Bluetooth (see docs/calls.md, planned).

use serde::Serialize;
use serde_json::json;

use super::{DeviceData, Outcome, Update};
use crate::Packet;

pub const TELEPHONY: &str = "kdeconnect.telephony";
pub const REQUEST_MUTE: &str = "kdeconnect.telephony.request_mute";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Ringing,
    Talking,
    Missed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Call {
    pub state: CallState,
    pub number: Option<String>,
    pub name: Option<String>,
}

pub(super) fn handle(data: &mut DeviceData, p: &Packet, out: &mut Outcome) {
    let state = match p.str("event") {
        Some("ringing") => CallState::Ringing,
        Some("talking") => CallState::Talking,
        Some("missedCall") => CallState::Missed,
        _ => return, // "sms" events are legacy; texts arrive via the SMS plugin
    };
    if p.bool("isCancel") == Some(true) {
        data.call = None;
        out.updates.push(Update::CallEnded);
        return;
    }
    let number = p.str("phoneNumber").map(String::from);
    let name = p
        .str("contactName")
        .map(String::from)
        .or_else(|| number.as_deref().and_then(|n| data.contacts.name_for(n)));
    let call = Call {
        state,
        number,
        name,
    };
    data.call = (call.state != CallState::Missed).then(|| call.clone());
    out.updates.push(Update::Call(call));
}

pub fn mute_ringer() -> Packet {
    Packet::new(REQUEST_MUTE, json!({}))
}
