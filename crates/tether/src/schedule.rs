//! Texts scheduled to go out later. The service holds them (not the window), so they send with
//! the window closed. Kept in `$XDG_DATA_HOME/tether/scheduled.json` (0600: message text is private).
//!
//! Never twice: a text is marked "sending" on disk before it is handed to the phone link and
//! removed once handed over. If the service dies in between, the text comes back as "unsure"
//! and waits for the person to decide, rather than going out again by itself.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tether_core::{plugins::sms, Event, PairState, Service};
use tokio::sync::{broadcast::error::RecvError, Notify};
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Scheduled,
    /// Being handed to the phone right now.
    Sending,
    /// The service stopped while sending: it may or may not have gone out.
    Unsure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scheduled {
    pub id: String,
    pub device: String,
    /// The conversation it belongs to, when it was scheduled from one.
    pub thread_id: Option<i64>,
    pub addresses: Vec<String>,
    pub body: String,
    #[serde(default)]
    pub sub_id: Option<i64>,
    /// When to send, in ms since the epoch.
    pub at: i64,
    #[serde(default)]
    pub state: State,
}

/// Furthest ahead a text may be scheduled.
const MAX_AHEAD_MS: i64 = 366 * 24 * 3600 * 1000;
/// A time picked a moment ago ("in 1 minute" while the dialog was open) still counts as now.
const PAST_GRACE_MS: i64 = 5 * 60 * 1000;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn is_due(s: &Scheduled, now: i64) -> bool {
    s.state == State::Scheduled && s.at <= now
}

fn check_time(at: i64, now: i64) -> Result<()> {
    if at < now - PAST_GRACE_MS {
        bail!("That time has passed. Pick a time later than now.");
    }
    if at > now + MAX_AHEAD_MS {
        bail!("Pick a time within the next year.");
    }
    Ok(())
}

fn check_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        bail!("Write the message first.");
    }
    Ok(())
}

/// Texts that were being sent when the service stopped can't be known to be unsent.
pub fn load(path: &Path) -> Vec<Scheduled> {
    let mut list: Vec<Scheduled> = match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
            warn!("scheduled texts: can't read {} ({e})", path.display());
            vec![]
        }),
        Err(_) => vec![],
    };
    for s in &mut list {
        if s.state == State::Sending {
            s.state = State::Unsure;
        }
    }
    list
}

pub fn save(path: &Path, list: &[Scheduled]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    crate::phonefiles::write_private(&tmp, &serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

/// Sends what is due on phones that are connected. Each text is saved as "sending" before it is
/// handed over and removed after; a text that can't be saved as sending isn't sent. Returns the
/// texts handed to the phone.
pub fn send_due(
    list: &mut Vec<Scheduled>,
    now: i64,
    connected: impl Fn(&str) -> bool,
    mut send: impl FnMut(&Scheduled) -> Result<()>,
    mut persist: impl FnMut(&[Scheduled]) -> Result<()>,
) -> Vec<Scheduled> {
    let mut sent = vec![];
    let due: Vec<String> = list
        .iter()
        .filter(|s| is_due(s, now) && connected(&s.device))
        .map(|s| s.id.clone())
        .collect();
    for id in due {
        let Some(i) = list.iter().position(|s| s.id == id) else {
            continue;
        };
        list[i].state = State::Sending;
        if let Err(e) = persist(list) {
            warn!("scheduled texts: not sending, can't save ({e})");
            list[i].state = State::Scheduled;
            continue;
        }
        match send(&list[i]) {
            Ok(()) => sent.push(list.remove(i)),
            Err(e) => {
                // The phone went away in between: back to waiting for it.
                debug!("scheduled text not sent yet: {e}");
                list[i].state = State::Scheduled;
            }
        }
        if let Err(e) = persist(list) {
            warn!("scheduled texts: can't save ({e})");
        }
    }
    sent
}

/// The list as the window shows it: `waiting` when it is due but the phone is out of reach.
pub fn view(list: &[Scheduled], now: i64, connected: impl Fn(&str) -> bool) -> Value {
    json!(list
        .iter()
        .map(|s| {
            let mut v = json!(s);
            v["waiting"] = json!(is_due(s, now) && !connected(&s.device));
            v
        })
        .collect::<Vec<_>>())
}

struct Store {
    path: PathBuf,
    list: Vec<Scheduled>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
/// The list changed through the API: look again now rather than at the next tick.
static CHANGED: Notify = Notify::const_new();

fn with_store<T>(f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
    let store = STORE
        .get()
        .context("Scheduled texts aren't ready yet. Try again in a moment.")?;
    let mut guard = store.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

fn connected_fn(service: &Service) -> impl Fn(&str) -> bool {
    let online: Vec<String> = service
        .devices()
        .into_iter()
        .filter(|d| d.connected && d.pair == PairState::Paired)
        .map(|d| d.id)
        .collect();
    move |id: &str| online.iter().any(|o| o == id)
}

pub fn list(service: &Service) -> Result<Value> {
    let connected = connected_fn(service);
    with_store(|s| Ok(view(&s.list, now_ms(), connected)))
}

pub fn add(service: &Service, p: &Value) -> Result<Value> {
    let device = p
        .get("device")
        .and_then(Value::as_str)
        .context("missing 'device'")?;
    if !service
        .devices()
        .iter()
        .any(|d| d.id == device && d.pair == PairState::Paired)
    {
        bail!("Pair your phone first.");
    }
    let addresses: Vec<String> =
        serde_json::from_value(p.get("addresses").cloned().unwrap_or(Value::Null))
            .context("missing 'addresses'")?;
    if addresses.iter().all(|a| a.trim().is_empty()) {
        bail!("Add who the message is for first.");
    }
    let body = p.get("body").and_then(Value::as_str).unwrap_or("");
    check_body(body)?;
    let at = p
        .get("at")
        .and_then(Value::as_i64)
        .context("missing 'at'")?;
    check_time(at, now_ms())?;
    let item = Scheduled {
        id: uuid::Uuid::new_v4().to_string(),
        device: device.into(),
        thread_id: p.get("thread_id").and_then(Value::as_i64),
        addresses,
        body: body.into(),
        sub_id: p.get("sub_id").and_then(Value::as_i64),
        at,
        state: State::Scheduled,
    };
    with_store(|s| {
        s.list.push(item.clone());
        save(&s.path, &s.list)
    })?;
    changed();
    Ok(json!(item))
}

/// New text or time. An "unsure" text that is edited counts as scheduled again.
pub fn edit(p: &Value) -> Result<Value> {
    let id = p
        .get("id")
        .and_then(Value::as_str)
        .context("missing 'id'")?;
    let body = p.get("body").and_then(Value::as_str);
    let at = p.get("at").and_then(Value::as_i64);
    if let Some(b) = body {
        check_body(b)?;
    }
    if let Some(t) = at {
        check_time(t, now_ms())?;
    }
    let item = with_store(|s| {
        let item = s
            .list
            .iter_mut()
            .find(|x| x.id == id)
            .context("This text has already been sent or cancelled.")?;
        if item.state == State::Sending {
            bail!("This text is being sent right now.");
        }
        if let Some(b) = body {
            item.body = b.into();
        }
        if let Some(t) = at {
            item.at = t;
        }
        item.state = State::Scheduled;
        let item = item.clone();
        save(&s.path, &s.list)?;
        Ok(item)
    })?;
    changed();
    Ok(json!(item))
}

pub fn cancel(p: &Value) -> Result<Value> {
    let id = p
        .get("id")
        .and_then(Value::as_str)
        .context("missing 'id'")?;
    with_store(|s| {
        let i = s
            .list
            .iter()
            .position(|x| x.id == id)
            .context("This text has already been sent or cancelled.")?;
        if s.list[i].state == State::Sending {
            bail!("This text is being sent right now.");
        }
        s.list.remove(i);
        save(&s.path, &s.list)
    })?;
    changed();
    Ok(json!(true))
}

fn changed() {
    CHANGED.notify_one();
}

/// Loads the saved texts, then sends each when it is due, as soon as its phone is connected.
pub async fn run(service: Service) {
    let path = service.data_dir().join("scheduled.json");
    let list = load(&path);
    if !list.is_empty() {
        info!("{} scheduled texts", list.len());
    }
    if STORE.set(Mutex::new(Store { path, list })).is_err() {
        return;
    }
    let mut events = service.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut last_view = Value::Null;
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = CHANGED.notified() => {}
            e = events.recv() => match e {
                // A phone came back: send what waited for it right away.
                Ok(Event::Device(_)) => {}
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
        }
        let svc = service.clone();
        let connected = connected_fn(&service);
        let now = now_ms();
        let result = with_store(|s| {
            let path = s.path.clone();
            let sent = send_due(
                &mut s.list,
                now,
                &connected,
                |item| {
                    svc.send(
                        &item.device,
                        sms::send(&item.addresses, &item.body, item.sub_id),
                    )
                    .map_err(Into::into)
                },
                |list| save(&path, list),
            );
            Ok((sent, view(&s.list, now, &connected)))
        });
        let Ok((sent, now_view)) = result else {
            continue;
        };
        if !sent.is_empty() {
            info!("sent {} scheduled texts", sent.len());
        }
        // Sent, cancelled, edited or now waiting for the phone: the window redraws its list.
        if now_view != last_view {
            crate::api::ui_event(json!({"event": "scheduled", "items": now_view}));
            last_view = now_view;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn item(id: &str, device: &str, at: i64) -> Scheduled {
        Scheduled {
            id: id.into(),
            device: device.into(),
            thread_id: Some(7),
            addresses: vec!["+15550100".into()],
            body: format!("text {id}"),
            sub_id: None,
            at,
            state: State::Scheduled,
        }
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("tether-sched-{}", uuid::Uuid::new_v4()))
            .join("scheduled.json")
    }

    #[test]
    fn due_only_when_the_time_has_come_and_still_scheduled() {
        let mut s = item("a", "p", 1_000);
        assert!(!is_due(&s, 999));
        assert!(is_due(&s, 1_000));
        assert!(is_due(&s, 5_000));
        s.state = State::Unsure;
        assert!(
            !is_due(&s, 5_000),
            "an unsure text never goes out by itself"
        );
        s.state = State::Sending;
        assert!(!is_due(&s, 5_000));
    }

    #[test]
    fn times_in_the_past_or_too_far_ahead_are_refused() {
        let now = 1_000_000_000_000;
        assert!(check_time(now + 3_600_000, now).is_ok());
        assert!(
            check_time(now - 60_000, now).is_ok(),
            "a minute ago is fine"
        );
        assert!(check_time(now - PAST_GRACE_MS - 1, now).is_err());
        assert!(check_time(now + MAX_AHEAD_MS + 1, now).is_err());
        assert!(check_body("  ").is_err());
    }

    #[test]
    fn saved_list_round_trips_and_sending_comes_back_unsure() {
        let path = temp_path();
        let mut sending = item("b", "p", 2);
        sending.state = State::Sending;
        let list = vec![item("a", "p", 1), sending];
        save(&path, &list).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let back = load(&path);
        assert_eq!(back[0], list[0]);
        assert_eq!(back[1].state, State::Unsure);
        assert_eq!(back[1].body, "text b");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_or_broken_file_is_an_empty_list() {
        let path = temp_path();
        assert!(load(&path).is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json").unwrap();
        assert!(load(&path).is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn sends_each_due_text_once() {
        let mut list = vec![item("a", "p", 10), item("b", "p", 100)];
        let sends = RefCell::new(vec![]);
        let send = |s: &Scheduled| {
            sends.borrow_mut().push(s.id.clone());
            Ok(())
        };
        let sent = send_due(&mut list, 50, |_| true, send, |_| Ok(()));
        assert_eq!(sent.len(), 1);
        assert_eq!(list.len(), 1, "the later text still waits");
        // Ticks after that send nothing more until "b" is due.
        send_due(&mut list, 60, |_| true, send, |_| Ok(()));
        send_due(&mut list, 200, |_| true, send, |_| Ok(()));
        send_due(&mut list, 300, |_| true, send, |_| Ok(()));
        assert_eq!(*sends.borrow(), ["a", "b"]);
        assert!(list.is_empty());
    }

    #[test]
    fn saved_as_sending_before_it_is_handed_over() {
        let mut list = vec![item("a", "p", 10)];
        let saved = RefCell::new(vec![]);
        send_due(
            &mut list,
            50,
            |_| true,
            |_| Ok(()),
            |l| {
                saved.borrow_mut().push(l.to_vec());
                Ok(())
            },
        );
        let saved = saved.borrow();
        assert_eq!(saved[0][0].state, State::Sending);
        assert!(saved[1].is_empty());
    }

    #[test]
    fn not_sent_when_it_cannot_be_saved_first() {
        let mut list = vec![item("a", "p", 10)];
        let mut sends = 0;
        let sent = send_due(
            &mut list,
            50,
            |_| true,
            |_| {
                sends += 1;
                Ok(())
            },
            |_| bail!("disk full"),
        );
        assert!(sent.is_empty());
        assert_eq!(sends, 0);
        assert_eq!(list[0].state, State::Scheduled);
    }

    #[test]
    fn offline_phone_waits_then_sends_once_back() {
        let mut list = vec![item("a", "p", 10), item("c", "other", 10)];
        let mut sends = vec![];
        let offline = |_: &str| false;
        assert!(send_due(&mut list, 50, offline, |_| Ok(()), |_| Ok(())).is_empty());
        let v = view(&list, 50, offline);
        assert_eq!(v[0]["waiting"], json!(true));
        // The link drops between the check and the send: still waiting, not lost.
        let sent = send_due(
            &mut list,
            60,
            |_| true,
            |s| {
                if s.id == "a" {
                    bail!("not connected")
                }
                Ok(())
            },
            |_| Ok(()),
        );
        assert_eq!(sent.len(), 1);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, State::Scheduled);
        // Back: sent once, then never again.
        let back = |d: &str| d == "p";
        for t in [70, 80, 90] {
            for s in send_due(&mut list, t, back, |_| Ok(()), |_| Ok(())) {
                sends.push(s.id);
            }
        }
        assert_eq!(sends, ["a"]);
        assert_eq!(view(&list, 90, back), json!([]));
    }

    #[test]
    fn view_marks_waiting_only_for_due_texts_of_absent_phones() {
        let list = vec![item("a", "p", 10), item("b", "p", 1_000)];
        let v = view(&list, 50, |_| false);
        assert_eq!(v[0]["waiting"], json!(true));
        assert_eq!(v[1]["waiting"], json!(false));
        assert_eq!(view(&list, 50, |_| true)[0]["waiting"], json!(false));
    }
}
