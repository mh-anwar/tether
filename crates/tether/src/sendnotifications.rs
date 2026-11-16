//! Observe desktop Notify calls, including replies so the phone can close the real notification.
//! Packet fields: KDE plugins/sendnotifications/dbusnotificationslistener.cpp.
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use tether_core::{
    plugins::{notification, Update},
    Event, Packet, PairState, Service,
};
use zbus::{Connection, OwnedVariant, Proxy};
type Notify = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedVariant>,
    i32,
);
const RULES: &[&str] = &[
    "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
    "type='method_return'",
    "type='signal',interface='org.freedesktop.Notifications',member='NotificationClosed'",
];
fn notification_packet(id: u32, n: &Notify) -> Packet {
    Packet::new(
        notification::NOTIFICATION,
        json!({"id": format!("tether-desktop:{id}"), "appName": n.0, "title": n.3, "text": n.4,
        "ticker": if n.4.is_empty() { n.3.clone() } else { format!("{}: {}",n.3,n.4) }, "isClearable": true, "time": tether_core::packet::now_millis().to_string()}),
    )
}
fn allowed(n: &Notify) -> bool {
    let cfg = crate::config::Config::load().unwrap_or_default();
    cfg.send_notifications
        && !n.0.eq_ignore_ascii_case("tether")
        && !n.6.contains_key("x-tether-mirrored")
        && !cfg
            .notification_blocklist
            .iter()
            .any(|app| app.eq_ignore_ascii_case(&n.0))
}
fn icon(n: &Notify, s: &Service) -> Option<std::path::PathBuf> {
    // freedesktop image-data is (width,height,rowstride,alpha,bits,channels,bytes), with row padding.
    for key in ["image-data", "image_data", "icon_data"] {
        let Some(v) = n.6.get(key).and_then(|v| v.try_clone().ok()) else {
            continue;
        };
        let Ok((w, h, stride, alpha, bits, channels, data)) =
            <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(v)
        else {
            continue;
        };
        if !(1..=2048).contains(&w)
            || !(1..=2048).contains(&h)
            || bits != 8
            || channels != if alpha { 4 } else { 3 }
            || stride < w * channels
        {
            continue;
        }
        if data.len() < (stride as usize).checked_mul(h as usize)? {
            continue;
        }
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h as usize {
            for col in 0..w as usize {
                let i = row * stride as usize + col * channels as usize;
                pixels.extend_from_slice(&data[i..i + 3]);
                pixels.push(if alpha { data[i + 3] } else { 255 });
            }
        }
        let image = image::RgbaImage::from_raw(w as u32, h as u32, pixels)?;
        let dir = s.data_dir().join("desktop-icons");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{}.png", uuid::Uuid::new_v4()));
        image.save(&path).ok()?;
        return Some(path);
    }
    let candidate =
        n.6.get("image-path")
            .and_then(|v| <&str>::try_from(v).ok())
            .unwrap_or(&n.2);
    if let Ok(url) = url::Url::parse(candidate) {
        if let Ok(path) = url.to_file_path() {
            return path.is_file().then_some(path);
        }
    }
    let path = std::path::PathBuf::from(candidate);
    if path.is_absolute() && path.is_file() {
        return Some(path);
    }
    for root in [
        "/usr/share/icons/hicolor/48x48/apps",
        "/usr/share/icons/hicolor/64x64/apps",
        "/usr/share/pixmaps",
    ] {
        let path = std::path::Path::new(root).join(format!("{}.png", candidate.replace('/', "")));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}
async fn forward(s: &Service, id: u32, n: Notify) {
    if !allowed(&n) {
        return;
    }
    let p = notification_packet(id, &n);
    let art = icon(&n, s);
    for d in s.devices() {
        if d.connected && d.pair == PairState::Paired {
            if let Some(path) = &art {
                let mut packet = p.clone();
                if let Ok(data) = tokio::fs::read(path).await {
                    use sha2::Digest;
                    packet.body.insert(
                        "payloadHash".into(),
                        json!(hex::encode(sha2::Sha256::digest(data))),
                    );
                }
                if s.send_payload(&d.id, packet, path).await.is_err() {
                    let _ = s.send(&d.id, p.clone());
                }
            } else {
                let _ = s.send(&d.id, p.clone());
            }
        }
    }
    if let Some(path) = art.filter(|p| p.starts_with(s.data_dir().join("desktop-icons"))) {
        let _ = tokio::fs::remove_file(path).await;
    }
}
pub async fn run(s: Service) {
    let Ok(control) = Connection::session().await else {
        return;
    };
    let Ok(monitor) = Connection::session().await else {
        return;
    };
    let mut stream = zbus::MessageStream::from(&monitor);
    let Ok(dbus) = Proxy::new(
        &monitor,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus.Monitoring",
    )
    .await
    else {
        return;
    };
    if dbus
        .call::<_, _, ()>("BecomeMonitor", &(RULES, 0u32))
        .await
        .is_err()
    {
        let Ok(dbus) = zbus::fdo::DBusProxy::new(&monitor).await else {
            return;
        };
        for rule in RULES {
            let rule_text = format!("{rule},eavesdrop='true'");
            let Ok(rule) = zbus::MatchRule::try_from(rule_text.as_str()) else {
                return;
            };
            if dbus.add_match_rule(rule).await.is_err() {
                tracing::info!("Session bus does not allow desktop notification monitoring");
                return;
            }
        }
    }
    let mut pending = HashMap::new();
    let mut known = std::collections::HashSet::new();
    let mut events = s.subscribe();
    loop {
        tokio::select! {
            message = stream.next() => {
                let Some(Ok(message)) = message else { return };
                let h = message.header();
                if h.member().map(|m|m.as_str()) == Some("Notify") {
                    if let Ok(n) = message.body().deserialize::<Notify>() { if allowed(&n) {
                        if pending.len() > 256 { pending.clear(); }
                        pending.insert((h.sender().map(|v|v.to_string()).unwrap_or_default(),message.primary_header().serial_num()),n);
                    }}
                } else if let Some(serial) = h.reply_serial() {
                    let key = (h.destination().map(|v|v.to_string()).unwrap_or_default(),serial);
                    if let Some(n) = pending.remove(&key) { if let Ok(id) = message.body().deserialize::<u32>() {
                        known.insert(id); let svc = s.clone(); tokio::spawn(async move { forward(&svc,id,n).await; });
                    }}
                } else if h.member().map(|m|m.as_str()) == Some("NotificationClosed") {
                    if let Ok((id,_)) = message.body().deserialize::<(u32,u32)>() { if known.remove(&id) { for d in s.devices() { let _ = s.send(&d.id,Packet::new(notification::NOTIFICATION,json!({"id": format!("tether-desktop:{id}"), "isCancel": true}))); } } }
                }
            }
            event = events.recv() => {
                match event {
                    Ok(Event::Update { update: Update::DesktopRequest { packet }, .. }) if packet.kind == notification::REQUEST => {
                        if let Some(id) = packet.str("cancel").and_then(|v|v.strip_prefix("tether-desktop:")).and_then(|v|v.parse::<u32>().ok()).filter(|id|known.contains(id)) {
                            if let Ok(proxy) = Proxy::new(&control,"org.freedesktop.Notifications","/org/freedesktop/Notifications","org.freedesktop.Notifications").await { let _ = proxy.call::<_,_,()>("CloseNotification",&(id,)).await; }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    _ => {}
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_wire_fields() {
        let n = (
            "Editor".into(),
            0,
            "".into(),
            "Saved".into(),
            "Document".into(),
            vec![],
            HashMap::new(),
            -1,
        );
        let p = notification_packet(12, &n);
        assert_eq!(p.str("id"), Some("tether-desktop:12"));
        assert_eq!(p.str("ticker"), Some("Saved: Document"));
        assert_eq!(p.bool("isClearable"), Some(true));
    }
}
