//! User settings in ~/.config/tether/config.json. Everything is optional.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub device_name: String,
    /// Phones to reach directly by address (e.g. over a VPN where broadcast doesn't work).
    pub addresses: Vec<String>,
    /// Show phone notifications as desktop notifications.
    pub mirror_notifications: bool,
    pub downloads_dir: Option<PathBuf>,
    #[serde(skip)]
    pub data_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            device_name: hostname(),
            addresses: vec![],
            mirror_notifications: true,
            downloads_dir: None,
            data_dir: PathBuf::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let dir = dirs::config_dir()
            .context("no config directory")?
            .join("tether");
        let path = dir.join("config.json");
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
            manual_addresses: self
                .addresses
                .iter()
                .filter_map(|a| parse_addr(a))
                .collect(),
        }
    }
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
