//! Files from the phone through its file server ("Filesystem expose" in KDE Connect), using the
//! desktop's OpenSSH `sftp` client. kdeconnect-kde mounts the same server with sshfs, with the
//! same login (our certificate's key, then the one-time password) and also without pinning the
//! server's host key: the login itself arrives over the authenticated TLS link.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use tether_core::{plugins::sftp::Login, Error, Service};
use tokio::{io::AsyncWriteExt, process::Command};

/// Env var through which `sftp` gets the one-time password: `SSH_ASKPASS` runs this binary,
/// which prints it (see `answer_askpass`). Never on a command line.
const ASKPASS_ENV: &str = "TETHER_ASKPASS_SECRET";

/// Called first thing in `main`: when OpenSSH runs us as its password helper, print the password.
pub fn answer_askpass() {
    if let Ok(secret) = std::env::var(ASKPASS_ENV) {
        if std::env::args().count() <= 2 {
            println!("{secret}");
            std::process::exit(0);
        }
    }
}

/// Where phones keep screenshots, under each shared storage root: Samsung and most others use
/// DCIM/Screenshots, Pixel and stock Android use Pictures/Screenshots.
const SCREENSHOT_DIRS: [&str; 2] = ["DCIM/Screenshots", "Pictures/Screenshots"];

/// Whether a phone notification is about a screenshot, so it gets Copy and Save.
pub fn is_screenshot(app: &str, title: &str, text: &str) -> bool {
    [app, title, text]
        .iter()
        .any(|s| s.to_lowercase().contains("screenshot"))
}

/// Downloads the newest screenshot on the phone into Tether's data folder; returns where it went
/// and its name on the phone (already dated, like `Screenshot_20261112-185300.jpg`).
pub async fn newest_screenshot(svc: &Service, dev: &str) -> Result<(PathBuf, String)> {
    let login = svc.sftp_login(dev).await.map_err(|e| match e {
        Error::Timeout => anyhow!(
            "Your phone didn't share its files. In KDE Connect on the phone, open this computer, \
             then Plugin settings, and turn on Filesystem expose."
        ),
        Error::NotConnected(_) => anyhow!("Your phone isn't connected."),
        Error::Protocol(msg) => anyhow!("Your phone said: {msg}"),
        e => e.into(),
    })?;
    let dir = svc.data_dir().join("phone-files");
    std::fs::create_dir_all(&dir)?;
    let key = dir.join("key.pem");
    write_private(&key, svc.key_pem().as_bytes())?;

    let mut list = String::new();
    for root in &login.roots {
        for sub in SCREENSHOT_DIRS {
            let path = format!("{}/{sub}", root.trim_end_matches('/'));
            list.push_str(&format!("ls -1t {}\n", quote(&path)?));
        }
    }
    let out = run_sftp(&login, &key, &list).await?;
    let remote = newest_image(&out).ok_or_else(|| {
        anyhow!(
            "There are no screenshots on your phone, or it doesn't share the folder they are in."
        )
    })?;
    let name = remote
        .rsplit('/')
        .next()
        .unwrap_or("screenshot.png")
        .to_string();
    let local = dir.join(format!("latest.{}", extension(&name)));
    let _ = std::fs::remove_file(&local);
    let local_str = local.to_str().context("data folder path isn't text")?;
    run_sftp(
        &login,
        &key,
        &format!("get {} {}\n", quote(&remote)?, quote(local_str)?),
    )
    .await?;
    if !local.is_file() {
        bail!("Couldn't download the screenshot from your phone.");
    }
    Ok((local, name))
}

/// Copies the newest screenshot to the clipboard as an image.
pub async fn copy_screenshot(svc: &Service, dev: &str) -> Result<()> {
    let (path, _) = newest_screenshot(svc, dev).await?;
    tokio::task::spawn_blocking(move || set_clipboard_image(&path)).await?
}

/// Saves the newest screenshot into the downloads folder; returns where it went.
pub async fn save_screenshot(svc: &Service, dev: &str) -> Result<PathBuf> {
    let (path, name) = newest_screenshot(svc, dev).await?;
    Ok(svc.save_to_downloads(&path, &name)?)
}

async fn run_sftp(login: &Login, key: &Path, commands: &str) -> Result<String> {
    let exe = std::env::current_exe().context("finding Tether's own program")?;
    let mut child = Command::new("sftp")
        .args(["-q", "-P", &login.port.to_string(), "-i"])
        .arg(key)
        .args([
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "PreferredAuthentications=publickey,password",
            "-o",
            "NumberOfPasswordPrompts=1",
        ])
        .arg(format!("{}@{}", login.user, login.ip))
        .env("SSH_ASKPASS", exe)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(ASKPASS_ENV, &login.password)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow!(
                "Getting files from the phone needs OpenSSH (the sftp program). Install the \
                 openssh-client package and try again."
            ),
            _ => anyhow!("Couldn't start sftp: {e}"),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(commands.as_bytes()).await?;
        stdin.write_all(b"bye\n").await?;
    }
    let out = tokio::time::timeout(Duration::from_secs(90), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("Your phone stopped answering."))??;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        tracing::warn!("sftp to the phone failed: {}", err.trim());
        // "No such file" for a missing folder still exits 0 when fed from stdin; a failure here
        // means the connection or the login itself.
        bail!("Couldn't log in to your phone's files.");
    }
    Ok(stdout)
}

/// First image path in `ls -1t` output (newest first, folders in the order asked).
fn newest_image(out: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("sftp>"))
        .find(|l| {
            let l = l.to_lowercase();
            [".png", ".jpg", ".jpeg", ".webp"]
                .iter()
                .any(|e| l.ends_with(e))
        })
        .map(String::from)
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| ["png", "jpg", "jpeg", "webp"].contains(&e.as_str()))
        .unwrap_or_else(|| "png".into())
}

/// sftp's command parser takes double-quoted arguments; refuse what it can't quote safely.
fn quote(s: &str) -> Result<String> {
    if s.contains(['"', '\n', '\r', '\\']) {
        bail!("Unusual file name on the phone: {s}");
    }
    Ok(format!("\"{s}\""))
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    Ok(())
}

/// The clipboard keeps the image only while its owner lives (X11), so the daemon holds one
/// clipboard for its whole life.
fn set_clipboard_image(path: &Path) -> Result<()> {
    let img = image::open(path).context("The screenshot couldn't be read.")?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    static CLIP: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    let mut guard = CLIP
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new().context("No clipboard on this desktop.")?);
    }
    let clip = guard.as_mut().context("No clipboard on this desktop.")?;
    clip.set_image(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: rgba.into_raw().into(),
    })
    .context("Couldn't put the screenshot on the clipboard.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_first_image_and_spots_screenshots() {
        let out = "sftp> ls -1t \"/s/DCIM/Screenshots\"\n/s/DCIM/Screenshots/.thumbs\n\
                   /s/DCIM/Screenshots/Screenshot_20261112.jpg\n/s/DCIM/Screenshots/old.png\n";
        assert_eq!(
            newest_image(out).as_deref(),
            Some("/s/DCIM/Screenshots/Screenshot_20261112.jpg")
        );
        assert!(newest_image("sftp> ls\n").is_none());
        assert!(is_screenshot("System UI", "Screenshot saved", ""));
        assert!(!is_screenshot("WhatsApp", "Leo", "see you"));
        assert!(quote("a\"b").is_err());
        assert_eq!(extension("x.JPG"), "jpg");
    }
}
