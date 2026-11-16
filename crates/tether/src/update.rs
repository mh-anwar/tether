//! Updates from the project's GitHub releases.
//!
//! The background service checks once a day. Installs Tether can replace by itself (the no-admin
//! `install.sh` folder and AppImages) download the new version, check it against the release's
//! `SHA256SUMS`, and swap the files in place; the new version runs after a restart, which the window
//! offers. `.deb` and `.rpm` installs need the admin password, so on request the installer is saved to
//! Downloads and opened. A Flatpak from Flathub updates from the store; a `.flatpak` bundle has no
//! remote, so it is pointed at the releases page. Builds from a source tree never update. A private repository answers 404, which reads as "no update".

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::exec::{download, fetch_text, run, which};

pub const DEFAULT_URL: &str = "https://api.github.com/repos/mh-anwar/tether/releases/latest";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const TARBALL: &str = "tether-linux-x86_64.tar.gz";
const SUMS: &str = "SHA256SUMS";
const FIRST_CHECK: Duration = Duration::from_secs(90);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How this copy of Tether was installed, which decides how it updates.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// Built from source (`target/debug`, `target/release`): never updates.
    Dev,
    /// Flatpak installed from Flathub: the store updates it.
    Store,
    AppImage(PathBuf),
    /// `install.sh` or any folder this user can write: `tether` and `tether-app` side by side.
    Folder(PathBuf),
    Deb,
    Rpm,
    /// Somewhere we can't write and no known package manager: point at the download page.
    Other,
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::Dev => "dev",
            Kind::Store => "store",
            Kind::AppImage(_) => "appimage",
            Kind::Folder(_) => "folder",
            Kind::Deb => "deb",
            Kind::Rpm => "rpm",
            Kind::Other => "other",
        }
    }

    /// Replaces its own files, so updating needs no password and can happen in the background.
    fn self_updates(&self) -> bool {
        matches!(self, Kind::AppImage(_) | Kind::Folder(_))
    }

    /// The release file this install updates from.
    fn asset_matches(&self, name: &str) -> bool {
        match self {
            Kind::AppImage(_) => name.ends_with(".AppImage"),
            Kind::Folder(_) => name == TARBALL,
            Kind::Deb => name.ends_with(".deb"),
            Kind::Rpm => name.ends_with(".rpm"),
            _ => false,
        }
    }
}

/// The running binary's path. Linux appends " (deleted)" once an update has replaced the file.
pub fn exe_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?;
    let s = p.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(s) => PathBuf::from(s),
        None => p,
    })
}

/// `flatpak`: inside a Flatpak, whether a store updates it.
fn detect(exe: &Path, appimage: Option<PathBuf>, flatpak: Option<bool>) -> Kind {
    match flatpak {
        Some(true) => return Kind::Store,
        Some(false) => return Kind::Other,
        None => {}
    }
    if let Some(a) = appimage {
        return Kind::AppImage(a);
    }
    let s = exe.to_string_lossy();
    if s.contains("/target/debug/") || s.contains("/target/release/") {
        return Kind::Dev;
    }
    let Some(dir) = exe.parent() else {
        return Kind::Other;
    };
    if (s.starts_with("/usr/") && !s.starts_with("/usr/local/")) || s.starts_with("/opt/") {
        if which("dpkg").is_some() && Path::new("/var/lib/dpkg/status").exists() {
            return Kind::Deb;
        }
        if which("rpm").is_some() {
            return Kind::Rpm;
        }
        return Kind::Other;
    }
    if writable(dir) {
        Kind::Folder(dir.to_path_buf())
    } else {
        Kind::Other
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".tether-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn kind() -> Kind {
    let exe = exe_path().unwrap_or_default();
    let appimage = std::env::var_os("APPIMAGE")
        .filter(|a| !a.is_empty())
        .map(PathBuf::from);
    let flatpak = crate::flatpak::active().then(crate::flatpak::store_updates);
    detect(&exe, appimage, flatpak)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    /// New files are in place; a restart runs them.
    Ready,
    /// A package installer was saved and opened; the owner finishes it.
    Installer,
}

struct State {
    url: String,
    auto: bool,
    phase: Phase,
    error: Option<String>,
    latest: Option<Release>,
    /// The last check reached the server but it had no release to show (a private repository).
    no_releases: bool,
    /// Where a package installer was saved.
    installer: Option<PathBuf>,
}

static STATE: Mutex<State> = Mutex::new(State {
    url: String::new(),
    auto: true,
    phase: Phase::Idle,
    error: None,
    latest: None,
    no_releases: false,
    installer: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    fn asset(&self, pick: impl Fn(&str) -> bool) -> Option<&Asset> {
        self.assets.iter().find(|a| pick(&a.name))
    }
}

/// "1.2.3" (a "-beta" style suffix is ignored) as numbers, for comparing.
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let core = v.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// The hash for `file` in a `sha256sum` listing ("<hex>  <name>", or "<hex> *<name>").
fn sum_for<'a>(sums: &'a str, file: &str) -> Option<&'a str> {
    sums.lines().find_map(|l| {
        let (hash, name) = l.trim().split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file && hash.len() == 64).then_some(hash)
    })
}

/// Called once by the daemon: remembers the settings and checks now and then from then on.
pub fn start(url: Option<String>, auto: bool) {
    {
        let mut s = state();
        s.url = url.unwrap_or_else(|| DEFAULT_URL.to_string());
        s.auto = auto;
    }
    if matches!(kind(), Kind::Dev | Kind::Store) {
        return;
    }
    tokio::spawn(async {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let auto = state().auto;
            if let Err(e) = check(auto).await {
                warn!("update check: {e:#}");
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Settings screen: install new versions by themselves, or only offer them.
pub fn set_auto(auto: bool) {
    state().auto = auto;
}

pub fn status() -> Value {
    let k = kind();
    let s = state();
    let latest = s.latest.as_ref().filter(|r| is_newer(r.version(), CURRENT));
    json!({
        "current": CURRENT,
        "latest": latest.map(|r| r.version()),
        "page": latest.map(|r| r.html_url.clone()),
        "kind": k.name(),
        "selfUpdates": k.self_updates(),
        "phase": s.phase,
        "error": s.error,
        "noReleases": s.no_releases,
        "installer": s.installer.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "installCommand": s.installer.as_ref().map(|p| install_command(&k, p)),
    })
}

fn install_command(k: &Kind, file: &Path) -> String {
    let f = file.to_string_lossy();
    match k {
        Kind::Rpm => format!("sudo dnf install '{f}'"),
        _ => format!("sudo apt install '{f}'"),
    }
}

/// Looks for a newer release; with `install`, also installs it when this kind of install can update
/// itself. Returns the status.
pub async fn check(install: bool) -> Result<Value> {
    let (url, before) = {
        let mut s = state();
        if matches!(s.phase, Phase::Checking | Phase::Downloading) {
            drop(s);
            return Ok(status());
        }
        let before = s.phase;
        s.phase = Phase::Checking;
        s.error = None;
        (s.url.clone(), before)
    };
    let result = fetch_release(&url).await;
    let newer = {
        let mut s = state();
        // A finished install or a saved installer stays on screen until the restart.
        s.phase = before;
        s.no_releases = matches!(result, Ok(None));
        match result {
            Ok(Some(r)) => {
                let newer = is_newer(r.version(), CURRENT);
                if newer {
                    info!("update: {} is out (running {CURRENT})", r.version());
                }
                s.latest = Some(r);
                newer
            }
            Ok(None) => false,
            Err(e) => {
                s.error = Some(format!("Couldn't check for updates: {e}"));
                false
            }
        }
    };
    if newer && install && kind().self_updates() {
        let _ = self::install().await;
    }
    Ok(status())
}

async fn fetch_release(url: &str) -> Result<Option<Release>> {
    let Some(text) = fetch_text(url, Duration::from_secs(30)).await? else {
        return Ok(None);
    };
    Ok(Some(
        serde_json::from_str(&text).context("reading the release list")?,
    ))
}

/// Installs the newest release, or for packages saves and opens its installer.
pub async fn install() -> Result<Value> {
    let k = kind();
    let release = {
        let mut s = state();
        if s.phase == Phase::Downloading {
            drop(s);
            return Ok(status());
        }
        let Some(r) = s.latest.clone().filter(|r| is_newer(r.version(), CURRENT)) else {
            bail!("Tether is up to date.");
        };
        s.phase = Phase::Downloading;
        s.error = None;
        r
    };
    let result = install_release(&k, &release).await;
    let mut s = state();
    match result {
        Ok(Some(installer)) => {
            s.installer = Some(installer);
            s.phase = Phase::Installer;
        }
        Ok(None) => {
            info!(
                "update: {} installed; it runs after a restart",
                release.version()
            );
            s.phase = Phase::Ready;
        }
        Err(e) => {
            warn!("update: {e:#}");
            s.phase = Phase::Idle;
            s.error = Some(format!("Couldn't update: {e}"));
            return Err(e);
        }
    }
    drop(s);
    Ok(status())
}

/// Some(path) when an installer was saved for the owner to finish, None when the files were replaced.
async fn install_release(k: &Kind, r: &Release) -> Result<Option<PathBuf>> {
    let asset = r
        .asset(|n| k.asset_matches(n))
        .ok_or_else(|| anyhow!("this release has no download for this kind of install"))?;
    let sums_asset = r
        .asset(|n| n == SUMS)
        .ok_or_else(|| anyhow!("this release has no checksums, so it can't be verified"))?;
    let sums = fetch_text(&sums_asset.browser_download_url, Duration::from_secs(30))
        .await?
        .ok_or_else(|| anyhow!("check the internet connection"))?;
    let want = sum_for(&sums, &asset.name)
        .ok_or_else(|| anyhow!("the checksum list doesn't cover {}", asset.name))?
        .to_ascii_lowercase();

    let work = dirs::data_dir()
        .ok_or_else(|| anyhow!("no data folder"))?
        .join("tether/update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let file = work.join(&asset.name);
    download(&asset.browser_download_url, &file, Duration::from_secs(900)).await?;
    let got = hex::encode(Sha256::digest(std::fs::read(&file)?));
    if got != want {
        let _ = std::fs::remove_dir_all(&work);
        bail!("the download was damaged on the way");
    }

    let result = match k {
        Kind::AppImage(path) => replace(&file, path).map(|_| None),
        Kind::Folder(dir) => replace_folder(&file, &work, dir).await.map(|_| None),
        Kind::Deb | Kind::Rpm => {
            let dest = downloads_dir().join(&asset.name);
            std::fs::create_dir_all(downloads_dir())?;
            std::fs::copy(&file, &dest)?;
            open_installer(&dest);
            Ok(Some(dest))
        }
        _ => Err(anyhow!("this copy of Tether can't update itself")),
    };
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Puts `new` at `target` in one step: copy next to it, make it runnable, rename over. Running
/// copies keep the old file until they exit.
fn replace(new: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("no folder for {}", target.display()))?;
    let name = target
        .file_name()
        .ok_or_else(|| anyhow!("no file name"))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.new"));
    std::fs::copy(new, &tmp).with_context(|| format!("writing in {}", dir.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

async fn replace_folder(tarball: &Path, work: &Path, dir: &Path) -> Result<()> {
    let tar = which("tar").ok_or_else(|| anyhow!("tar isn't installed"))?;
    let out = work.join("unpacked");
    std::fs::create_dir_all(&out)?;
    let (ok, msg) = run(
        &tar,
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &out.to_string_lossy(),
        ],
        Duration::from_secs(120),
    )
    .await?;
    if !ok {
        bail!("unpacking failed: {}", msg.trim());
    }
    let src = out.join("tether-linux-x86_64");
    // Both first checked, so a broken archive can't leave a new daemon next to an old window.
    for bin in ["tether", "tether-app"] {
        if !crate::exec::is_executable(&src.join(bin)) {
            bail!("the download is missing {bin}");
        }
    }
    for bin in ["tether", "tether-app"] {
        let target = dir.join(bin);
        if bin == "tether" || target.exists() {
            replace(&src.join(bin), &target)?;
        }
    }
    Ok(())
}

fn downloads_dir() -> PathBuf {
    crate::config::Config::load()
        .ok()
        .map(|c| c.core().downloads_dir)
        .unwrap_or_else(std::env::temp_dir)
}

/// Opens the saved package with the desktop's software installer, where there is one.
fn open_installer(file: &Path) {
    if let Some(open) = which("xdg-open") {
        let _ = std::process::Command::new(open)
            .arg(file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("nightly", "0.2.0"));
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((1, 2, 3)));
    }

    #[test]
    fn checksums() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  Tether_0.3.0_amd64.deb\n{}  {TARBALL}\n",
            "b".repeat(64)
        );
        assert_eq!(sum_for(&sums, "Tether_0.3.0_amd64.deb"), Some(h.as_str()));
        assert_eq!(sum_for(&sums, TARBALL), Some("b".repeat(64).as_str()));
        assert_eq!(sum_for(&sums, "other"), None);
        assert_eq!(sum_for(&format!("{h} *x.rpm"), "x.rpm"), Some(h.as_str()));
        assert_eq!(sum_for("short  x.rpm", "x.rpm"), None);
    }

    #[test]
    fn kinds() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(detect(&p("/app/bin/tether"), None, Some(true)), Kind::Store);
        assert_eq!(detect(&p("/app/bin/tether"), None, Some(false)), Kind::Other);
        assert_eq!(
            detect(
                &p("/tmp/.mount_x/usr/bin/tether"),
                Some(p("/home/a/T.AppImage")),
                None
            ),
            Kind::AppImage(p("/home/a/T.AppImage"))
        );
        assert_eq!(
            detect(&p("/src/t/target/debug/tether"), None, None),
            Kind::Dev
        );
        let dir = std::env::temp_dir();
        assert_eq!(detect(&dir.join("tether"), None, None), Kind::Folder(dir));
        assert!(Kind::Folder(p("/x")).asset_matches(TARBALL));
        assert!(!Kind::Folder(p("/x")).asset_matches("Tether_0.3.0_amd64.deb"));
        assert!(Kind::Deb.asset_matches("Tether_0.3.0_amd64.deb"));
        assert!(Kind::AppImage(p("/x")).asset_matches("Tether_0.3.0_amd64.AppImage"));
    }

    #[test]
    fn release_json() {
        let r: Release = serde_json::from_str(
            r#"{"tag_name":"v0.3.0","html_url":"https://example.org/r","assets":[
                {"name":"SHA256SUMS","browser_download_url":"https://example.org/s"}]}"#,
        )
        .unwrap();
        assert_eq!(r.version(), "0.3.0");
        assert!(r.asset(|n| n == SUMS).is_some());
    }
}
