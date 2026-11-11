//! Two-way clipboard sync, run by the daemon so it works with the window closed.
//!
//! Desktop clipboards have no portable change signal, so a thread polls once a second. The
//! clipboard handle lives on that thread for the daemon's life: on X11 our copied text exists only
//! while its owner does. Contents are never logged.
//!
//! Limits: GNOME on Wayland doesn't let background apps read the clipboard (no data-control
//! protocol); phone-to-desktop still works there only while Tether's window is focused.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use tether_core::{plugins, plugins::Update, Event, PairState, Service};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info};

const POLL: Duration = Duration::from_millis(1000);
/// Big clipboards (a whole document) aren't worth sending to a phone.
const MAX_LEN: usize = 256 * 1024;

/// Whether sync is running; the window reads it to avoid copying phone text a second time.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn start(service: Service) {
    let (to_desk, from_phone) = mpsc::channel::<String>();
    let (to_phone, mut changed) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || clipboard_thread(from_phone, to_phone))
        .ok();

    // Desktop -> phones.
    let svc = service.clone();
    tokio::spawn(async move {
        while let Some(text) = changed.recv().await {
            for d in svc.devices() {
                if d.connected && d.pair == PairState::Paired {
                    if let Err(e) = svc.send(&d.id, plugins::clipboard(&text)) {
                        debug!("clipboard to {}: {e}", d.name);
                    }
                }
            }
        }
    });

    // Phone -> desktop.
    tokio::spawn(async move {
        let mut events = service.subscribe();
        loop {
            match events.recv().await {
                Ok(Event::Update {
                    update: Update::Clipboard { content },
                    ..
                }) => {
                    if to_desk.send(content).is_err() {
                        return;
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    });
}

fn clipboard_thread(
    from_phone: mpsc::Receiver<String>,
    to_phone: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut clip: Option<arboard::Clipboard> = None;
    let mut last: Option<String> = None;
    let mut baselined = false;
    let mut warned = false;
    loop {
        // Retry until a display is reachable: at login the service can start before the session.
        if clip.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => {
                    info!("clipboard sync on");
                    ACTIVE.store(true, Ordering::Relaxed);
                    clip = Some(c);
                }
                Err(e) => {
                    if !warned {
                        info!("clipboard not reachable yet ({e}); retrying");
                        warned = true;
                    }
                }
            }
        }
        match from_phone.recv_timeout(POLL) {
            Ok(text) => {
                if let Some(c) = clip.as_mut() {
                    if c.set_text(text.clone()).is_ok() {
                        // Remember what the clipboard now reports (it may normalise line endings),
                        // so the next poll doesn't send it straight back to the phone.
                        last = Some(c.get_text().unwrap_or(text));
                        baselined = true;
                    } else {
                        // Display gone (session ended, compositor restarted): reconnect, and let
                        // the window copy in the meantime.
                        ACTIVE.store(false, Ordering::Relaxed);
                        clip = None;
                    }
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let Some(c) = clip.as_mut() else { continue };
        // Empty or not text (an image, files) reads as None.
        let now = c.get_text().ok().filter(|t| !t.is_empty());
        if !baselined {
            // What was there before we started watching wasn't copied for the phone.
            baselined = true;
            last = now;
            continue;
        }
        if now == last {
            continue;
        }
        last = now.clone();
        if let Some(text) = now.filter(|t| t.len() <= MAX_LEN) {
            if to_phone.send(text).is_err() {
                return;
            }
        }
    }
}

/// A login service may start without the session's display variables (common on i3 and other
/// setups that don't import them into systemd). Point at the usual sockets if they exist.
/// Call before any other thread starts: changing the environment isn't thread-safe.
pub fn guess_display_env() {
    let runtime = dirs::runtime_dir();
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Some(r) = &runtime {
            if r.join("wayland-0").exists() {
                std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            }
        }
    }
    if std::env::var_os("DISPLAY").is_none() && std::path::Path::new("/tmp/.X11-unix/X0").exists() {
        std::env::set_var("DISPLAY", ":0");
        if std::env::var_os("XAUTHORITY").is_none() {
            // GDM keeps the cookie here; other display managers use ~/.Xauthority, Xlib's default.
            if let Some(gdm) = runtime
                .map(|r| r.join("gdm/Xauthority"))
                .filter(|p| p.exists())
            {
                std::env::set_var("XAUTHORITY", gdm);
            }
        }
    }
}
