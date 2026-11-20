//! Local control API: JSON lines over a Unix socket, readable only by this user.
//! Request:  {"id": 1, "method": "devices", "params": {...}}
//! Reply:    {"id": 1, "result": ...}  or  {"id": 1, "error": "..."}
//! Events are pushed to every connection as {"event": ..., ...}.

use std::{os::unix::fs::PermissionsExt, path::Path};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tether_core::{
    plugins::{self, notification, sms, telephony},
    Service,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
};
use tracing::{debug, info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

pub async fn serve(service: Service, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(path);
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(handle_conn(service.clone(), stream));
    }
}

async fn handle_conn(service: Service, stream: UnixStream) {
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<Value>(512);
    let mut events = service.subscribe();
    let ev_tx = tx.clone();
    let forward = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(e) => {
                    if ev_tx
                        .send(serde_json::to_value(e).unwrap_or(Value::Null))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("client missed {n} events")
                }
                Err(_) => break,
            }
        }
    });
    let ui_tx = tx.clone();
    let mut ui = UI_EVENTS.subscribe();
    let forward_ui = tokio::spawn(async move {
        while let Ok(v) = ui.recv().await {
            if ui_tx.send(v).await.is_err() {
                break;
            }
        }
    });
    let writer = tokio::spawn(async move {
        while let Some(v) = rx.recv().await {
            let mut line = serde_json::to_vec(&v).unwrap_or_default();
            line.push(b'\n');
            if wr.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    let mut reader = BufReader::new(rd);
    while let Ok(Some(line)) = request_line(&mut reader).await {
        let req: Request = match serde_json::from_slice(&line) {
            Ok(r) => r,
            Err(e) => {
                debug!("bad request: {e}");
                continue;
            }
        };
        let (service, tx) = (service.clone(), tx.clone());
        // Each request runs on its own so a slow file transfer doesn't block the rest.
        tokio::spawn(async move {
            let reply = match call(&service, &req.method, &req.params).await {
                Ok(result) => json!({"id": req.id, "result": result}),
                Err(e) => json!({"id": req.id, "error": e.to_string()}),
            };
            let _ = tx.send(reply).await;
        });
    }
    forward.abort();
    forward_ui.abort();
    drop(tx);
    let _ = writer.await;
}

fn s<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing '{key}'"))
}

fn i(p: &Value, key: &str) -> Result<i64> {
    p.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing '{key}'"))
}

/// Events from the service itself (not from a phone) to every connected window.
static UI_EVENTS: std::sync::LazyLock<tokio::sync::broadcast::Sender<Value>> =
    std::sync::LazyLock::new(|| tokio::sync::broadcast::channel(16).0);

pub fn ui_event(event: Value) {
    let _ = UI_EVENTS.send(event);
}

/// A file in the downloads folder (what the phone sent), refusing any other path.
fn received_file(path: &str) -> Result<std::path::PathBuf> {
    let downloads = crate::config::Config::load()?
        .core()
        .downloads_dir
        .canonicalize()?;
    let path = std::path::PathBuf::from(path).canonicalize()?;
    if !path.starts_with(downloads) {
        anyhow::bail!("This file is outside the downloads folder");
    }
    Ok(path)
}

/// Stops the service after the reply has gone out, so the window knows it was heard; the window
/// then starts a fresh copy (after an update, or to pick up changed settings).
pub(crate) fn exit_soon() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = std::fs::remove_file(crate::socket_path());
        std::process::exit(0);
    });
}

/// Quits Tether completely: open windows close first, since a window left open would start the
/// service again.
pub(crate) fn quit_all() {
    ui_event(json!({"event": "quit"}));
    exit_soon();
}

async fn call(svc: &Service, method: &str, p: &Value) -> Result<Value> {
    let ok = || Ok(json!(true));
    match method {
        "identity" => {
            let mut v = serde_json::to_value(svc.identity())?;
            // The window compares this with its own version and replaces a service left running
            // from before an upgrade.
            v["version"] = json!(env!("CARGO_PKG_VERSION"));
            v["discoveryListening"] = json!(svc.discovery_listening());
            // The window hides what the sandbox can't do (menu entries, sending notifications).
            v["flatpak"] = json!(crate::flatpak::active());
            v["clipboardSync"] =
                json!(crate::clipsync::ACTIVE.load(std::sync::atomic::Ordering::Relaxed));
            Ok(v)
        }
        "received_list" => Ok(json!(crate::received::history())),
        // The window may only load files from its own folders, so a camera photo saved in
        // Downloads is shown from a copy next to the other full-size phone pictures.
        "photo_preview" => {
            let path = received_file(s(p, "path")?)?;
            Ok(json!(crate::phonefiles::preview_copy(svc, &path)?))
        }
        "received_open" | "received_show" => {
            let path = received_file(s(p, "path")?)?;
            let target = if method == "received_show" {
                path.parent().context("no parent folder")?
            } else {
                &path
            };
            crate::received::open(&target.to_string_lossy())?;
            ok()
        }
        "devices" => Ok(serde_json::to_value(svc.devices())?),
        "refresh" => {
            svc.refresh();
            ok()
        }
        "connect" => {
            let addr = crate::config::parse_addr(s(p, "address")?)
                .ok_or_else(|| anyhow!("not an address"))?;
            svc.add_manual_address(addr);
            ok()
        }
        "apps_status" => crate::apps::status(svc).await,
        "apps_pair_qr" => crate::apps::pair_qr(svc),
        "apps_pair_code" => {
            let address = p.get("address").and_then(Value::as_str);
            crate::apps::pair_code(svc, address, s(p, "code")?).await
        }
        "apps_cancel" => Ok(crate::apps::cancel()),
        "update_status" => Ok(crate::update::status()),
        "update_check" => crate::update::check(false).await,
        "update_install" => crate::update::install().await,
        "shutdown" => {
            exit_soon();
            ok()
        }
        "quit" => {
            info!("quitting at the window's request");
            quit_all();
            ok()
        }
        "window_shown" => {
            crate::tray::window_shown();
            ok()
        }
        "ui_pending" => Ok(crate::tray::take_pending()),
        // A fixed page, so the window can't be used to open arbitrary links.
        "tray_extension_open" => {
            crate::received::open(crate::tray::EXTENSION_URL)?;
            ok()
        }
        "seen_set" => {
            // thread id -> date of the newest message read in the window (JSON keys are text).
            let seen: std::collections::HashMap<String, i64> =
                serde_json::from_value(p.get("seen").cloned().unwrap_or(Value::Null))
                    .context("missing 'seen'")?;
            crate::tray::set_seen(
                seen.into_iter()
                    .filter_map(|(k, v)| Some((k.parse().ok()?, v)))
                    .collect(),
            );
            ok()
        }
        "commands_get" => Ok(json!(crate::config::Config::load()?.commands)),
        "commands_set" => {
            crate::config::save_settings(
                &json!({"commands":p.get("commands").context("missing commands")?}),
            )?;
            crate::computer::send_state(svc, None).await;
            ok()
        }
        "computer_status" => Ok(
            json!({"ringing":crate::computer::RINGING.load(std::sync::atomic::Ordering::SeqCst),"commands":crate::computer::OPEN_COMMANDS.swap(false,std::sync::atomic::Ordering::SeqCst)}),
        ),
        "computer_ring_stop" => {
            crate::computer::stop_ring();
            ok()
        }
        "settings_get" => crate::config::settings(),
        "settings_set" => {
            // Turning on start at login in the Flatpak waits for the desktop's permission dialog,
            // so keep it off the runtime's worker threads.
            let changes = p.clone();
            let restart =
                tokio::task::spawn_blocking(move || crate::config::save_settings(&changes))
                    .await??;
            if let Some(on) = p.get("mirror_notifications").and_then(Value::as_bool) {
                crate::notify::ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
                crate::tray::refresh();
            }
            if let Some(on) = p.get("auto_update").and_then(Value::as_bool) {
                crate::update::set_auto(on);
            }
            if p.get("allow_ring") == Some(&json!(false)) {
                crate::computer::stop_ring();
            }
            crate::computer::send_state(svc, None).await;
            if restart {
                // The window starts a fresh service, which reads the new settings.
                exit_soon();
            }
            Ok(json!({"restart": restart}))
        }
        "apps_find_port" => Ok(crate::apps::find_pairing_port(svc)),
        "apps_doctor" => crate::apps::doctor(svc).await,
        "apps_download" => crate::apps::download(svc),
        "apps_list" => crate::apps::list(svc).await,
        "apps_menu" => Ok(crate::apps::menu_entries()),
        "apps_menu_set" => {
            let on = p.get("on").and_then(Value::as_bool).unwrap_or(true);
            let icon = p.get("icon").and_then(Value::as_str);
            crate::apps::set_menu_entry(s(p, "package")?, s(p, "name")?, icon, on)
        }
        "apps_open" => {
            let package = p.get("package").and_then(Value::as_str);
            let title = p.get("title").and_then(Value::as_str).unwrap_or("Phone");
            crate::apps::open(svc, package, title).await
        }
        "call_status" => Ok(crate::calls::status()),
        "call_answer" => crate::calls::answer(p.get("id").and_then(Value::as_str))
            .await
            .map(|_| json!(true)),
        "call_hangup" => crate::calls::hangup(p.get("id").and_then(Value::as_str))
            .await
            .map(|_| json!(true)),
        "call_dial" => crate::calls::dial(s(p, "number")?)
            .await
            .map(|_| json!(true)),
        "call_audio_here" => crate::calls::audio_here().await.map(|_| json!(true)),
        "apps_windows" => Ok(crate::apps::windows()),
        "apps_close" => Ok(crate::apps::close(p.get("package").and_then(Value::as_str))),
        _ => {
            let dev = s(p, "device")?;
            match method {
                "keyboard" => {
                    let key: plugins::mousepad::Key = serde_json::from_value(p.clone())?;
                    let packet =
                        plugins::mousepad::request(&key).context("Invalid or empty key")?;
                    svc.send(dev, packet)?;
                    ok()
                }
                "device_data" => Ok(serde_json::to_value(svc.device_data(dev)?)?),
                "threads" => Ok(serde_json::to_value(svc.threads(dev)?)?),
                "thread" => Ok(serde_json::to_value(svc.thread(dev, i(p, "thread_id")?)?)?),
                "contacts" => Ok(json!(svc
                    .contacts(dev)?
                    .into_iter()
                    .map(|(name, number)| json!({"name": name, "number": number}))
                    .collect::<Vec<_>>())),
                "contact_name" => Ok(json!(svc.contact_name(dev, s(p, "number")?))),
                "request_thread" => {
                    let before = p.get("before").and_then(Value::as_i64);
                    let count = p.get("count").and_then(Value::as_i64);
                    svc.send(
                        dev,
                        sms::request_conversation(i(p, "thread_id")?, before, count),
                    )?;
                    ok()
                }
                "request_threads" => {
                    svc.send(dev, sms::request_conversations())?;
                    ok()
                }
                "pair" => Ok(svc.pair(dev).map(|_| json!(true))?),
                "accept" => Ok(svc.accept_pair(dev).map(|_| json!(true))?),
                "reject" => Ok(svc.reject_pair(dev).map(|_| json!(true))?),
                "unpair" => Ok(svc.unpair(dev).map(|_| json!(true))?),
                "ping" => Ok(svc.send(dev, plugins::ping()).map(|_| json!(true))?),
                "ring" => Ok(svc.send(dev, plugins::ring_phone()).map(|_| json!(true))?),
                "photo_take" => Ok(svc.take_photo(dev).map(|_| json!(true))?),
                "mute" => Ok(svc
                    .send(dev, telephony::mute_ringer())
                    .map(|_| json!(true))?),
                "sms_send" => {
                    let to: Vec<String> =
                        serde_json::from_value(p.get("addresses").cloned().unwrap_or(Value::Null))
                            .context("missing 'addresses'")?;
                    let sub = p.get("sub_id").and_then(Value::as_i64);
                    svc.send(dev, sms::send(&to, s(p, "body")?, sub))?;
                    ok()
                }
                "notification_dismiss" => Ok(svc
                    .send(dev, notification::dismiss(s(p, "id")?))
                    .map(|_| json!(true))?),
                "notification_reply" => Ok(svc
                    .send(
                        dev,
                        notification::reply(s(p, "reply_id")?, s(p, "message")?),
                    )
                    .map(|_| json!(true))?),
                "notification_action" => Ok(svc
                    .send(dev, notification::action(s(p, "id")?, s(p, "action")?))
                    .map(|_| json!(true))?),
                "clipboard" => Ok(svc
                    .send(dev, plugins::clipboard(s(p, "content")?))
                    .map(|_| json!(true))?),
                "share_url" => Ok(svc
                    .send(dev, plugins::share_url(s(p, "url")?))
                    .map(|_| json!(true))?),
                "share_text" => Ok(svc
                    .send(dev, plugins::share_text(s(p, "text")?))
                    .map(|_| json!(true))?),
                "sms_attachment" => {
                    let path =
                        svc.sms_attachment(dev, i(p, "part_id")?, s(p, "unique_identifier")?)?;
                    Ok(json!({"path": path.map(|p| p.to_string_lossy().into_owned())}))
                }
                "sms_attachment_save" => {
                    let to = svc.save_attachment(s(p, "unique_identifier")?)?;
                    Ok(json!({"path": to.to_string_lossy()}))
                }
                "media_action" => {
                    let pkt = plugins::mpris::action(s(p, "player")?, s(p, "action")?)
                        .ok_or_else(|| anyhow!("unknown media action"))?;
                    svc.send(dev, pkt)?;
                    ok()
                }
                "media_volume" => Ok(svc
                    .send(
                        dev,
                        plugins::mpris::set_volume(s(p, "player")?, i(p, "volume")?),
                    )
                    .map(|_| json!(true))?),
                "media_seek" => Ok(svc
                    .send(
                        dev,
                        plugins::mpris::set_position(s(p, "player")?, i(p, "position_ms")?),
                    )
                    .map(|_| json!(true))?),
                "files_list" => {
                    crate::phonefiles::list_files(svc, dev, p.get("path").and_then(Value::as_str))
                        .await
                }
                "file_operation" => {
                    crate::phonefiles::file_operation(
                        svc,
                        dev,
                        s(p, "action")?,
                        s(p, "path")?,
                        p.get("target").and_then(Value::as_str),
                        p.get("directory").and_then(Value::as_bool).unwrap_or(false),
                    )
                    .await
                }
                "photos_list" => crate::phonefiles::list_photos(svc, dev).await,
                "photos_thumbs" => {
                    let paths: Vec<String> =
                        serde_json::from_value(p.get("paths").cloned().unwrap_or(Value::Null))
                            .context("missing 'paths'")?;
                    crate::phonefiles::thumbs(svc, dev, &paths).await
                }
                "photo_get" => {
                    let to = crate::phonefiles::full_picture(svc, dev, s(p, "path")?).await?;
                    Ok(json!({"path": to.to_string_lossy()}))
                }
                "photo_copy" => {
                    crate::phonefiles::copy_picture(svc, dev, s(p, "path")?).await?;
                    ok()
                }
                "photo_save" => {
                    let to = crate::phonefiles::save_picture(svc, dev, s(p, "path")?).await?;
                    Ok(json!({"path": to.to_string_lossy()}))
                }
                "screenshot_copy" => {
                    crate::phonefiles::copy_screenshot(svc, dev).await?;
                    ok()
                }
                "screenshot_save" => {
                    let to = crate::phonefiles::save_screenshot(svc, dev).await?;
                    Ok(json!({"path": to.to_string_lossy()}))
                }
                "share_files" => {
                    let paths: Vec<std::path::PathBuf> =
                        serde_json::from_value(p.get("paths").cloned().context("missing paths")?)?;
                    if paths.is_empty() || paths.len() > 256 {
                        anyhow::bail!("Choose between 1 and 256 files");
                    }
                    Ok(json!({"bytes": svc.share_files(dev, &paths).await?}))
                }
                "share_file" => {
                    let n = svc.share_file(dev, Path::new(s(p, "path")?)).await?;
                    Ok(json!({"bytes": n}))
                }
                _ => Err(anyhow!("unknown method '{method}'")),
            }
        }
    }
}

// fill_buf bounds allocation even when a client never sends a newline.
const MAX_REQUEST_LINE: usize = 1024 * 1024;
async fn request_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let bytes = reader.fill_buf().await?;
        if bytes.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let count = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |n| n + 1);
        if line.len() + count > MAX_REQUEST_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request line exceeds 1 MiB",
            ));
        }
        let done = bytes[count - 1] == b'\n';
        line.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if done {
            return Ok(Some(line));
        }
    }
}
#[cfg(test)]
mod limit_tests {
    use super::*;
    #[tokio::test]
    async fn request_lines_are_bounded() {
        let bytes = vec![b'x'; MAX_REQUEST_LINE + 1];
        assert_eq!(
            request_line(&mut BufReader::new(bytes.as_slice()))
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        let mut exact = vec![b' '; MAX_REQUEST_LINE];
        *exact.last_mut().unwrap() = b'\n';
        assert_eq!(
            request_line(&mut BufReader::new(exact.as_slice()))
                .await
                .unwrap()
                .unwrap()
                .len(),
            MAX_REQUEST_LINE
        );
        let mut reader = BufReader::new(&b"one\ntwo\n"[..]);
        assert_eq!(request_line(&mut reader).await.unwrap().unwrap(), b"one\n");
        assert_eq!(request_line(&mut reader).await.unwrap().unwrap(), b"two\n");
    }
}
