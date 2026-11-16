//! Files from the phone through its file server ("Filesystem expose" in KDE Connect), using the
//! desktop's OpenSSH `sftp` client. kdeconnect-kde mounts the same server with sshfs, with the
//! same login (our certificate's key, then the one-time password) and also without pinning the
//! server's host key: the login itself arrives over the authenticated TLS link.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
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

/// Camera photos: every phone uses DCIM/Camera.
const CAMERA_DIRS: [&str; 1] = ["DCIM/Camera"];
/// How many pictures of each kind the Photos page lists.
const LIST_LIMIT: usize = 60;
/// Full-size pictures kept on disk after viewing; older ones are deleted.
const FULL_KEEP: usize = 20;
const THUMB_SIZE: u32 = 360;

fn explain(e: Error) -> anyhow::Error {
    match e {
        Error::Timeout => anyhow!(
            "Your phone didn't share its files. In KDE Connect on the phone, open this computer, \
             then Plugin settings, and turn on Filesystem expose."
        ),
        Error::NotConnected(_) => anyhow!("Your phone isn't connected."),
        Error::Protocol(msg) => anyhow!("Your phone said: {msg}"),
        e => e.into(),
    }
}

fn files_dir(svc: &Service) -> PathBuf {
    svc.data_dir().join("phone-files")
}

/// The phone keeps its file server up while connected, so a login is reused for a few minutes
/// instead of asking again for every batch of thumbnails.
static LOGIN: Mutex<Option<(Instant, String, Login)>> = Mutex::new(None);
const LOGIN_FOR: Duration = Duration::from_secs(300);

async fn login(svc: &Service, dev: &str, fresh: bool) -> Result<Login> {
    if !fresh {
        let cached = LOGIN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, d, l)) = cached.as_ref() {
            if d == dev && at.elapsed() < LOGIN_FOR {
                return Ok(l.clone());
            }
        }
    }
    let l = svc.sftp_login(dev).await.map_err(explain)?;
    *LOGIN.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), dev.to_string(), l.clone()));
    Ok(l)
}

/// Runs sftp commands on the phone. `commands` gets the login (for the shared folders) and
/// returns the script; a remembered login that no longer works is replaced once.
async fn sftp(
    svc: &Service,
    dev: &str,
    commands: impl Fn(&Login) -> Result<String>,
) -> Result<String> {
    let dir = files_dir(svc);
    std::fs::create_dir_all(&dir)?;
    let key = dir.join("key.pem");
    write_private(&key, svc.key_pem().as_bytes())?;
    let first = login(svc, dev, false).await?;
    match run_sftp(&first, &key, &commands(&first)?).await {
        Ok(out) => Ok(out),
        Err(e) if e.to_string().starts_with("Phone files:") => Err(e),
        Err(_) => {
            let again = login(svc, dev, true).await?;
            run_sftp(&again, &key, &commands(&again)?).await
        }
    }
}

/// One listing per session; metadata uses numeric owner/group so names cannot shift columns.
pub async fn list_files(svc: &Service, dev: &str, path: Option<&str>) -> Result<serde_json::Value> {
    let l = login(svc, dev, false).await?;
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Ok(serde_json::json!({"roots": l.roots, "entries": []}));
    };
    let out = sftp(svc, dev, |_| Ok(format!("ls -lan {}\n", quote(path)?))).await?;
    let entries: Vec<_> = out
        .lines()
        .filter_map(|line| parse_entry(line, path))
        .collect();
    Ok(serde_json::json!({"roots": l.roots, "entries": entries}))
}

fn parse_entry(line: &str, parent: &str) -> Option<serde_json::Value> {
    let mut rest = line.trim();
    let mut fields = Vec::new();
    for _ in 0..8 {
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = rest[end..].trim_start();
    }
    let mode = fields[0];
    if !mode.starts_with(['d', '-']) || rest == "." || rest == ".." {
        return None;
    }
    let name = rest.rsplit('/').next()?;
    if name == "." || name == ".." {
        return None;
    }
    Some(serde_json::json!({"name": name,
        "path": format!("{}/{}", parent.trim_end_matches('/'), name),
        "directory": mode.starts_with('d'), "size": fields[4].parse::<u64>().ok()?,
        "date": format!("{} {} {}", fields[5], fields[6], fields[7])}))
}

/// Every mutation is a single command in the existing authenticated SFTP path.
pub async fn file_operation(
    svc: &Service,
    dev: &str,
    action: &str,
    path: &str,
    target: Option<&str>,
    directory: bool,
) -> Result<serde_json::Value> {
    let script = match action {
        "mkdir" => format!("mkdir {}\n", quote(path)?),
        "delete" => format!(
            "{} {}\n",
            if directory { "rmdir" } else { "rm" },
            quote(path)?
        ),
        "rename" => format!(
            "rename {} {}\n",
            quote(path)?,
            quote(target.context("Missing new name")?)?
        ),
        "upload" => {
            let local = target.context("Missing local file")?;
            if !Path::new(local).is_file() {
                bail!("Choose a file to upload.");
            }
            format!("put {} {}\n", quote(local)?, quote(path)?)
        }
        "open" | "save" => {
            if matches!(file_name(path), "" | "." | "..") {
                bail!("Choose a file, not a folder.");
            }
            let dir = files_dir(svc).join("files").join(local_name(path));
            std::fs::create_dir_all(&dir)?;
            let local = dir.join(file_name(path));
            fetch(svc, dev, &[(path.into(), local.clone())]).await?;
            if !local.is_file() {
                bail!("The phone did not send this file.");
            }
            if action == "save" {
                let saved = svc.save_to_downloads(&local, file_name(path))?;
                return Ok(serde_json::json!({"path": saved}));
            }
            Command::new("xdg-open")
                .arg(&local)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("Could not open the file")?;
            return Ok(serde_json::json!({"path": local}));
        }
        _ => bail!("Unknown file operation"),
    };
    sftp(svc, dev, |_| Ok(script.clone())).await?;
    Ok(serde_json::json!(true))
}

fn list_script(login: &Login, dirs: &[&str]) -> Result<String> {
    let mut list = String::new();
    for root in &login.roots {
        for sub in dirs {
            let path = format!("{}/{sub}", root.trim_end_matches('/'));
            list.push_str(&format!("ls -1t {}\n", quote(&path)?));
        }
    }
    Ok(list)
}

/// Fetches phone files into local paths in one session.
async fn fetch(svc: &Service, dev: &str, files: &[(String, PathBuf)]) -> Result<()> {
    let mut script = String::new();
    for (remote, local) in files {
        let _ = std::fs::remove_file(local);
        let local = local.to_str().context("data folder path isn't text")?;
        script.push_str(&format!("get {} {}\n", quote(remote)?, quote(local)?));
    }
    sftp(svc, dev, |_| Ok(script.clone())).await?;
    Ok(())
}

/// Recent pictures on the phone, newest first within each kind: camera photos and screenshots.
pub async fn list_photos(svc: &Service, dev: &str) -> Result<serde_json::Value> {
    let mut folders = vec![];
    for (name, dirs) in [
        ("Camera", &CAMERA_DIRS[..]),
        ("Screenshots", &SCREENSHOT_DIRS[..]),
    ] {
        let out = sftp(svc, dev, |l| list_script(l, dirs)).await?;
        let items: Vec<String> = images(&out).take(LIST_LIMIT).collect();
        folders.push(serde_json::json!({"name": name, "items": items}));
    }
    Ok(serde_json::json!({ "folders": folders }))
}

/// A stable local file name for a picture on the phone.
fn local_name(remote: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(&Sha256::digest(remote.as_bytes())[..12]);
    format!("{hash}.{}", extension(remote))
}

fn check_picture(remote: &str) -> Result<()> {
    if !is_image(remote) {
        bail!("That isn't a picture Tether can show.");
    }
    Ok(())
}

/// Small previews of phone pictures, made once and kept: remote path -> local file (null when
/// that picture couldn't be fetched or read).
pub async fn thumbs(svc: &Service, dev: &str, remotes: &[String]) -> Result<serde_json::Value> {
    let dir = files_dir(svc).join("thumbs");
    let tmp = files_dir(svc).join("tmp");
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(&tmp)?;
    let mut out = serde_json::Map::new();
    let mut missing = vec![];
    for r in remotes {
        check_picture(r)?;
        let thumb = dir.join(local_name(r)).with_extension("jpg");
        if thumb.is_file() {
            out.insert(r.clone(), thumb.to_string_lossy().into());
        } else {
            missing.push((r.clone(), tmp.join(local_name(r)), thumb));
        }
    }
    if !missing.is_empty() {
        let files: Vec<_> = missing
            .iter()
            .map(|(r, t, _)| (r.clone(), t.clone()))
            .collect();
        fetch(svc, dev, &files).await?;
        let made = tokio::task::spawn_blocking(move || {
            missing
                .into_iter()
                .map(|(remote, full, thumb)| {
                    let ok = make_thumb(&full, &thumb).is_ok();
                    let _ = std::fs::remove_file(&full);
                    (remote, ok.then_some(thumb))
                })
                .collect::<Vec<_>>()
        })
        .await?;
        for (remote, thumb) in made {
            out.insert(
                remote,
                thumb.map_or(serde_json::Value::Null, |t| t.to_string_lossy().into()),
            );
        }
    }
    Ok(out.into())
}

fn open_upright(path: &Path) -> Result<image::DynamicImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    // Phone cameras store the picture sideways and say so in the file.
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn make_thumb(full: &Path, thumb: &Path) -> Result<()> {
    let small = open_upright(full)?
        .thumbnail(THUMB_SIZE, THUMB_SIZE)
        .to_rgb8();
    let tmp = thumb.with_extension("part");
    small.save_with_format(&tmp, image::ImageFormat::Jpeg)?;
    std::fs::rename(tmp, thumb)?;
    Ok(())
}

/// Downloads one picture in full size for viewing, copying or saving.
pub async fn full_picture(svc: &Service, dev: &str, remote: &str) -> Result<PathBuf> {
    check_picture(remote)?;
    let dir = files_dir(svc).join("full");
    std::fs::create_dir_all(&dir)?;
    let local = dir.join(local_name(remote));
    if !local.is_file() {
        let part = local.with_extension("part");
        fetch(svc, dev, &[(remote.to_string(), part.clone())]).await?;
        if !part.is_file() {
            bail!("Couldn't download the picture from your phone.");
        }
        std::fs::rename(part, &local)?;
        prune(&dir, FULL_KEEP);
    }
    Ok(local)
}

/// Keeps the `keep` newest files of a folder.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

fn file_name(remote: &str) -> &str {
    remote.rsplit('/').next().unwrap_or("picture.jpg")
}

/// Copies a phone picture to the clipboard as an image.
pub async fn copy_picture(svc: &Service, dev: &str, remote: &str) -> Result<()> {
    let path = full_picture(svc, dev, remote).await?;
    tokio::task::spawn_blocking(move || set_clipboard_image(&path)).await?
}

/// Saves a phone picture into the downloads folder under its own name; returns where it went.
pub async fn save_picture(svc: &Service, dev: &str, remote: &str) -> Result<PathBuf> {
    let path = full_picture(svc, dev, remote).await?;
    Ok(svc.save_to_downloads(&path, file_name(remote))?)
}

async fn newest_screenshot(svc: &Service, dev: &str) -> Result<String> {
    let out = sftp(svc, dev, |l| list_script(l, &SCREENSHOT_DIRS)).await?;
    let newest = images(&out).next();
    newest.ok_or_else(|| {
        anyhow!(
            "There are no screenshots on your phone, or it doesn't share the folder they are in."
        )
    })
}

/// Copies the newest screenshot to the clipboard as an image.
pub async fn copy_screenshot(svc: &Service, dev: &str) -> Result<()> {
    let remote = newest_screenshot(svc, dev).await?;
    copy_picture(svc, dev, &remote).await
}

/// Saves the newest screenshot into the downloads folder; returns where it went.
pub async fn save_screenshot(svc: &Service, dev: &str) -> Result<PathBuf> {
    let remote = newest_screenshot(svc, dev).await?;
    save_picture(svc, dev, &remote).await
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
        .env("LC_ALL", "C")
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
    let err = String::from_utf8_lossy(&out.stderr);
    // Photo discovery deliberately probes folders that may not exist.
    if !commands.starts_with("ls -1t") && !err.trim().is_empty() {
        bail!("Phone files: {}", err.trim());
    }
    if !out.status.success() && stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        tracing::warn!("sftp to the phone failed: {}", err.trim());
        // "No such file" for a missing folder still exits 0 when fed from stdin; a failure here
        // means the connection or the login itself.
        bail!("Couldn't log in to your phone's files.");
    }
    Ok(stdout)
}

const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn is_image(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Picture paths in `ls -1t` output (newest first, folders in the order asked).
fn images(out: &str) -> impl Iterator<Item = String> + '_ {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("sftp>") && is_image(l))
        .map(String::from)
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
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
    let img = open_upright(path).context("The picture couldn't be read.")?;
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
    .context("Couldn't put the picture on the clipboard.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_keeps_spaces_and_metadata() {
        let entry = parse_entry(
            "-rw-r--r-- 1 1000 1000 123 Nov 16 12:30 my file.txt",
            "/storage",
        )
        .unwrap();
        assert_eq!(entry["path"], "/storage/my file.txt");
        assert_eq!(entry["size"], 123);
        assert!(parse_entry("sftp> ls", "/").is_none());
    }

    #[test]
    fn listing_skips_dot_entries_and_reads_folders() {
        let dir = parse_entry("drwxrwx--x 2 0 0 4096 Nov 12 2025 Camera", "/s/").unwrap();
        assert_eq!(dir["directory"], true);
        assert_eq!(dir["path"], "/s/Camera");
        assert_eq!(dir["date"], "Nov 12 2025");
        for name in [".", "..", "/s/."] {
            assert!(
                parse_entry(&format!("drwxr-xr-x 2 0 0 4096 Nov 12 18:01 {name}"), "/s").is_none()
            );
        }
        // Server paths in the name column (some servers print full paths) shrink to the name.
        let full = parse_entry("-rw-r--r-- 1 0 0 5 Nov 12 18:01 /s/a.txt", "/s").unwrap();
        assert_eq!(full["name"], "a.txt");
        assert!(parse_entry("total 12", "/s").is_none());
    }

    #[test]
    fn quoting_refuses_what_sftp_cannot_carry() {
        assert_eq!(quote("/s/my file").unwrap(), "\"/s/my file\"");
        for bad in ["a\\b", "a\nb", "a\"b"] {
            assert!(quote(bad).is_err());
        }
    }

    #[test]
    fn picks_first_image_and_spots_screenshots() {
        let out = "sftp> ls -1t \"/s/DCIM/Screenshots\"\n/s/DCIM/Screenshots/.thumbs\n\
                   /s/DCIM/Screenshots/Screenshot_20261112.jpg\n/s/DCIM/Screenshots/old.png\n";
        assert_eq!(
            images(out).collect::<Vec<_>>(),
            [
                "/s/DCIM/Screenshots/Screenshot_20261112.jpg",
                "/s/DCIM/Screenshots/old.png"
            ]
        );
        assert!(images("sftp> ls\n").next().is_none());
        assert_eq!(local_name("/a/b.JPG"), local_name("/a/b.JPG"));
        assert!(local_name("/a/b.JPG").ends_with(".jpg"));
        assert!(check_picture("/a/video.mp4").is_err());
        assert!(is_screenshot("System UI", "Screenshot saved", ""));
        assert!(!is_screenshot("WhatsApp", "Leo", "see you"));
        assert!(quote("a\"b").is_err());
        assert_eq!(extension("x.JPG"), "jpg");
    }
}
