//! The Tether window. A thin client of `tether daemon`: it forwards API calls from the page to the
//! daemon's socket and pushes daemon events to the page. Starts the daemon if it isn't running.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{unix::OwnedWriteHalf, UnixStream},
    sync::{oneshot, Mutex},
};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

#[derive(Default)]
struct Daemon {
    writer: Mutex<Option<OwnedWriteHalf>>,
    pending: Pending,
    next: AtomicU64,
}

fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tether")
        .join("tether.sock")
}

/// The `tether` binary next to this one (packages and AppImages ship them together), or from PATH.
fn tether_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("tether")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("tether"))
}

fn start_daemon() {
    let _ = std::process::Command::new(tether_bin())
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Keeps a connection to the daemon, reconnecting and (re)starting it as needed.
async fn connection_loop(app: tauri::AppHandle) {
    let mut last_start: Option<std::time::Instant> = None;
    loop {
        let stream = match UnixStream::connect(socket_path()).await {
            Ok(s) => s,
            Err(_) => {
                // Retry the start now and then in case the daemon crashed, without spawning a
                // second copy while the first is still coming up.
                if last_start.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                    last_start = Some(std::time::Instant::now());
                    start_daemon();
                }
                let _ = app.emit("daemon", json!({"connected": false}));
                tokio::time::sleep(Duration::from_millis(700)).await;
                continue;
            }
        };
        let service_pid = stream.peer_cred().ok().and_then(|c| c.pid());
        let (rd, wr) = stream.into_split();
        let state = app.state::<Daemon>();
        *state.writer.lock().await = Some(wr);
        let _ = app.emit("daemon", json!({"connected": true}));
        tauri::async_runtime::spawn(replace_if_outdated(app.clone(), service_pid));

        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(id) = v.get("id").and_then(Value::as_u64) {
                if let Some(tx) = state.pending.lock().await.remove(&id) {
                    let _ = tx.send(v);
                }
            } else if v.get("event").and_then(Value::as_str) == Some("show_window") {
                // A click on the tray icon: come to the front and say so, or the service
                // starts another window.
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = call(app.state::<Daemon>(), "window_shown".into(), json!({})).await;
                });
            } else if v.get("event").is_some() {
                let _ = app.emit("tether", v);
            }
        }
        *state.writer.lock().await = None;
        // Fail whatever was waiting so the page doesn't hang.
        for (_, tx) in state.pending.lock().await.drain() {
            let _ = tx.send(json!({"error": "Tether's background service stopped"}));
        }
        let _ = app.emit("daemon", json!({"connected": false}));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// After an upgrade the old background service (started at login, or by the old window) keeps
/// running, and the new window would show the new screens on top of old behaviour. If the service
/// is another version, stop it; the connection loop then starts the one installed next to us.
/// Once per window, so a mismatched `tether` found on PATH can't make us loop.
async fn replace_if_outdated(app: tauri::AppHandle, pid: Option<i32>) {
    static TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let daemon = app.state::<Daemon>();
    let Ok(me) = call(daemon.clone(), "identity".into(), json!({})).await else {
        return;
    };
    let theirs = me.get("version").and_then(Value::as_str);
    if theirs == Some(env!("CARGO_PKG_VERSION")) || TRIED.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "Tether: background service is version {}, this window is {}; restarting it",
        theirs.unwrap_or("before 0.3.1"),
        env!("CARGO_PKG_VERSION")
    );
    // 0.3.0 knows "shutdown"; older services don't, so fall back to stopping the process itself.
    if call(daemon, "shutdown".into(), json!({})).await.is_ok() {
        return;
    }
    let Some(pid) = pid else { return };
    // Only a Tether process: the pid came from the socket's peer, but check before signalling.
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    if comm.trim().starts_with("tether") {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }
}

#[tauri::command]
async fn call(daemon: State<'_, Daemon>, method: String, params: Value) -> Result<Value, String> {
    let id = daemon.next.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, rx) = oneshot::channel();
    daemon.pending.lock().await.insert(id, tx);
    let mut line = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
        .map_err(|e| e.to_string())?;
    line.push(b'\n');
    {
        let mut w = daemon.writer.lock().await;
        let Some(w) = w.as_mut() else {
            daemon.pending.lock().await.remove(&id);
            return Err("Tether's background service isn't running".into());
        };
        w.write_all(&line).await.map_err(|e| e.to_string())?;
    }
    // File transfers can take a while; everything else answers immediately.
    let reply = tokio::time::timeout(Duration::from_secs(600), rx)
        .await
        .map_err(|_| "Timed out".to_string())?
        .map_err(|_| "No answer".to_string())?;
    if let Some(e) = reply.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

/// After an update replaced the files: stop the old background service, start a fresh window (which
/// starts the new service), and close this one. An AppImage is started through its file, since this
/// binary lives in the old copy's mount.
#[tauri::command]
async fn relaunch(app: tauri::AppHandle, daemon: State<'_, Daemon>) -> Result<(), String> {
    let _ = call(daemon, "shutdown".into(), json!({})).await;
    // Give the old service time to let go of its lock and socket.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let me = std::env::var_os("APPIMAGE")
        .filter(|a| !a.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            // Linux names a replaced binary "<path> (deleted)"; the new file is at <path>.
            let s = exe.to_string_lossy();
            Some(PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s)))
        })
        .ok_or("Couldn't find Tether to restart it")?;
    std::process::Command::new(me)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't restart Tether: {e}"))?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn choose_files() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Upload to phone")
            .pick_files()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// On X11 the clipboard content lives only as long as its owner, so keep one handle for the
/// whole app instead of dropping it right after copying.
#[derive(Default)]
struct Clip(std::sync::Mutex<Option<arboard::Clipboard>>);

#[tauri::command]
fn copy_text(clip: State<'_, Clip>, text: String) -> Result<(), String> {
    let mut guard = clip.0.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    match guard.as_mut() {
        Some(cb) => cb.set_text(text).map_err(|e| e.to_string()),
        None => Err("No clipboard".into()),
    }
}

/// Phone Link starts with the computer; do the same, turning it on at the first launch only, so
/// `tether autostart off` sticks. While it's on, rewrite the entry each launch so it follows an
/// AppImage that was moved or updated.
fn sync_autostart() {
    let Some(marker) = dirs::data_dir().map(|d| d.join("tether").join("autostart-offered")) else {
        return;
    };
    let enabled = dirs::config_dir().is_some_and(|c| {
        c.join("systemd/user/tether.service").exists()
            || c.join("autostart/tether.desktop").exists()
    });
    if marker.exists() && !enabled {
        return;
    }
    let ok = std::process::Command::new(tether_bin())
        .args(["autostart", "on"])
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok && !marker.exists() {
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, b"");
    }
}

fn main() {
    // The login entry of an AppImage can only name the AppImage itself, which runs this binary.
    if std::env::args().nth(1).as_deref() == Some("--daemon") {
        let status = std::process::Command::new(tether_bin())
            .arg("daemon")
            .status();
        std::process::exit(status.ok().and_then(|s| s.code()).unwrap_or(1));
    }
    // Phone-app menu entries of an AppImage run `Tether.AppImage --tether apps <package>`.
    if std::env::args().nth(1).as_deref() == Some("--tether") {
        let status = std::process::Command::new(tether_bin())
            .args(std::env::args().skip(2))
            .status();
        std::process::exit(status.ok().and_then(|s| s.code()).unwrap_or(1));
    }
    std::thread::spawn(sync_autostart);
    tauri::Builder::default()
        .manage(Daemon::default())
        .manage(Clip::default())
        .invoke_handler(tauri::generate_handler![
            call,
            copy_text,
            relaunch,
            choose_files
        ])
        .setup(|app| {
            tauri::async_runtime::spawn(connection_loop(app.handle().clone()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| eprintln!("Tether could not start: {e}"));
}
