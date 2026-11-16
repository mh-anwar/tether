//! User settings in ~/.config/tether/config.json. Everything is optional.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub commands: Vec<tether_core::plugins::runcommand::Command>,
    pub allow_commands: bool,
    pub allow_input: bool,
    pub allow_lock: bool,
    pub allow_volume: bool,
    pub allow_ring: bool,
    pub device_name: String,
    /// Phones to reach directly by address (e.g. over a VPN where broadcast doesn't work).
    pub addresses: Vec<String>,
    /// Show phone notifications as desktop notifications.
    pub mirror_notifications: bool,
    /// Keep the computer's and the phone's clipboards the same.
    pub clipboard_sync: bool,
    pub mdns: bool,
    pub send_notifications: bool,
    pub notification_blocklist: Vec<String>,
    pub pause_media: String,
    pub mute_media: bool,
    pub downloads_dir: Option<PathBuf>,
    /// Install new versions by themselves where that needs no password (AppImage, no-admin install).
    /// Off: still check, and the window offers the update.
    pub auto_update: bool,
    /// Where to look for releases (GitHub's "latest release" API). For forks and tests.
    pub update_url: Option<String>,
    #[serde(skip)]
    pub data_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            commands: vec![],
            allow_commands: true,
            allow_input: true,
            allow_lock: true,
            allow_volume: true,
            allow_ring: true,
            device_name: hostname(),
            addresses: vec![],
            mirror_notifications: true,
            clipboard_sync: true,
            mdns: true,
            send_notifications: false,
            notification_blocklist: vec![],
            pause_media: "ringing".into(),
            mute_media: false,
            downloads_dir: None,
            auto_update: true,
            update_url: None,
            data_dir: PathBuf::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        let mut cfg: Config = match std::fs::read(&path) {
            Ok(b) => {
                serde_json::from_slice(&b).with_context(|| format!("reading {}", path.display()))?
            }
            Err(_) => Config::default(),
        };
        cfg.data_dir = dirs::data_dir()
            .context("no data directory")?
            .join("tether");
        Ok(cfg)
    }

    pub fn core(&self) -> tether_core::Config {
        let downloads = self
            .downloads_dir
            .clone()
            .or_else(dirs::download_dir)
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| self.data_dir.join("downloads"));
        tether_core::Config {
            data_dir: self.data_dir.clone(),
            downloads_dir: downloads,
            device_name: self.device_name.clone(),
            device_type: device_type(),
            mdns: self.mdns,
            manual_addresses: self
                .addresses
                .iter()
                .filter_map(|a| parse_addr(a))
                .collect(),
        }
    }
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("no config directory")?
        .join("tether")
        .join("config.json"))
}

/// What the Settings screen shows. `downloads_dir` is the folder in use, set or not.
pub fn settings() -> Result<Value> {
    let cfg = Config::load()?;
    Ok(json!({
        "commands": cfg.commands,
        "allow_commands": cfg.allow_commands,
        "allow_input": cfg.allow_input,
        "allow_lock": cfg.allow_lock,
        "allow_volume": cfg.allow_volume,
        "allow_ring": cfg.allow_ring,
        "device_name": tether_core::identity::sanitize_name(&cfg.device_name),
        "addresses": cfg.addresses,
        "mirror_notifications": cfg.mirror_notifications,
        "clipboard_sync": cfg.clipboard_sync,
        "mdns": cfg.mdns,
        "send_notifications": cfg.send_notifications,
        "notification_blocklist": cfg.notification_blocklist,
        "pause_media": cfg.pause_media,
        "mute_media": cfg.mute_media,
        "downloads_dir": cfg.core().downloads_dir,
        "auto_update": cfg.auto_update,
        "autostart": crate::autostart::is_enabled(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Saves the given settings into config.json, leaving every other key as it was. Returns whether
/// the service has to restart for them (the name, folder, addresses and clipboard are read at start).
pub fn save_settings(changes: &Value) -> Result<bool> {
    let path = config_path()?;
    let mut file: serde_json::Map<String, Value> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut restart = false;
    for (key, value) in changes.as_object().context("settings must be an object")? {
        let value = match key.as_str() {
            "commands" => {
                let commands: Vec<tether_core::plugins::runcommand::Command> =
                    serde_json::from_value(value.clone())
                        .context("commands must have id, name and command")?;
                let mut ids = std::collections::HashSet::new();
                for c in &commands {
                    if c.id.is_empty()
                        || c.name.trim().is_empty()
                        || c.command.trim().is_empty()
                        || !ids.insert(&c.id)
                    {
                        bail!("Give each command a unique id, a name and a command.");
                    }
                }
                json!(commands)
            }
            "allow_commands" | "allow_input" | "allow_lock" | "allow_volume" | "allow_ring" => {
                json!(value.as_bool().context("must be on or off")?)
            }
            "device_name" => {
                let name = value.as_str().unwrap_or("").trim();
                if name.is_empty() {
                    bail!("Give this computer a name.");
                }
                restart = true;
                json!(tether_core::identity::sanitize_name(name))
            }
            "addresses" => {
                let list: Vec<String> =
                    serde_json::from_value(value.clone()).context("addresses must be a list")?;
                if let Some(bad) = list.iter().find(|a| parse_addr(a).is_none()) {
                    bail!("\"{bad}\" isn't an address. It looks like 192.168.1.20.");
                }
                restart = true;
                json!(list)
            }
            "downloads_dir" => {
                let dir = PathBuf::from(value.as_str().unwrap_or("").trim());
                if !dir.is_absolute() {
                    bail!("Give the whole path of the folder, starting with /.");
                }
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("Can't use {} as a folder", dir.display()))?;
                restart = true;
                json!(dir)
            }
            "clipboard_sync" | "mdns" => {
                restart = true;
                json!(value.as_bool().context("must be on or off")?)
            }
            "pause_media" => {
                let mode = value
                    .as_str()
                    .context("pause_media must be ringing, talking or off")?;
                if !["ringing", "talking", "off"].contains(&mode) {
                    bail!("Invalid call media setting");
                }
                json!(mode)
            }
            "notification_blocklist" => {
                let names: Vec<String> = serde_json::from_value(value.clone())
                    .context("blocklist must be a list of app names")?;
                json!(names)
            }
            "mirror_notifications" | "auto_update" | "send_notifications" | "mute_media" => {
                json!(value.as_bool().context("must be on or off")?)
            }
            "autostart" => {
                if value.as_bool().context("autostart must be on or off")? {
                    crate::autostart::enable()?;
                } else {
                    crate::autostart::disable()?;
                }
                continue;
            }
            other => bail!("unknown setting '{other}'"),
        };
        file.insert(key.clone(), value);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
    std::fs::rename(tmp, &path)?;
    Ok(restart)
}

/// Accepts "1.2.3.4" or "1.2.3.4:1716".
pub fn parse_addr(s: &str) -> Option<SocketAddr> {
    s.parse().ok().or_else(|| {
        s.parse::<std::net::IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, tether_core::identity::UDP_PORT))
    })
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Linux computer".into())
}

/// "laptop" when a battery is present, so the phone shows the right icon.
fn device_type() -> String {
    let has_battery = std::fs::read_dir("/sys/class/power_supply")
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("BAT"))
        })
        .unwrap_or(false);
    if has_battery { "laptop" } else { "desktop" }.into()
}
