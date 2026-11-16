//! Running inside a Flatpak sandbox: what changes, and the portals that stand in for host access.
//!
//! Inside the sandbox our binaries live under `/app/bin`, which the host can't run directly, and the
//! session bus is filtered, so start-at-login goes through the Background portal instead of a
//! systemd unit, and features that need the host (watching other apps' notifications, menu entries)
//! are switched off rather than failing quietly.

use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use zbus::{OwnedObjectPath, OwnedVariant, Variant};

/// The sandbox sets `FLATPAK_ID` for every process, and always mounts `/.flatpak-info`.
pub fn active() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

/// The remote this copy was installed from (`flathub`, or `<name>-origin` for a bundle file).
pub fn origin() -> Option<String> {
    origin_from(&std::fs::read_to_string("/.flatpak-info").ok()?)
}

fn origin_from(info: &str) -> Option<String> {
    let mut in_instance = false;
    for line in info.lines().map(str::trim) {
        if line.starts_with('[') {
            in_instance = line == "[Instance]";
        } else if in_instance {
            if let Some(v) = line.strip_prefix("origin=") {
                return Some(v.trim().to_string()).filter(|v| !v.is_empty());
            }
        }
    }
    None
}

/// Whether a store updates this copy. A `.flatpak` bundle from the releases page has no remote to
/// update from, so it is treated like any other download.
pub fn store_updates() -> bool {
    origin().is_some_and(|o| o == "flathub")
}

/// Request object path the portal will answer on, predictable from our unique name and a token, so
/// we can listen before calling and never miss a fast reply (xdg-desktop-portal's Request docs).
fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

/// Asks the Background portal to start `tether daemon` at login (or to stop doing so). The desktop
/// may ask the user first; a refusal is an error so the setting doesn't claim to be on.
pub async fn request_background(autostart: bool) -> Result<()> {
    let conn = zbus::Connection::session()
        .await
        .context("no session bus")?;
    let unique = conn
        .unique_name()
        .ok_or_else(|| anyhow!("no bus name"))?
        .to_string();
    let token = format!("tether{}", std::process::id());
    let path = request_path(&unique, &token);
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .member("Response")
        .path(path.as_str())
        .build()?;
    let mut responses = zbus::MessageStream::for_match_rule(rule, &conn, std::num::NonZeroUsize::new(4)).await?;

    let mut options: HashMap<&str, Variant> = HashMap::new();
    options.insert("handle_token", Variant::from(token.as_str()));
    options.insert(
        "reason",
        Variant::from("Keep your phone linked after you close the window."),
    );
    options.insert("autostart", Variant::from(autostart));
    options.insert("commandline", Variant::from(vec!["tether", "daemon"]));
    options.insert("dbus-activatable", Variant::from(false));
    let portal = zbus::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Background",
    )
    .await?;
    let _handle: OwnedObjectPath = portal
        .call("RequestBackground", &("", options))
        .await
        .context("the desktop has no Background portal")?;

    // The user may be looking at a permission dialog; give them time.
    let msg = tokio::time::timeout(Duration::from_secs(300), responses.next())
        .await
        .map_err(|_| anyhow!("no answer from the desktop"))?
        .ok_or_else(|| anyhow!("no answer from the desktop"))??;
    let (code, results): (u32, HashMap<String, OwnedVariant>) = msg.body().deserialize()?;
    if code != 0 {
        bail!("Starting at login wasn't allowed. You can allow it in your system's app settings.");
    }
    let granted = results
        .get("autostart")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(autostart);
    if autostart && !granted {
        bail!("Starting at login wasn't allowed. You can allow it in your system's app settings.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{origin_from, request_path};

    #[test]
    fn reads_origin_from_instance_section() {
        let info = "[Application]\nname=io.github.mh_anwar.Tether\n\n[Instance]\nbranch=master\norigin=tether-origin\n";
        assert_eq!(origin_from(info).as_deref(), Some("tether-origin"));
        assert_eq!(origin_from("[Application]\norigin=x\n"), None);
    }

    #[test]
    fn request_path_follows_portal_rules() {
        assert_eq!(
            request_path(":1.42", "tether7"),
            "/org/freedesktop/portal/desktop/request/1_42/tether7"
        );
    }
}
