//! Recent texts and contacts of each paired phone, kept on disk so the window opens with them
//! after a restart instead of waiting for the phone to connect and resend everything.
//!
//! Message contents are private: files are readable only by the user and removed on unpair.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    identity::is_valid_device_id,
    plugins::{contacts::Contacts, DeviceData, Message},
    Result,
};

/// Enough to fill the conversation list and the first screen of each thread; older messages are
/// fetched from the phone when scrolled to, as before.
const PER_THREAD: usize = 30;

#[derive(Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    contacts: Contacts,
}

impl Snapshot {
    pub fn of(data: &DeviceData) -> Snapshot {
        let messages = data
            .threads
            .values()
            .flat_map(|t| t.values().rev().take(PER_THREAD).cloned())
            .collect();
        Snapshot {
            messages,
            contacts: data.contacts.clone(),
        }
    }

    /// Fills `data` with the cached texts and contacts.
    pub fn restore(self, data: &mut DeviceData) {
        for m in self.messages {
            data.threads
                .entry(m.thread_id)
                .or_insert_with(BTreeMap::new)
                .insert(m.id, m);
        }
        data.contacts = self.contacts;
    }
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Cache { dir }
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        // Ids come from the network; refuse anything that could escape the directory.
        is_valid_device_id(id).then(|| self.dir.join(format!("{id}.json")))
    }

    pub fn load(&self, id: &str) -> Option<Snapshot> {
        let bytes = fs::read(self.path(id)?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, id: &str, snap: &Snapshot) -> Result<()> {
        let Some(path) = self.path(id) else {
            return Ok(());
        };
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let tmp = path.with_extension("json.tmp");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec(snap)?)?;
        f.sync_all()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: i64, thread_id: i64, date: i64) -> Message {
        serde_json::from_value(serde_json::json!({
            "_id": id, "thread_id": thread_id, "body": format!("m{id}"), "date": date,
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn round_trips_newest_messages_per_thread() {
        let dir = std::env::temp_dir().join(format!("tether-cache-{}", std::process::id()));
        let cache = Cache::new(dir.clone());
        let id = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6";
        let mut data = DeviceData::default();
        for i in 0..40 {
            data.threads.entry(1).or_default().insert(i, msg(i, 1, i));
        }
        data.threads.entry(2).or_default().insert(100, msg(100, 2, 5));
        cache
            .save(id, &Snapshot::of(&data))
            .unwrap_or_else(|e| panic!("{e}"));

        let mode = fs::metadata(dir.join(format!("{id}.json")))
            .map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o600);

        let mut back = DeviceData::default();
        cache
            .load(id)
            .unwrap_or_else(|| panic!("no cache"))
            .restore(&mut back);
        assert_eq!(back.threads[&1].len(), PER_THREAD);
        assert!(back.threads[&1].contains_key(&39));
        assert!(!back.threads[&1].contains_key(&0));
        assert_eq!(back.threads[&2].len(), 1);

        cache.remove(id).unwrap_or_else(|e| panic!("{e}"));
        assert!(cache.load(id).is_none());
        assert!(cache.load("../etc/passwd").is_none());
        let _ = fs::remove_dir_all(dir);
    }
}
