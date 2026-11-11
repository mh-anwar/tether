//! Trusted (paired) devices on disk: one JSON file per device holding the pinned certificate.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{identity::is_valid_device_id, Identity, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub identity: Identity,
    /// Hex DER of the certificate pinned at pairing.
    pub certificate: String,
    /// Last address the device was reached at, so we can call it directly after a restart.
    #[serde(default)]
    pub last_address: Option<String>,
}

impl TrustedDevice {
    pub fn cert_der(&self) -> Option<Vec<u8>> {
        hex::decode(&self.certificate).ok()
    }
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        // Ids come from the network; refuse anything that could escape the directory.
        is_valid_device_id(id).then(|| self.dir.join(format!("{id}.json")))
    }

    pub fn load_all(&self) -> Vec<TrustedDevice> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return vec![];
        };
        entries
            .flatten()
            .filter_map(|e| fs::read(e.path()).ok())
            .filter_map(|b| serde_json::from_slice::<TrustedDevice>(&b).ok())
            .filter(|d| is_valid_device_id(&d.identity.device_id) && d.cert_der().is_some())
            .collect()
    }

    pub fn save(&self, d: &TrustedDevice) -> Result<()> {
        let Some(path) = self.path(&d.identity.device_id) else {
            return Ok(());
        };
        fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(d)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        if let Some(p) = self.path(id) {
            match fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }
}
