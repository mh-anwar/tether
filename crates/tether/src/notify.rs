//! Mirrors phone notifications and calls to the desktop's notification server (works with GNOME,
//! KDE, dunst, mako, ...). Text stays at debug level in logs: it is private.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant},
};

use tether_core::{plugins::Update, Event, PairState, Service};
use tokio::sync::broadcast::error::RecvError;
use tracing::debug;

/// Settings screen switch; phone notifications still reach the window when off.
pub static ENABLED: AtomicBool = AtomicBool::new(true);
/// "Pause notification pop-ups" in the tray menu: until it is unticked or the service restarts.
/// Calls still show, so a ringing phone is never missed.
pub static PAUSED: AtomicBool = AtomicBool::new(false);

/// Quiet mode (Settings, tray, palette): notifications are held and summed up in one pop-up at
/// most every `QUIET_MINUTES`. Calls and texts with a verification code still pop up at once.
static QUIET: AtomicBool = AtomicBool::new(false);
static QUIET_MINUTES: AtomicU32 = AtomicU32::new(60);
/// The summary on screen, replaced by the next one rather than stacking up.
static SUMMARY_ID: AtomicU32 = AtomicU32::new(0);
/// Only a click on the newest summary opens the window (older waits see the same id).
static SUMMARY_GEN: AtomicU64 = AtomicU64::new(0);

pub fn set_quiet(on: bool, minutes: u32) {
    QUIET.store(on, Ordering::Relaxed);
    QUIET_MINUTES.store(minutes.max(1), Ordering::Relaxed);
}

pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// Notifications held back by quiet mode, in arrival order, one per phone notification.
#[derive(Default)]
struct Held {
    since: Option<Instant>,
    /// (device/notification id, app name)
    items: Vec<(String, String)>,
}

impl Held {
    fn add(&mut self, key: String, app: String) {
        self.since.get_or_insert_with(Instant::now);
        // An updated notification (a chat with a new message) still counts once.
        if !self.items.iter().any(|(k, _)| *k == key) {
            self.items.push((key, app));
        }
    }

    fn remove(&mut self, key: &str) {
        self.items.retain(|(k, _)| k != key);
        if self.items.is_empty() {
            self.since = None;
        }
    }

    /// Due once the first held notification has waited the whole interval, or right away when
    /// quiet mode was turned off with some still held.
    fn take_due(&mut self, quiet: bool, every: Duration) -> Option<Vec<String>> {
        let waited = self.since.is_some_and(|t| t.elapsed() >= every);
        if self.items.is_empty() || (quiet && !waited) {
            return None;
        }
        self.since = None;
        Some(
            std::mem::take(&mut self.items)
                .into_iter()
                .map(|(_, a)| a)
                .collect(),
        )
    }
}

pub async fn mirror(service: Service) {
    let mut events = service.subscribe();
    // phone notification id -> desktop notification id, so dismissals on the phone close ours.
    let mut shown: HashMap<String, notify_rust::NotificationHandle> = HashMap::new();
    let mut held = Held::default();
    let mut tick = tokio::time::interval(Duration::from_secs(20));
    loop {
        let event = tokio::select! {
            e = events.recv() => match e {
                Ok(e) => e,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
            _ = tick.tick() => {
                if !ENABLED.load(Ordering::Relaxed) {
                    held = Held::default();
                } else if !PAUSED.load(Ordering::Relaxed) {
                    let every = Duration::from_secs(60 * u64::from(QUIET_MINUTES.load(Ordering::Relaxed)));
                    if let Some(apps) = held.take_due(quiet(), every) {
                        show_summary(summary_text(&apps));
                    }
                }
                continue;
            }
        };
        match event {
            Event::Update {
                device_id,
                update: Update::Notification(n),
            } => {
                if !ENABLED.load(Ordering::Relaxed)
                    || PAUSED.load(Ordering::Relaxed)
                    || n.silent
                    || (n.title.is_empty() && n.text.is_empty())
                {
                    continue;
                }
                let key = format!("{device_id}/{}", n.id);
                if quiet() && !has_code(&format!("{} {}", n.title, n.text)) {
                    held.add(key, n.app.clone());
                    continue;
                }
                let replaces = shown.get(&key).map(|h| h.id());
                let summary = if n.title.is_empty() {
                    n.app.clone()
                } else {
                    n.title.clone()
                };
                let mut desk = notify_rust::Notification::new();
                desk.hint(notify_rust::Hint::Custom(
                    "x-tether-mirrored".into(),
                    "true".into(),
                ));
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
                let key = format!("{device_id}/{id}");
                // Read or dismissed on the phone before the summary: nothing to tell about.
                held.remove(&key);
                if let Some(h) = shown.remove(&key) {
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
                // With the phone connected for calls over Bluetooth, the calls module shows the
                // ringing call itself, with Answer and Decline.
                if matches!(c.state, CallState::Ringing) && crate::calls::rings_here() {
                    continue;
                }
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

/// "12 notifications from WhatsApp, Gmail and 3 others": apps with the most first.
pub fn summary_text(apps: &[String]) -> String {
    let mut counts: Vec<(&str, usize)> = vec![];
    for app in apps {
        let name = if app.trim().is_empty() {
            "other apps"
        } else {
            app.as_str()
        };
        match counts.iter_mut().find(|(a, _)| *a == name) {
            Some((_, n)) => *n += 1,
            None => counts.push((name, 1)),
        }
    }
    // Stable: equal counts keep the order they arrived in.
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    let names: Vec<&str> = counts.iter().map(|(a, _)| *a).collect();
    let from = match names.as_slice() {
        [] => "your phone".to_string(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        // Four or more: a third name would be as long as "and 2 others".
        [a, b, rest @ ..] => format!("{a}, {b} and {} others", rest.len()),
    };
    let n = apps.len();
    let noun = if n == 1 {
        "notification"
    } else {
        "notifications"
    };
    format!("{n} {noun} from {from}")
}

/// A text with a sign-in code: shown at once even in quiet mode, codes expire within minutes.
/// Same idea as the window's `findCode`: a code word and a 4-8 digit number standing alone.
pub fn has_code(text: &str) -> bool {
    const WORDS: [&str; 10] = [
        "code",
        "otp",
        "passcode",
        "pin",
        "password",
        "verification",
        "token",
        "kod",
        "codigo",
        "código",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
    let has_word = words.iter().any(|w| WORDS.contains(w));
    let has_number = words
        .iter()
        .any(|w| (4..=8).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit()));
    has_word && has_number
}

/// One pop-up for everything quiet mode held back; a click opens the window's notifications.
fn show_summary(text: String) {
    let gen = SUMMARY_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    tokio::spawn(async move {
        let clicked = tokio::task::spawn_blocking(move || {
            let mut desk = notify_rust::Notification::new();
            desk.appname("Tether")
                .summary(&escape(&text))
                .body("Open Tether to read them.")
                .icon(&std::env::var("FLATPAK_ID").unwrap_or_else(|_| "tether".into()))
                .action("default", "Open Tether");
            let prev = SUMMARY_ID.load(Ordering::Relaxed);
            if prev != 0 {
                desk.id(prev);
            }
            let handle = desk.show().ok()?;
            SUMMARY_ID.store(handle.id(), Ordering::Relaxed);
            let mut clicked = false;
            handle.wait_for_action(|a| clicked = a == "default");
            Some(clicked)
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
        if clicked && SUMMARY_GEN.load(Ordering::Relaxed) == gen {
            crate::tray::open_window_with(serde_json::json!({"event": "show_notifications"})).await;
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn apps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn summary_uses_real_plurals_and_names_the_busiest_apps() {
        assert_eq!(
            summary_text(&apps(&["WhatsApp"])),
            "1 notification from WhatsApp"
        );
        assert_eq!(
            summary_text(&apps(&["WhatsApp", "WhatsApp"])),
            "2 notifications from WhatsApp"
        );
        assert_eq!(
            summary_text(&apps(&["Gmail", "WhatsApp", "WhatsApp"])),
            "3 notifications from WhatsApp and Gmail"
        );
        assert_eq!(
            summary_text(&apps(&["Gmail", "Signal", "WhatsApp"])),
            "3 notifications from Gmail, Signal and WhatsApp"
        );
        assert_eq!(
            summary_text(&apps(&["Gmail", "Signal", "WhatsApp", "Slack"])),
            "4 notifications from Gmail, Signal and 2 others"
        );
        let mut many = apps(&["WhatsApp"; 6]);
        many.extend(apps(&["Gmail", "Gmail", "Signal", "Slack", "Maps"]));
        many.push(String::new());
        assert_eq!(
            summary_text(&many),
            "12 notifications from WhatsApp, Gmail and 4 others"
        );
        assert_eq!(
            summary_text(&apps(&["A", "B", "C", "C", "D"])),
            "5 notifications from C, A and 2 others"
        );
        assert_eq!(
            summary_text(&apps(&["A", "A", "B", "C"])),
            "4 notifications from A, B and C"
        );
    }

    #[test]
    fn verification_codes_are_spotted() {
        assert!(has_code("Your verification code is 482913"));
        assert!(has_code("G-123456 is your Google code"));
        assert!(has_code("Use 1234 as your PIN"));
        assert!(!has_code("Lunch at 1230?"));
        assert!(!has_code("Your code review is ready"));
        assert!(!has_code("Call me at 5551234567 about the code"));
    }

    #[test]
    fn held_notifications_wait_for_the_interval() {
        let mut held = Held::default();
        let hour = Duration::from_secs(3600);
        assert!(held.take_due(true, hour).is_none(), "nothing held");
        held.add("p/1".into(), "WhatsApp".into());
        held.add("p/1".into(), "WhatsApp".into());
        held.add("p/2".into(), "Gmail".into());
        assert!(held.take_due(true, hour).is_none(), "not an hour yet");
        assert_eq!(
            held.take_due(true, Duration::ZERO).unwrap(),
            ["WhatsApp", "Gmail"]
        );
        assert!(held.take_due(true, Duration::ZERO).is_none(), "shown once");
        held.add("p/3".into(), "Maps".into());
        held.remove("p/3");
        assert!(
            held.take_due(false, hour).is_none(),
            "dismissed on the phone"
        );
        held.add("p/4".into(), "Maps".into());
        assert_eq!(
            held.take_due(false, hour).unwrap(),
            ["Maps"],
            "quiet mode turned off"
        );
    }
}
