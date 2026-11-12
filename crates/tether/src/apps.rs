//! Phone apps on the desktop: each phone app opens in its own window. KDE Connect can't carry screen
//! contents, so this uses Android's wireless debugging (adb) and scrcpy, which starts the app on a
//! virtual display and streams it (`--new-display --start-app`, scrcpy 3.0+). See docs/decisions.md.
//!
//! Setup is one QR scan: we show `WIFI:T:ADB;S:<name>;P:<password>;;` (the format Android Studio
//! uses), the phone then advertises `_adb-tls-pairing._tcp` under that name, and we `adb pair` with
//! the password. After that the phone advertises `_adb-tls-connect._tcp` and we `adb connect`.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tether_core::Service;
use tokio::process::Command;
use tracing::{info, warn};

/// The helper offered as a one-click download: scrcpy's static Linux build, which also ships adb.
/// Distros mostly package scrcpy 1.x/2.x, too old for app windows. Pinned with its checksum.
const HELPER_VERSION: &str = "v5.0.1";
const HELPER_URL: &str = "https://github.com/Genymobile/scrcpy/releases/download/v5.0.1/scrcpy-linux-x86_64-v5.0.1.tar.gz";
const HELPER_SHA256: &str = "9f969d30cc574816077edecda65719c1c70b0aee1df1ba5fa00779ac07b8fd6e";
const PAIR_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Default)]
struct Apps {
    phase: Phase,
    error: Option<String>,
    /// Name and password of the QR code currently shown.
    qr: Option<(String, String)>,
    /// Bumped on every new pairing attempt so a stale one stops.
    attempt: u64,
    last_autoconnect: Option<Instant>,
    apps: Option<(String, Vec<PhoneApp>)>,
}

#[derive(Default, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    #[default]
    Idle,
    /// QR code on screen, waiting for the phone to scan it.
    Waiting,
    Pairing,
    Connecting,
    Downloading,
}

static STATE: Mutex<Apps> = Mutex::new(Apps {
    phase: Phase::Idle,
    error: None,
    qr: None,
    attempt: 0,
    last_autoconnect: None,
    apps: None,
});

fn state() -> std::sync::MutexGuard<'static, Apps> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn set(phase: Phase, error: Option<String>) {
    let mut s = state();
    s.phase = phase;
    s.error = error;
}

/// Like `set`, but only while `attempt` is still the current pairing attempt, so a slow, stale task
/// can't overwrite a newer one (a fresh QR code, a cancel).
fn set_if(attempt: u64, phase: Phase, error: Option<String>) -> bool {
    let mut s = state();
    if s.attempt != attempt {
        return false;
    }
    s.phase = phase;
    s.error = error;
    true
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhoneApp {
    pub name: String,
    pub package: String,
    pub system: bool,
}

#[derive(Debug, PartialEq)]
struct AdbDevice {
    serial: String,
    online: bool,
    model: Option<String>,
}

#[derive(Debug, PartialEq)]
enum MdnsKind {
    Pairing,
    Connect,
}

#[derive(Debug, PartialEq)]
struct Mdns {
    instance: String,
    kind: MdnsKind,
    addr: SocketAddr,
}

fn helper_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tether/helpers")
        .join(format!("scrcpy-linux-x86_64-{HELPER_VERSION}"))
}

/// Finds a helper program: an explicit override (packagers, tests), then our downloaded copy, then
/// next to our own binary (bundles), then PATH.
fn tool(name: &str) -> Option<PathBuf> {
    let env = format!("TETHER_{}", name.to_uppercase());
    if let Some(p) = std::env::var_os(&env).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return is_executable(&p).then_some(p);
    }
    let mut dirs = vec![helper_dir()];
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        dirs.push(d);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Runs a helper to completion; returns whether it succeeded and stdout + stderr together.
async fn run(prog: &Path, args: &[&str], timeout: Duration) -> Result<(bool, String)> {
    run_input(prog, args, None, timeout).await
}

/// `input` goes to the helper's stdin: secrets go there, never into arguments (visible in /proc).
async fn run_input(
    prog: &Path,
    args: &[&str],
    input: Option<&str>,
    timeout: Duration,
) -> Result<(bool, String)> {
    use tokio::io::AsyncWriteExt;
    let mut child = Command::new(prog)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("running {}", prog.display()))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(format!("{text}\n").as_bytes()).await;
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("{} took too long", prog.display()))?
        .with_context(|| format!("running {}", prog.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

async fn adb(args: &[&str], timeout: Duration) -> Result<(bool, String)> {
    let prog = tool("adb").ok_or_else(|| anyhow!("adb isn't installed"))?;
    run(&prog, args, timeout).await
}

/// `adb devices -l`: "SERIAL  device product:x model:SM_S911B device:dm1q transport_id:1".
fn parse_devices(out: &str) -> Vec<AdbDevice> {
    out.lines()
        .filter(|l| !l.starts_with("List of") && !l.starts_with('*'))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let serial = words.next()?.to_string();
            let status = words.next()?;
            let model = words
                .find_map(|w| w.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            Some(AdbDevice {
                serial,
                online: status == "device",
                model,
            })
        })
        .collect()
}

/// `adb mdns services`: "instance<TAB>_adb-tls-connect._tcp<TAB>192.168.1.5:37199".
fn parse_mdns(out: &str) -> Vec<Mdns> {
    out.lines()
        .filter_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let kind_at = words.iter().position(|w| w.contains("_adb-tls-"))?;
            let kind = if words[kind_at].contains("pairing") {
                MdnsKind::Pairing
            } else if words[kind_at].contains("connect") {
                MdnsKind::Connect
            } else {
                return None;
            };
            Some(Mdns {
                instance: words[..kind_at].join(" "),
                kind,
                addr: words.last()?.parse().ok()?,
            })
        })
        .collect()
}

/// `scrcpy --list-apps`: after a "List of apps:" log line, " * Name   package" for system apps and
/// " - Name   package" for the rest (scrcpy server/.../util/LogUtils.java, buildAppListMessage).
fn parse_apps(out: &str) -> Vec<PhoneApp> {
    out.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (system, rest) = match l.strip_prefix("* ") {
                Some(r) => (true, r),
                None => (false, l.strip_prefix("- ")?),
            };
            let rest = rest.trim_end();
            let (name, package) = rest.rsplit_once(' ')?;
            let name = name.trim();
            if name.is_empty() || !package.contains('.') {
                return None;
            }
            Some(PhoneApp {
                name: name.to_string(),
                package: package.to_string(),
                system,
            })
        })
        .collect()
}

fn scrcpy_major(version_out: &str) -> Option<u32> {
    let v = version_out.split_whitespace().nth(1)?;
    v.split('.').next()?.parse().ok()
}

/// IPs of the paired phones KDE Connect currently reaches, to pick the right adb device.
fn phone_ips(svc: &Service) -> Vec<String> {
    svc.devices()
        .into_iter()
        .filter(|d| d.connected)
        .filter_map(|d| d.address)
        .filter_map(|a| {
            a.parse::<SocketAddr>()
                .map(|s| s.ip().to_string())
                .ok()
                .or(Some(a))
        })
        .collect()
}

async fn online_device(svc: &Service) -> Result<Option<AdbDevice>> {
    let (_, out) = adb(&["devices", "-l"], Duration::from_secs(15)).await?;
    let mut devs: Vec<AdbDevice> = parse_devices(&out)
        .into_iter()
        .filter(|d| d.online)
        .collect();
    let ips = phone_ips(svc);
    devs.sort_by_key(|d| !ips.iter().any(|ip| d.serial.starts_with(&format!("{ip}:"))));
    Ok(devs.into_iter().next())
}

fn paired_marker() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tether/apps-paired")
}

pub async fn status(svc: &Service) -> Result<Value> {
    let adb_path = tool("adb");
    let scrcpy_path = tool("scrcpy");
    let scrcpy_major = match &scrcpy_path {
        Some(p) => run(p, &["--version"], Duration::from_secs(5))
            .await
            .ok()
            .and_then(|(_, out)| scrcpy_major(&out)),
        None => None,
    };
    let device = if adb_path.is_some() {
        match online_device(svc).await {
            Ok(d) => d,
            Err(e) => {
                warn!("adb: {e:#}");
                None
            }
        }
    } else {
        None
    };
    if device.is_none() && adb_path.is_some() {
        maybe_autoconnect(svc);
    }
    let s = state();
    let qr = match (&s.qr, s.phase) {
        (Some((name, pw)), Phase::Waiting) => {
            Some(qr_svg(&format!("WIFI:T:ADB;S:{name};P:{pw};;"))?)
        }
        _ => None,
    };
    Ok(json!({
        "adb": adb_path.is_some(),
        "scrcpy": scrcpy_path.is_some(),
        "scrcpyTooOld": scrcpy_major.is_some_and(|m| m < 3),
        "canDownload": cfg!(target_arch = "x86_64"),
        "phase": s.phase,
        "error": s.error,
        "qr": qr,
        "pairedBefore": paired_marker().exists(),
        "device": device.map(|d| json!({"serial": d.serial, "model": d.model})),
    }))
}

/// Phones that were paired once come back by themselves when wireless debugging is on: adb usually
/// reconnects on its own, and we also connect to any advertised phone at the KDE Connect address.
fn maybe_autoconnect(svc: &Service) {
    {
        let mut s = state();
        if s.phase != Phase::Idle
            || s.last_autoconnect
                .is_some_and(|t| t.elapsed() < Duration::from_secs(10))
            || !paired_marker().exists()
        {
            return;
        }
        s.last_autoconnect = Some(Instant::now());
    }
    let ips = phone_ips(svc);
    tokio::spawn(async move {
        let Ok((_, out)) = adb(&["mdns", "services"], Duration::from_secs(10)).await else {
            return;
        };
        for m in parse_mdns(&out) {
            if m.kind == MdnsKind::Connect && ips.contains(&m.addr.ip().to_string()) {
                let _ = adb(&["connect", &m.addr.to_string()], Duration::from_secs(10)).await;
            }
        }
    });
}

fn random_hex(n: usize) -> String {
    let mut s = String::new();
    while s.len() < n {
        s.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    s.truncate(n);
    s
}

pub fn pair_qr(svc: &Service) -> Result<Value> {
    tool("adb").ok_or_else(|| anyhow!("adb isn't installed"))?;
    let name = format!("tether-{}", random_hex(6));
    let password = random_hex(12);
    let attempt = {
        let mut s = state();
        s.attempt += 1;
        s.qr = Some((name.clone(), password.clone()));
        s.phase = Phase::Waiting;
        s.error = None;
        s.attempt
    };
    let svc = svc.clone();
    tokio::spawn(async move {
        let result = wait_for_scan(&name, &password, attempt).await;
        match result {
            Ok(ip) => finish_connect(&svc, Some(ip), attempt).await,
            Err(e) => {
                set_if(attempt, Phase::Idle, Some(e.to_string()));
            }
        }
    });
    Ok(json!(true))
}

async fn wait_for_scan(name: &str, password: &str, attempt: u64) -> Result<String> {
    let start = Instant::now();
    while start.elapsed() < PAIR_TIMEOUT {
        if state().attempt != attempt {
            bail!("cancelled");
        }
        if let Ok((_, out)) = adb(&["mdns", "services"], Duration::from_secs(10)).await {
            if let Some(m) = parse_mdns(&out)
                .into_iter()
                .find(|m| m.kind == MdnsKind::Pairing && m.instance == name)
            {
                if !set_if(attempt, Phase::Pairing, None) {
                    bail!("cancelled");
                }
                pair(&m.addr.to_string(), password).await?;
                return Ok(m.addr.ip().to_string());
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("Your phone didn't respond. Check it's on the same Wi-Fi and scan again.")
}

async fn pair(addr: &str, secret: &str) -> Result<()> {
    // adb reads the code from stdin when it isn't an argument (adb client/commandline.cpp, "pair").
    let prog = tool("adb").ok_or_else(|| anyhow!("adb isn't installed"))?;
    let (_, out) = run_input(
        &prog,
        &["pair", addr],
        Some(secret),
        Duration::from_secs(30),
    )
    .await?;
    if out.contains("Successfully paired") {
        let marker = paired_marker();
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(marker, "");
        info!("paired with the phone for apps");
        return Ok(());
    }
    if out.to_lowercase().contains("wrong password") {
        bail!("The code didn't match. Check it and try again.");
    }
    warn!("adb pair: {}", out.trim());
    bail!("Pairing didn't work. Open Wireless debugging on the phone and try again.")
}

/// After pairing, the phone advertises its connect port; adb often connects by itself.
async fn finish_connect(svc: &Service, ip: Option<String>, attempt: u64) {
    if !set_if(attempt, Phase::Connecting, None) {
        return;
    }
    for _ in 0..30 {
        if state().attempt != attempt {
            return;
        }
        if let Ok(Some(_)) = online_device(svc).await {
            set_if(attempt, Phase::Idle, None);
            return;
        }
        if let Ok((_, out)) = adb(&["mdns", "services"], Duration::from_secs(10)).await {
            for m in parse_mdns(&out) {
                let ip_ok = ip.as_ref().is_none_or(|ip| *ip == m.addr.ip().to_string());
                if m.kind == MdnsKind::Connect && ip_ok {
                    let _ = adb(&["connect", &m.addr.to_string()], Duration::from_secs(10)).await;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    set_if(
        attempt,
        Phase::Idle,
        Some(
            "Paired, but the phone can't be reached. Check that Wireless debugging is still on."
                .into(),
        ),
    );
}

pub async fn pair_code(svc: &Service, address: &str, code: &str) -> Result<Value> {
    let addr: SocketAddr = address
        .trim()
        .parse()
        .map_err(|_| anyhow!("Type the address as shown on the phone, like 192.168.1.20:41234."))?;
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        bail!("The pairing code has 6 digits.");
    }
    let attempt = {
        let mut s = state();
        s.attempt += 1;
        s.qr = None;
        s.phase = Phase::Pairing;
        s.error = None;
        s.attempt
    };
    if let Err(e) = pair(&addr.to_string(), &code).await {
        set_if(attempt, Phase::Idle, Some(e.to_string()));
        return Err(e);
    }
    let svc = svc.clone();
    tokio::spawn(async move { finish_connect(&svc, Some(addr.ip().to_string()), attempt).await });
    Ok(json!(true))
}

pub fn cancel() -> Value {
    let mut s = state();
    s.attempt += 1;
    s.qr = None;
    if s.phase != Phase::Downloading {
        s.phase = Phase::Idle;
    }
    s.error = None;
    json!(true)
}

async fn serial(svc: &Service) -> Result<String> {
    online_device(svc)
        .await?
        .map(|d| d.serial)
        .ok_or_else(|| anyhow!("Your phone isn't connected for apps. Turn on Wireless debugging."))
}

pub async fn list(svc: &Service) -> Result<Value> {
    let serial = serial(svc).await?;
    if let Some((s, apps)) = &state().apps {
        if *s == serial {
            return Ok(serde_json::to_value(apps)?);
        }
    }
    let prog = tool("scrcpy").ok_or_else(|| anyhow!("scrcpy isn't installed"))?;
    let (ok, out) = run(
        &prog,
        &["-s", &serial, "--list-apps"],
        Duration::from_secs(45),
    )
    .await?;
    let mut apps = parse_apps(&out);
    apps.sort_by_key(|a| a.name.to_lowercase());
    if !ok && apps.is_empty() {
        bail!("{}", scrcpy_error(&out));
    }
    state().apps = Some((serial, apps.clone()));
    Ok(serde_json::to_value(apps)?)
}

/// The last "ERROR:" line scrcpy printed, without its prefix.
fn scrcpy_error(out: &str) -> String {
    out.lines()
        .rev()
        .find_map(|l| l.split_once("ERROR: ").map(|(_, e)| e.trim().to_string()))
        .unwrap_or_else(|| "The phone didn't answer. Try again.".into())
}

/// Opens one phone app in its own window, or the whole phone screen when `package` is None.
/// Waits briefly so an immediate failure (old Android, app gone) is reported instead of lost.
pub async fn open(svc: &Service, package: Option<&str>, title: &str) -> Result<Value> {
    let serial = serial(svc).await?;
    let prog = tool("scrcpy").ok_or_else(|| anyhow!("scrcpy isn't installed"))?;
    let mut args = vec!["-s".to_string(), serial, format!("--window-title={title}")];
    if let Some(pkg) = package {
        if !pkg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        {
            bail!("not an app package name");
        }
        args.push("--new-display".into());
        args.push(format!("--start-app={pkg}"));
    } else {
        args.push("--stay-awake".into());
    }
    let log = std::env::temp_dir().join(format!("tether-scrcpy-{}.log", random_hex(6)));
    let mut child = Command::new(&prog)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log)?)
        .spawn()
        .context("starting scrcpy")?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            let out = std::fs::read_to_string(&log).unwrap_or_default();
            let _ = std::fs::remove_file(&log);
            if status.success() {
                return Ok(json!(true));
            }
            warn!("scrcpy exited: {}", out.lines().last().unwrap_or(""));
            bail!("{}", friendly_open_error(&out));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Still running: the window is up. Reap it in the background when it closes.
    tokio::spawn(async move {
        if let Ok(status) = child.wait().await {
            if !status.success() {
                let out = std::fs::read_to_string(&log).unwrap_or_default();
                warn!("scrcpy stopped: {}", scrcpy_error(&out));
            }
        }
        let _ = std::fs::remove_file(&log);
    });
    Ok(json!(true))
}

fn friendly_open_error(out: &str) -> String {
    let e = scrcpy_error(out);
    let lower = e.to_lowercase();
    if lower.contains("virtual display") || lower.contains("new display") {
        "Opening apps in their own window needs Android 10 or newer.".into()
    } else if lower.contains("display") && lower.contains("could not") {
        "Tether couldn't open a window on this desktop.".into()
    } else {
        e
    }
}

/// Downloads scrcpy's static build (with adb) into Tether's data folder, checking its checksum.
pub fn download(_svc: &Service) -> Result<Value> {
    if !cfg!(target_arch = "x86_64") {
        bail!("There's no ready-made helper for this processor. Install scrcpy 3.0 or newer.");
    }
    {
        let mut s = state();
        if s.phase == Phase::Downloading {
            return Ok(json!(true));
        }
        s.phase = Phase::Downloading;
        s.error = None;
    }
    tokio::spawn(async move {
        match fetch_helper().await {
            Ok(()) => set(Phase::Idle, None),
            Err(e) => {
                warn!("helper download: {e:#}");
                set(Phase::Idle, Some(format!("The download didn't work: {e}")));
            }
        }
    });
    Ok(json!(true))
}

async fn fetch_helper() -> Result<()> {
    let dest = helper_dir();
    let parent = dest.parent().ok_or_else(|| anyhow!("no data folder"))?;
    std::fs::create_dir_all(parent)?;
    let archive = parent.join(format!("scrcpy-{HELPER_VERSION}.tar.gz.part"));
    let fetched = match which("curl") {
        Some(curl) => {
            let a = archive.to_string_lossy();
            run(
                &curl,
                &["-fsSL", "-o", &a, HELPER_URL],
                Duration::from_secs(600),
            )
            .await
        }
        None => match which("wget") {
            Some(wget) => {
                let a = archive.to_string_lossy();
                run(
                    &wget,
                    &["-q", "-O", &a, HELPER_URL],
                    Duration::from_secs(600),
                )
                .await
            }
            None => bail!("this computer has neither curl nor wget"),
        },
    }?;
    if !fetched.0 {
        let _ = std::fs::remove_file(&archive);
        bail!("check the internet connection");
    }
    let bytes = std::fs::read(&archive)?;
    if hex::encode(Sha256::digest(&bytes)) != HELPER_SHA256 {
        let _ = std::fs::remove_file(&archive);
        bail!("the file was damaged on the way");
    }
    let tar = which("tar").ok_or_else(|| anyhow!("tar isn't installed"))?;
    let (ok, out) = run(
        &tar,
        &[
            "-xzf",
            &archive.to_string_lossy(),
            "-C",
            &parent.to_string_lossy(),
        ],
        Duration::from_secs(120),
    )
    .await?;
    let _ = std::fs::remove_file(&archive);
    if !ok || !is_executable(&dest.join("scrcpy")) {
        bail!("unpacking failed: {}", out.trim());
    }
    info!("helper installed in {}", dest.display());
    Ok(())
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

/// The QR code as an SVG path, dark modules on a light square with the standard quiet zone.
fn qr_svg(data: &str) -> Result<String> {
    let code = qrcode::QrCode::new(data.as_bytes()).map_err(|e| anyhow!("QR code: {e}"))?;
    let w = code.width();
    let colors = code.to_colors();
    let quiet = 4;
    let size = w + 2 * quiet;
    let mut path = String::new();
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                path.push_str(&format!("M{} {}h1v1h-1z", x + quiet, y + quiet));
            }
        }
    }
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" shape-rendering="crispEdges"><rect width="{size}" height="{size}" fill="#fff"/><path d="{path}" fill="#000"/></svg>"##
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices() {
        let out = "List of devices attached\n\
            192.168.1.5:37199      device product:dm1qxxx model:SM_S911B device:dm1q transport_id:3\n\
            adb-R5CW-AbCd._adb-tls-connect._tcp offline transport_id:4\n\n";
        let d = parse_devices(out);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].serial, "192.168.1.5:37199");
        assert!(d[0].online);
        assert_eq!(d[0].model.as_deref(), Some("SM S911B"));
        assert!(!d[1].online);
    }

    #[test]
    fn mdns() {
        let out = "List of discovered mdns services\n\
            adb-R5CW1234-AbCdEf\t_adb-tls-connect._tcp\t192.168.1.5:37199\n\
            tether-a1b2c3\t_adb-tls-pairing._tcp.\t192.168.1.5:41234\n\
            junk line\n";
        let m = parse_mdns(out);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].kind, MdnsKind::Connect);
        assert_eq!(m[1].instance, "tether-a1b2c3");
        assert_eq!(m[1].kind, MdnsKind::Pairing);
        assert_eq!(m[1].addr.port(), 41234);
    }

    #[test]
    fn apps() {
        let out = "scrcpy 5.0.1 <https://github.com/Genymobile/scrcpy>\n\
            INFO: ADB device found:\n\
            [server] INFO: List of apps:\n \
            * Calculator                     com.sec.android.app.popupcalculator\n \
            - Spotify                        com.spotify.music\n \
            - A name that is longer than thirty chars com.example.long\n";
        let a = parse_apps(out);
        assert_eq!(a.len(), 3);
        assert_eq!(
            a[0],
            PhoneApp {
                name: "Calculator".into(),
                package: "com.sec.android.app.popupcalculator".into(),
                system: true
            }
        );
        assert_eq!(a[1].name, "Spotify");
        assert!(!a[1].system);
        assert_eq!(a[2].name, "A name that is longer than thirty chars");
    }

    #[test]
    fn versions_and_errors() {
        assert_eq!(
            scrcpy_major("scrcpy 5.0.1 <https://github.com/Genymobile/scrcpy>"),
            Some(5)
        );
        assert_eq!(scrcpy_major("scrcpy 1.25"), Some(1));
        assert_eq!(
            scrcpy_error("INFO: x\nERROR: Server connection failed\n"),
            "Server connection failed"
        );
    }

    #[test]
    fn qr() {
        let svg = qr_svg("WIFI:T:ADB;S:tether-abc123;P:0123456789ab;;").unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("h1v1h-1z"));
    }
}
