//! Shared files, links and text are handled in the daemon, even with the window closed.
use tether_core::{plugins::Update, Event, Service};
pub fn open(path: &str) -> anyhow::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
static HISTORY: std::sync::Mutex<Vec<serde_json::Value>> = std::sync::Mutex::new(Vec::new());
pub fn history() -> Vec<serde_json::Value> {
    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
fn remember(path: &str) {
    let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    history.insert(0,serde_json::json!({"path":path,"name":std::path::Path::new(path).file_name().unwrap_or_default().to_string_lossy()}));
    history.truncate(100);
}
pub async fn run(s: Service) {
    let mut events = s.subscribe();
    // Keep the clipboard owner alive: dropping it immediately loses the text on X11.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut clip = arboard::Clipboard::new().ok();
        while let Ok(text) = rx.recv() {
            if clip.is_none() {
                clip = arboard::Clipboard::new().ok();
            }
            if let Some(c) = &mut clip {
                let _ = c.set_text(text);
            }
        }
    });
    loop {
        match events.recv().await {
            Ok(Event::Update {
                update:
                    Update::FileReceived {
                        path,
                        open: should_open,
                    },
                ..
            }) => {
                remember(&path);
                if should_open {
                    let _ = open(&path);
                }
                tokio::task::spawn_blocking(move || {
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    if let Ok(handle) = notify_rust::Notification::new()
                        .appname("Tether")
                        .summary(&format!("Received {name}"))
                        .action("open", "Open")
                        .action("folder", "Show in folder")
                        .show()
                    {
                        handle.wait_for_action(|a| {
                            if a == "open" {
                                let _ = open(&path);
                            } else if a == "folder" {
                                if let Some(parent) = std::path::Path::new(&path).parent() {
                                    let _ = open(&parent.to_string_lossy());
                                }
                            }
                        });
                    }
                });
            }
            Ok(Event::Update {
                update: Update::SharedText { text },
                ..
            }) => {
                let _ = tx.send(text);
                tokio::task::spawn_blocking(|| {
                    let _ = notify_rust::Notification::new()
                        .appname("Tether")
                        .summary("Text copied from your phone")
                        .show();
                });
            }
            Ok(Event::Update {
                update: Update::SharedUrl { url },
                ..
            }) => {
                // Only web links: xdg-open also accepts executable handlers and local paths.
                if url::Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
                    let _ = open(&url);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            _ => {}
        }
    }
}
