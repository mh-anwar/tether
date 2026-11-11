//! Phone notifications. kdeconnect-android:src/org/kde/kdeconnect/Plugins/NotificationsPlugin

use serde::Serialize;
use serde_json::{json, Value};

use super::{DeviceData, Outcome, Update};
use crate::Packet;

pub const NOTIFICATION: &str = "kdeconnect.notification";
pub const REQUEST: &str = "kdeconnect.notification.request";
pub const REPLY: &str = "kdeconnect.notification.reply";
pub const ACTION: &str = "kdeconnect.notification.action";

#[derive(Debug, Clone, Default, Serialize)]
pub struct Notification {
    pub id: String,
    pub app: String,
    pub title: String,
    pub text: String,
    pub time: i64,
    pub dismissable: bool,
    /// Present when the app supports inline replies (messaging apps).
    pub reply_id: Option<String>,
    pub actions: Vec<String>,
    /// Already on the phone before we connected; shown in the list but not popped up.
    pub silent: bool,
    pub icon_hash: Option<String>,
    /// Local path once the icon payload has been downloaded.
    pub icon_path: Option<String>,
}

pub(super) fn handle(data: &mut DeviceData, p: &Packet, out: &mut Outcome) {
    let Some(id) = p.str("id").map(String::from) else {
        return;
    };
    if p.bool("isCancel") == Some(true) {
        data.notifications.remove(&id);
        out.updates.push(Update::NotificationRemoved { id });
        return;
    }
    let n = Notification {
        app: p.str("appName").unwrap_or("").into(),
        title: p.str("title").unwrap_or("").into(),
        text: p
            .str("text")
            .or_else(|| p.str("ticker"))
            .unwrap_or("")
            .into(),
        time: p.i64("time").unwrap_or(p.id),
        dismissable: p.bool("isClearable").unwrap_or(true),
        reply_id: p
            .str("requestReplyId")
            .filter(|s| !s.is_empty())
            .map(String::from),
        actions: p
            .body
            .get("actions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        silent: p.bool("silent").unwrap_or(false),
        icon_hash: p.str("payloadHash").map(String::from),
        icon_path: data
            .notifications
            .get(&id)
            .and_then(|old| old.icon_path.clone()),
        id: id.clone(),
    };
    data.notifications.insert(id, n.clone());
    out.updates.push(Update::Notification(n));
}

pub fn dismiss(id: &str) -> Packet {
    Packet::new(REQUEST, json!({"cancel": id}))
}

pub fn reply(reply_id: &str, message: &str) -> Packet {
    Packet::new(
        REPLY,
        json!({"requestReplyId": reply_id, "message": message}),
    )
}

pub fn action(id: &str, action: &str) -> Packet {
    Packet::new(ACTION, json!({"key": id, "action": action}))
}
