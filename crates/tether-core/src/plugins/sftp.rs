//! The phone's file server ("Filesystem expose" in the KDE Connect app).
//! kdeconnect-android:src/org/kde/kdeconnect/Plugins/SftpPlugin/SftpPlugin.kt: we ask with
//! `startBrowsing`, the phone starts an SFTP server and answers with where and how to log in.

use std::fmt;

use serde_json::{json, Value};

use crate::Packet;

pub const SFTP: &str = "kdeconnect.sftp";
pub const REQUEST: &str = "kdeconnect.sftp.request";

/// Login details for the phone's SFTP server. The password is fresh each time and never logged.
#[derive(Clone)]
pub struct Login {
    pub ip: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    /// Folders the phone shares (internal storage, SD card), as absolute paths on its server.
    pub roots: Vec<String>,
}

impl fmt::Debug for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Login")
            .field("ip", &self.ip)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

pub fn request() -> Packet {
    Packet::new(REQUEST, json!({"startBrowsing": true}))
}

/// `None` for packets that aren't an answer to `request` (the phone sends `serverRunning: false`
/// when the plugin loads). `Some(Err)` carries the phone's own error text.
pub fn parse(p: &Packet) -> Option<Result<Login, String>> {
    if let Some(e) = p.str("errorMessage") {
        return Some(Err(e.to_string()));
    }
    let ip = p.str("ip")?.to_string();
    let port = u16::try_from(p.i64("port")?).ok()?;
    let strings = |k: &str| -> Vec<String> {
        p.body
            .get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut roots = strings("multiPaths");
    if roots.is_empty() {
        roots.extend(p.str("path").map(String::from));
    }
    Some(Ok(Login {
        ip,
        port,
        user: p.str("user").unwrap_or("kdeconnect").to_string(),
        password: p.str("password").unwrap_or("").to_string(),
        roots,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_login_and_errors() {
        let p = Packet::new(
            SFTP,
            json!({"ip": "192.168.1.5", "port": 1740, "user": "kdeconnect", "password": "pw",
                   "path": "/", "multiPaths": ["/storage/emulated/0"], "pathNames": ["Internal"]}),
        );
        let Some(Ok(l)) = parse(&p) else {
            panic!("no login")
        };
        assert_eq!(
            (l.port, l.roots.as_slice()),
            (1740, &["/storage/emulated/0".to_string()][..])
        );
        assert!(!format!("{l:?}").contains("pw"));

        assert!(parse(&Packet::new(SFTP, json!({"serverRunning": false}))).is_none());
        assert!(matches!(
            parse(&Packet::new(SFTP, json!({"errorMessage": "no permission"}))),
            Some(Err(_))
        ));
    }
}
