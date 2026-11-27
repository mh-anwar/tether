//! A ringing call's desktop pop-up with Answer and Decline, so a call can be picked up without opening
//! the window, the way Phone Link does it. Spoken to the notification server over the watcher's own
//! session connection (not notify-rust) because the pop-up has to be closed from here once the call is
//! answered on the phone or the caller gives up, while its buttons are still being waited on.

use std::collections::HashMap;

use zbus::{Connection, Variant};

use super::Call;

pub const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub fn ringing(c: &Call) -> bool {
    c.incoming && matches!(c.state.as_str(), "incoming" | "waiting")
}

/// Which calls have a pop-up, by call id; 0 while the server hasn't answered with its id yet.
#[derive(Default)]
pub struct Popups {
    shown: HashMap<String, u32>,
}

impl Popups {
    /// Compares with the latest calls: ringing calls without a pop-up get one, pop-ups whose call
    /// stopped ringing (answered, declined, ended) close.
    pub fn diff(&mut self, calls: &[Call]) -> (Vec<Call>, Vec<u32>) {
        let ring: Vec<&Call> = calls.iter().filter(|c| ringing(c)).collect();
        let mut close = Vec::new();
        self.shown.retain(|id, n| {
            let keep = ring.iter().any(|c| &c.id == id);
            if !keep && *n != 0 {
                close.push(*n);
            }
            keep
        });
        let mut show = Vec::new();
        for c in ring {
            if !self.shown.contains_key(&c.id) {
                self.shown.insert(c.id.clone(), 0);
                show.push(c.clone());
            }
        }
        (show, close)
    }

    /// Records the server's id; false when the call stopped ringing meanwhile (close it right away).
    pub fn shown(&mut self, call: &str, notification: u32) -> bool {
        match self.shown.get_mut(call) {
            Some(n) => {
                *n = notification;
                true
            }
            None => false,
        }
    }

    pub fn call_for(&self, notification: u32) -> Option<String> {
        self.shown
            .iter()
            .find(|(_, n)| **n == notification && notification != 0)
            .map(|(c, _)| c.clone())
    }
}

/// Shows the pop-up and returns the server's id for it.
pub async fn show(conn: &Connection, c: &Call) -> Option<u32> {
    let who = c.name.clone().unwrap_or_else(|| {
        if c.number.is_empty() {
            "Unknown caller".into()
        } else {
            c.number.clone()
        }
    });
    let body = match (&c.name, c.number.is_empty()) {
        (Some(_), false) => format!("{who}\n{}", c.number),
        _ => who,
    };
    let summary = if c.state == "waiting" {
        "Call waiting"
    } else {
        "Incoming call"
    };
    // "default" is a click on the pop-up itself.
    let actions = [
        "answer",
        "Answer",
        "decline",
        "Decline",
        "default",
        "Open Tether",
    ];
    let mut hints: HashMap<&str, Variant> = HashMap::new();
    hints.insert("urgency", Variant::U8(2));
    hints.insert("category", Variant::from("call.incoming"));
    let app = std::env::var("FLATPAK_ID").unwrap_or_else(|_| "tether".into());
    hints.insert("desktop-entry", Variant::from(app.clone()));
    let reply = conn
        .call_method(
            Some(NOTIFICATIONS),
            PATH,
            Some(NOTIFICATIONS),
            "Notify",
            // Never expires on its own: it closes when the call stops ringing.
            &(
                "Tether",
                0u32,
                "call-start",
                summary,
                escape(&body),
                &actions[..],
                hints,
                0i32,
            ),
        )
        .await
        .map_err(|e| tracing::debug!("call pop-up failed: {e}"))
        .ok()?;
    reply.body().deserialize::<u32>().ok()
}

pub async fn close(conn: &Connection, id: u32) {
    let _ = conn
        .call_method(
            Some(NOTIFICATIONS),
            PATH,
            Some(NOTIFICATIONS),
            "CloseNotification",
            &(id,),
        )
        .await;
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

    fn call(id: &str, state: &str, incoming: bool) -> Call {
        Call {
            id: id.into(),
            number: "+15551234567".into(),
            name: None,
            state: state.into(),
            incoming,
            since: None,
        }
    }

    #[test]
    fn pops_up_once_per_ringing_call_and_closes_when_it_stops() {
        let mut p = Popups::default();
        let (show, close) = p.diff(&[call("a", "incoming", true)]);
        assert_eq!((show.len(), close.len()), (1, 0));
        assert!(p.shown("a", 7));
        assert_eq!(p.call_for(7).as_deref(), Some("a"));
        // Still ringing: nothing new.
        let (show, close) = p.diff(&[call("a", "incoming", true)]);
        assert!(show.is_empty() && close.is_empty());
        // A second call while talking on the first rings as "waiting".
        let (show, close) = p.diff(&[call("a", "active", true), call("b", "waiting", true)]);
        assert_eq!((show[0].id.as_str(), close), ("b", vec![7]));
        // Outgoing calls never pop up, and a call gone before the server answered is dropped.
        let (show, close) = p.diff(&[call("c", "dialing", false)]);
        assert!(show.is_empty() && close.is_empty());
        assert!(!p.shown("b", 8));
        assert_eq!(p.call_for(0), None);
    }
}
