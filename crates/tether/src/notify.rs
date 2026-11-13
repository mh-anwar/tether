//! Mirrors phone notifications and calls to the desktop's notification server (works with GNOME,
//! KDE, dunst, mako, ...). Text stays at debug level in logs: it is private.

use std::collections::HashMap;

use tether_core::{plugins::Update, Event, PairState, Service};
use tokio::sync::broadcast::error::RecvError;
use tracing::debug;

/// Settings screen switch; phone notifications still reach the window when off.
pub static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

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
                if !ENABLED.load(std::sync::atomic::Ordering::Relaxed)
                    || n.silent
                    || (n.title.is_empty() && n.text.is_empty())
                {
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
                if crate::phonefiles::is_screenshot(&n.app, &n.title, &n.text) {
                    // Waiting for the click owns the handle, so this one isn't closed from the phone.
                    desk.action("copy", "Copy").action("save", "Save");
                    shown.remove(&key);
                    tokio::spawn(screenshot_actions(service.clone(), device_id, desk));
                    continue;
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

/// Shows a screenshot notification and acts on its Copy or Save button.
async fn screenshot_actions(service: Service, device_id: String, desk: notify_rust::Notification) {
    let clicked = tokio::task::spawn_blocking(move || {
        let handle = desk.show().ok()?;
        let mut clicked = None;
        handle.wait_for_action(|a| clicked = Some(a.to_string()));
        clicked
    })
    .await
    .ok()
    .flatten();
    let Some(action) = clicked.filter(|a| a == "copy" || a == "save") else {
        return;
    };
    let mut progress = notify_rust::Notification::new();
    progress
        .appname("Tether")
        .summary(if action == "copy" {
            "Copying the screenshot"
        } else {
            "Saving the screenshot"
        })
        .body("Getting it from your phone.");
    let handle = show(progress).await;
    let (summary, body) = if action == "copy" {
        match crate::phonefiles::copy_screenshot(&service, &device_id).await {
            Ok(()) => (
                "Screenshot copied".to_string(),
                "Paste it anywhere.".to_string(),
            ),
            Err(e) => ("Couldn't copy the screenshot".to_string(), e.to_string()),
        }
    } else {
        match crate::phonefiles::save_screenshot(&service, &device_id).await {
            Ok(p) => (
                "Screenshot saved".to_string(),
                p.to_string_lossy().into_owned(),
            ),
            Err(e) => ("Couldn't save the screenshot".to_string(), e.to_string()),
        }
    };
    let mut done = notify_rust::Notification::new();
    done.appname("Tether")
        .summary(&summary)
        .body(&escape(&body));
    if let Some(h) = &handle {
        done.id(h.id());
    }
    show(done).await;
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
