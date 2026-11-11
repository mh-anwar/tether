//! Mirrors phone notifications and calls to the desktop's notification server (works with GNOME,
//! KDE, dunst, mako, ...). Text stays at debug level in logs: it is private.

use std::collections::HashMap;

use tether_core::{plugins::Update, Event, PairState, Service};
use tokio::sync::broadcast::error::RecvError;
use tracing::debug;

pub async fn mirror(service: Service) {
    let mut events = service.subscribe();
    // phone notification id -> desktop notification id, so dismissals on the phone close ours.
    let mut shown: HashMap<String, notify_rust::NotificationHandle> = HashMap::new();
    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        match event {
            Event::Update {
                device_id,
                update: Update::Notification(n),
            } => {
                if n.silent || (n.title.is_empty() && n.text.is_empty()) {
                    continue;
                }
                let key = format!("{device_id}/{}", n.id);
                let replaces = shown.get(&key).map(|h| h.id());
                let summary = if n.title.is_empty() {
                    n.app.clone()
                } else {
                    n.title.clone()
                };
                let mut desk = notify_rust::Notification::new();
                desk.appname(&n.app)
                    .summary(&summary)
                    .body(&escape(&n.text));
                if let Some(icon) = &n.icon_path {
                    desk.icon(icon);
                }
                if let Some(id) = replaces {
                    desk.id(id);
                }
                match show(desk).await {
                    Some(h) => {
                        shown.insert(key, h);
                    }
                    None => debug!("desktop notification failed"),
                }
            }
            Event::Update {
                device_id,
                update: Update::NotificationRemoved { id },
            } => {
                if let Some(h) = shown.remove(&format!("{device_id}/{id}")) {
                    let _ = tokio::task::spawn_blocking(move || h.close()).await;
                }
            }
            Event::Update {
                update: Update::Call(c),
                ..
            } => {
                use tether_core::plugins::telephony::CallState;
                let who = c
                    .name
                    .clone()
                    .or(c.number.clone())
                    .unwrap_or_else(|| "Unknown number".into());
                let title = match c.state {
                    CallState::Ringing => "Incoming call",
                    CallState::Missed => "Missed call",
                    CallState::Talking => continue,
                };
                let mut desk = notify_rust::Notification::new();
                desk.appname("Tether")
                    .summary(title)
                    .body(&escape(&who))
                    .icon("call-start");
                show(desk).await;
            }
            Event::Device(d) if d.pair == PairState::RequestedByPeer => {
                let key = d.verification_key.unwrap_or_default();
                let mut desk = notify_rust::Notification::new();
                desk.appname("Tether")
                    .summary(&format!("{} wants to pair", d.name))
                    .body(&format!(
                        "Check that the phone shows {key}, then accept in Tether."
                    ));
                show(desk).await;
            }
            _ => {}
        }
    }
}

async fn show(n: notify_rust::Notification) -> Option<notify_rust::NotificationHandle> {
    tokio::task::spawn_blocking(move || n.show().ok())
        .await
        .ok()
        .flatten()
}

/// Many notification servers render a subset of HTML in the body.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
