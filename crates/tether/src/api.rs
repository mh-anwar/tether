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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
};
use tracing::{debug, warn};

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
    let writer = tokio::spawn(async move {
        while let Some(v) = rx.recv().await {
            let mut line = serde_json::to_vec(&v).unwrap_or_default();
            line.push(b'\n');
            if wr.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(rd).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let req: Request = match serde_json::from_str(&line) {
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

async fn call(svc: &Service, method: &str, p: &Value) -> Result<Value> {
    let ok = || Ok(json!(true));
    match method {
        "identity" => {
            let mut v = serde_json::to_value(svc.identity())?;
            // The window compares this with its own version and replaces a service left running
            // from before an upgrade.
            v["version"] = json!(env!("CARGO_PKG_VERSION"));
            v["discoveryListening"] = json!(svc.discovery_listening());
            v["clipboardSync"] =
                json!(crate::clipsync::ACTIVE.load(std::sync::atomic::Ordering::Relaxed));
            Ok(v)
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
            // After the reply goes out, so the window knows it was heard (it restarts us after an update).
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let _ = std::fs::remove_file(crate::socket_path());
                std::process::exit(0);
            });
            ok()
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
        _ => {
            let dev = s(p, "device")?;
            match method {
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
                "share_file" => {
                    let n = svc.share_file(dev, Path::new(s(p, "path")?)).await?;
                    Ok(json!({"bytes": n}))
                }
                _ => Err(anyhow!("unknown method '{method}'")),
            }
        }
    }
}
