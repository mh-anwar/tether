//! Phone apps on the desktop: each phone app opens in its own window. KDE Connect can't carry screen
//! contents, so this uses Android's wireless debugging (adb) and scrcpy, which starts the app on a
//! virtual display and streams it (`--new-display --start-app`, scrcpy 3.0+). See docs/decisions.md.
//!
//! Setup is one QR scan: we show `WIFI:T:ADB;S:<name>;P:<password>;;` (the format Android Studio
//! uses), the phone then advertises `_adb-tls-pairing._tcp` under that name, and we `adb pair` with
//! the password. After that the phone advertises `_adb-tls-connect._tcp` and we `adb connect`.

use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};

use crate::exec::{download as fetch_file, is_executable, run, run_input, which};
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
    /// Code pairing: the phone's pairing address once we've spotted it, so only the code is typed.
    code_addr: Option<SocketAddr>,
    /// What the last look for the phone found, logged when it changes so a failure can be diagnosed.
    last_seen: Vec<Mdns>,
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
    code_addr: None,
    last_seen: Vec::new(),
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

/// Starts a new pairing attempt, which stops any older one, and returns its number. `phase: None` goes
/// back to idle, except that a helper download carries on.
fn new_attempt(phase: Option<Phase>, qr: Option<(String, String)>) -> u64 {
    let mut s = state();
    s.attempt += 1;
    s.qr = qr;
    s.code_addr = None;
    s.error = None;
    match phase {
        Some(p) => s.phase = p,
        None if s.phase != Phase::Downloading => s.phase = Phase::Idle,
        None => {}
    }
    s.attempt
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MdnsKind {
    Pairing,
    Connect,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Mdns {
    pub instance: String,
    pub kind: MdnsKind,
    pub addr: SocketAddr,
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

/// `scrcpy --version`, remembered per binary (path and modification time): the Apps page asks
/// for status every few seconds and the answer only changes when scrcpy is replaced.
async fn scrcpy_version(prog: &Path) -> Option<u32> {
    static CACHE: Mutex<Option<(PathBuf, std::time::SystemTime, Option<u32>)>> = Mutex::new(None);
    let mtime = std::fs::metadata(prog).and_then(|m| m.modified()).ok()?;
    if let Some((p, t, v)) = &*CACHE.lock().unwrap_or_else(|e| e.into_inner()) {
        if p == prog && *t == mtime {
            return *v;
        }
    }
    let v = run(prog, &["--version"], Duration::from_secs(5))
        .await
        .ok()
        .and_then(|(_, out)| scrcpy_major(&out));
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((prog.to_path_buf(), mtime, v));
    v
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

fn phone_addrs(svc: &Service) -> Vec<IpAddr> {
    phone_ips(svc)
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Wireless-debugging services in sight: what adb heard over multicast DNS, plus what the phones at
/// `ips` say when asked directly (works when a firewall drops multicast; see mdns_probe).
async fn services(ips: &[IpAddr]) -> Vec<Mdns> {
    let from_adb = async {
        match adb(&["mdns", "services"], Duration::from_secs(10)).await {
            Ok((_, out)) => parse_mdns(&out),
            Err(_) => Vec::new(),
        }
    };
    let (mut seen, direct) = tokio::join!(
        from_adb,
        crate::mdns_probe::probe(ips, Duration::from_millis(800))
    );
    for m in direct {
        if !seen.iter().any(|s| s.kind == m.kind && s.addr == m.addr) {
            seen.push(m);
        }
    }
    let mut s = state();
    if s.last_seen != seen {
        let list: Vec<String> = seen
            .iter()
            .map(|m| format!("{} {:?} {}", m.instance, m.kind, m.addr))
            .collect();
        info!("phone apps: services in sight: [{}]", list.join(", "));
        s.last_seen = seen.clone();
    }
    seen
}

async fn online_device(svc: &Service) -> Result<Option<AdbDevice>> {
    let (_, out) = adb(&["devices", "-l"], Duration::from_secs(15)).await?;
    let devs: Vec<AdbDevice> = parse_devices(&out)
        .into_iter()
        .filter(|d| d.online)
        .collect();
    pick_device(devs, &phone_ips(svc))
}

/// The phone to use: the one at the KDE Connect phone's address, else the only one. With several
/// and no match, refuse rather than open apps on an emulator or someone else's phone.
fn pick_device(devs: Vec<AdbDevice>, ips: &[String]) -> Result<Option<AdbDevice>> {
    let at_ip = |d: &AdbDevice| ips.iter().any(|ip| d.serial.starts_with(&format!("{ip}:")));
    if devs.len() > 1 && !devs.iter().any(at_ip) {
        bail!(
            "Several Android devices are connected to this computer. Unplug or turn off wireless \
             debugging on the ones you don't want."
        );
    }
    let mut devs = devs;
    devs.sort_by_key(|d| !at_ip(d));
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
        Some(p) => scrcpy_version(p).await,
        None => None,
    };
    let mut device_error = None;
    let device = if adb_path.is_some() {
        match online_device(svc).await {
            Ok(d) => d,
            Err(e) => {
                warn!("adb: {e:#}");
                device_error = Some(e.to_string());
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
        "error": s.error.clone().or(device_error),
        "qr": qr,
        "pairedBefore": paired_marker().exists(),
        "phoneIp": phone_ips(svc).into_iter().find(|ip| ip.parse::<IpAddr>().is_ok()),
        "pairingAddr": s.code_addr.map(|a| a.to_string()),
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
    let ips = phone_addrs(svc);
    tokio::spawn(async move {
        for m in services(&ips).await {
            if m.kind == MdnsKind::Connect && ips.contains(&m.addr.ip()) {
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
    let attempt = new_attempt(Some(Phase::Waiting), Some((name.clone(), password.clone())));
    let svc = svc.clone();
    tokio::spawn(async move {
        let result = wait_for_scan(&svc, &name, &password, attempt).await;
        match result {
            Ok(ip) => finish_connect(&svc, Some(ip), attempt).await,
            Err(e) => {
                set_if(attempt, Phase::Idle, Some(e.to_string()));
            }
        }
    });
    Ok(json!(true))
}

async fn wait_for_scan(svc: &Service, name: &str, password: &str, attempt: u64) -> Result<String> {
    let start = Instant::now();
    let mut tried = HashSet::new();
    let mut round = 0u32;
    while start.elapsed() < PAIR_TIMEOUT {
        if state().attempt != attempt {
            bail!("cancelled");
        }
        // Asked every round: KDE Connect may reconnect, or the phone change address, meanwhile.
        let ips = phone_addrs(svc);
        if let Some(m) = services(&ips)
            .await
            .into_iter()
            .find(|m| m.kind == MdnsKind::Pairing && m.instance == name)
        {
            if !set_if(attempt, Phase::Pairing, None) {
                bail!("cancelled");
            }
            match try_pair(&m.addr, password).await? {
                PairResult::Paired => return Ok(m.addr.ip().to_string()),
                PairResult::WrongCode => {
                    bail!("Pairing didn't work. Show a new code and scan it again.")
                }
                PairResult::Failed => bail!(PAIR_FAILED),
            }
        }
        // Nothing heard yet: the network may hide the phone's announcements altogether. After a few
        // seconds, and then now and then, try our password on the phone's open ports instead.
        if round % 8 == 4 {
            if let Some(addr) = pair_by_sweep(&ips, password, &mut tried, attempt).await? {
                return Ok(addr.ip().to_string());
            }
        }
        round += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!(
        "Your phone didn't answer. Check that it's on the same Wi-Fi as this computer, then show a \
         new code and scan it again."
    )
}

const PAIR_FAILED: &str =
    "Pairing didn't work. Open Wireless debugging on the phone and try again.";

enum PairResult {
    Paired,
    /// The port was the pairing port, but the code or password was wrong.
    WrongCode,
    /// Anything else, including a port that isn't the pairing port at all.
    Failed,
}

async fn try_pair(addr: &SocketAddr, secret: &str) -> Result<PairResult> {
    // adb reads the code from stdin when it isn't an argument (adb client/commandline.cpp, "pair").
    let prog = tool("adb").ok_or_else(|| anyhow!("adb isn't installed"))?;
    let out = match run_input(
        &prog,
        &["pair", &addr.to_string()],
        Some(secret),
        Duration::from_secs(20),
    )
    .await
    {
        Ok((_, out)) => out,
        Err(e) => {
            warn!("adb pair {addr}: {e:#}");
            return Ok(PairResult::Failed);
        }
    };
    if out.contains("Successfully paired") {
        let marker = paired_marker();
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(marker, "");
        info!("paired with the phone for apps at {addr}");
        return Ok(PairResult::Paired);
    }
    // adb says "Wrong password or connection was dropped" for both, so it only means a wrong code
    // when we know this is the pairing port.
    info!("adb pair {addr}: {}", out.trim());
    if out.to_lowercase().contains("wrong password") {
        return Ok(PairResult::WrongCode);
    }
    Ok(PairResult::Failed)
}

/// The last resort when the phone's announcements never arrive: try `secret` on each TCP port open on
/// the phone (see mdns_probe::open_ports). Ports that failed once are in `tried` and skipped later,
/// so the connect port isn't retried every round.
async fn pair_by_sweep(
    ips: &[IpAddr],
    secret: &str,
    tried: &mut HashSet<SocketAddr>,
    attempt: u64,
) -> Result<Option<SocketAddr>> {
    for ip in ips.iter().filter(|ip| ip.is_ipv4()) {
        let ports = crate::mdns_probe::open_ports(*ip).await;
        // KDE Connect's own ports are never adb's.
        let candidates: Vec<SocketAddr> = ports
            .into_iter()
            .filter(|p| !(1714..=1764).contains(p))
            .map(|p| SocketAddr::new(*ip, p))
            .filter(|a| !tried.contains(a))
            .collect();
        if !candidates.is_empty() {
            info!("phone apps: trying ports on the phone directly: {candidates:?}");
        }
        for addr in candidates {
            if state().attempt != attempt {
                bail!("cancelled");
            }
            match try_pair(&addr, secret).await? {
                PairResult::Paired => return Ok(Some(addr)),
                PairResult::WrongCode => {
                    bail!("The code didn't match. Check it and try again.")
                }
                PairResult::Failed => {
                    tried.insert(addr);
                }
            }
        }
    }
    Ok(None)
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
        let mut ips = phone_addrs(svc);
        if let Some(a) = ip.as_ref().and_then(|ip| ip.parse::<IpAddr>().ok()) {
            if !ips.contains(&a) {
                ips.push(a);
            }
        }
        for m in services(&ips).await {
            let ip_ok = ip.as_ref().is_none_or(|ip| *ip == m.addr.ip().to_string());
            if m.kind == MdnsKind::Connect && ip_ok {
                let _ = adb(&["connect", &m.addr.to_string()], Duration::from_secs(10)).await;
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

/// Pairs with the 6-digit code from "Pair device with pairing code". `address` is only for the rare
/// case it was typed by hand; normally Tether has spotted the pairing port, or finds it now.
pub async fn pair_code(svc: &Service, address: Option<&str>, code: &str) -> Result<Value> {
    let typed: Option<SocketAddr> = match address.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => Some(a.parse().map_err(|_| {
            anyhow!("Type the address as shown on the phone, like 192.168.1.20:41234.")
        })?),
        None => None,
    };
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        bail!("The pairing code has 6 digits.");
    }
    let spotted = state().code_addr;
    let attempt = new_attempt(Some(Phase::Pairing), None);
    let result = pair_with_code(svc, typed.or(spotted), &code, attempt).await;
    match result {
        Ok(addr) => {
            let svc = svc.clone();
            tokio::spawn(async move {
                finish_connect(&svc, Some(addr.ip().to_string()), attempt).await
            });
            Ok(json!(true))
        }
        Err(e) => {
            set_if(attempt, Phase::Idle, None);
            Err(e)
        }
    }
}

async fn pair_with_code(
    svc: &Service,
    known: Option<SocketAddr>,
    code: &str,
    attempt: u64,
) -> Result<SocketAddr> {
    let mut tried = HashSet::new();
    if let Some(addr) = known {
        match try_pair(&addr, code).await? {
            PairResult::Paired => return Ok(addr),
            PairResult::WrongCode => bail!("The code didn't match. Check it and try again."),
            PairResult::Failed => {
                tried.insert(addr);
            }
        }
    }
    let ips = phone_addrs(svc);
    if ips.is_empty() {
        bail!(
            "Tether can't see your phone right now. Check that it shows as connected in Tether and \
             is on the same Wi-Fi."
        );
    }
    match pair_by_sweep(&ips, code, &mut tried, attempt).await? {
        Some(addr) => Ok(addr),
        None => bail!(
            "Couldn't reach the pairing screen on your phone. Keep the screen with the code open, \
             check Wireless debugging is still on, and try again."
        ),
    }
}

pub fn cancel() -> Value {
    new_attempt(None, None);
    json!(true)
}

/// Code pairing: watch for the phone's pairing service so the window can fill in the address and the
/// owner types only the 6 digits. Runs until another attempt starts or the timeout.
pub fn find_pairing_port(svc: &Service) -> Value {
    let attempt = new_attempt(None, None);
    let svc = svc.clone();
    tokio::spawn(async move {
        let start = Instant::now();
        while start.elapsed() < PAIR_TIMEOUT && state().attempt == attempt {
            let ips = phone_addrs(&svc);
            let found = services(&ips)
                .await
                .into_iter()
                .find(|m| m.kind == MdnsKind::Pairing && ips.contains(&m.addr.ip()));
            {
                let mut s = state();
                if s.attempt != attempt {
                    return;
                }
                s.code_addr = found.map(|m| m.addr);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    json!(true)
}

/// A report for `tether apps doctor`: everything needed to see why the phone isn't found.
pub async fn doctor(svc: &Service) -> Result<Value> {
    let mut out = Vec::new();
    let ips = phone_addrs(svc);
    out.push(format!(
        "Phone address from KDE Connect: {}",
        if ips.is_empty() {
            "none (is the phone connected?)".to_string()
        } else {
            ips.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    for name in ["adb", "scrcpy"] {
        match tool(name) {
            Some(p) => {
                let (_, v) = run(&p, &["--version"], Duration::from_secs(10))
                    .await
                    .unwrap_or_default();
                let v = v.lines().next().unwrap_or("").trim().to_string();
                out.push(format!("{name}: {} ({v})", p.display()));
            }
            None => out.push(format!("{name}: not found")),
        }
    }
    if tool("adb").is_some() {
        for (title, args) in [
            ("adb mdns check", &["mdns", "check"][..]),
            ("adb devices", &["devices", "-l"][..]),
        ] {
            let text = match adb(args, Duration::from_secs(10)).await {
                Ok((_, t)) => t,
                Err(e) => e.to_string(),
            };
            out.push(format!("{title}:\n  {}", text.trim().replace('\n', "\n  ")));
        }
        let (_, heard) = adb(&["mdns", "services"], Duration::from_secs(10))
            .await
            .unwrap_or_default();
        out.push(format!(
            "adb mdns services:\n  {}",
            heard.trim().replace('\n', "\n  ")
        ));
    }
    let direct = crate::mdns_probe::probe(&ips, Duration::from_millis(1500)).await;
    out.push(format!(
        "Asked the phone directly: {}",
        if direct.is_empty() {
            "no answer (Wireless debugging off, or the phone ignores direct questions)".to_string()
        } else {
            direct
                .iter()
                .map(|m| format!("{:?} at {}", m.kind, m.addr))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    Ok(json!(out.join("\n")))
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
    if let Err(e) = fetch_file(HELPER_URL, &archive, Duration::from_secs(600)).await {
        let _ = std::fs::remove_file(&archive);
        return Err(e);
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

// ---- phone apps in the desktop's app menu ----

fn menu_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("applications"))
}

fn menu_file(package: &str) -> Option<PathBuf> {
    menu_dir().map(|d| d.join(format!("tether-app-{package}.desktop")))
}

fn valid_package(pkg: &str) -> bool {
    !pkg.is_empty()
        && pkg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

/// Packages name their icon differently (deb, AppImage, tarball), so entries carry their own.
fn menu_icon() -> Result<PathBuf> {
    let icon = dirs::data_dir()
        .map(|d| d.join("tether/menu-icon.png"))
        .ok_or_else(|| anyhow!("no data folder"))?;
    if !icon.exists() {
        if let Some(dir) = icon.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&icon, include_bytes!("../../tether-app/icons/icon.png"))?;
    }
    Ok(icon)
}

fn menu_entry(package: &str, name: &str, icon: &Path) -> Result<String> {
    let (prog, pre) = crate::autostart::cli_command()?;
    if prog.contains(['\n', '\r']) {
        bail!("Tether's path contains a line break; move it somewhere else first.");
    }
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let mut exec = vec![crate::autostart::quote_desktop(&prog)];
    exec.extend(pre.iter().map(|a| a.to_string()));
    exec.extend(["apps".to_string(), package.to_string()]);
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment={name} on your phone, opened by Tether\n\
         Exec={}\nIcon={}\nCategories=Network;\nStartupNotify=false\nX-Tether-Package={package}\n",
        exec.join(" "),
        icon.display()
    ))
}

/// Packages that have an entry in the app menu.
pub fn menu_entries() -> Value {
    let mut out = vec![];
    if let Some(Ok(rd)) = menu_dir().map(std::fs::read_dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if let Some(pkg) = f
                .strip_prefix("tether-app-")
                .and_then(|r| r.strip_suffix(".desktop"))
            {
                out.push(pkg.to_string());
            }
        }
    }
    json!(out)
}

/// The app's own icon for its menu entry: copied out of the notification-icon cache (which is
/// pruned) into a per-app file. Only files from that cache are accepted.
fn app_icon_copy(package: &str, from: &str) -> Option<PathBuf> {
    let data = dirs::data_dir()?.join("tether");
    let from = std::fs::canonicalize(from).ok()?;
    if !from.starts_with(std::fs::canonicalize(data.join("icons")).ok()?) {
        return None;
    }
    let to = data.join("menu-icons").join(format!("{package}.png"));
    std::fs::create_dir_all(to.parent()?).ok()?;
    std::fs::copy(&from, &to).ok()?;
    Some(to)
}

pub fn set_menu_entry(package: &str, name: &str, icon: Option<&str>, on: bool) -> Result<Value> {
    if !valid_package(package) {
        bail!("not an app package name");
    }
    let path = menu_file(package).ok_or_else(|| anyhow!("no data folder"))?;
    if on {
        std::fs::create_dir_all(path.parent().ok_or_else(|| anyhow!("no data folder"))?)?;
        let icon = match icon.and_then(|i| app_icon_copy(package, i)) {
            Some(i) => i,
            None => menu_icon()?,
        };
        std::fs::write(&path, menu_entry(package, name, &icon)?)?;
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(json!(true))
}

/// Rewrites existing entries so they point at this copy of Tether (an AppImage may have moved).
pub fn refresh_menu_entries() {
    let Some(Ok(rd)) = menu_dir().map(std::fs::read_dir) else {
        return;
    };
    for e in rd.flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        let Some(pkg) = f
            .strip_prefix("tether-app-")
            .and_then(|r| r.strip_suffix(".desktop"))
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        let name = text
            .lines()
            .find_map(|l| l.strip_prefix("Name="))
            .unwrap_or(pkg)
            .to_string();
        let icon = match text.lines().find_map(|l| l.strip_prefix("Icon=")) {
            Some(i) if Path::new(i).exists() => PathBuf::from(i),
            _ => match menu_icon() {
                Ok(i) => i,
                Err(_) => return,
            },
        };
        if valid_package(pkg) {
            if let Ok(entry) = menu_entry(pkg, &name, &icon) {
                if entry != text {
                    let _ = std::fs::write(e.path(), entry);
                }
            }
        }
    }
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
    fn picks_the_right_phone() {
        let dev = |serial: &str| AdbDevice {
            serial: serial.into(),
            online: true,
            model: None,
        };
        let ips = vec!["192.168.1.5".to_string()];
        let one = pick_device(vec![dev("adb-X._adb-tls-connect._tcp")], &ips).unwrap();
        assert!(one.is_some());
        let two = vec![dev("emulator-5554"), dev("192.168.1.5:37199")];
        assert_eq!(
            pick_device(two, &ips).unwrap().unwrap().serial,
            "192.168.1.5:37199"
        );
        assert!(pick_device(
            vec![dev("emulator-5554"), dev("adb-X._adb-tls-connect._tcp")],
            &ips
        )
        .is_err());
        assert!(pick_device(vec![], &ips).unwrap().is_none());
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
    fn menu_entry_text() {
        let e = menu_entry("com.spotify.music", "Spot\nify", Path::new("/x/icon.png")).unwrap();
        assert!(e.contains("\nName=Spotify\n"));
        assert!(e.contains(" apps com.spotify.music\n"));
        assert!(!valid_package("a b"));
        assert!(!valid_package(""));
    }

    #[test]
    fn qr() {
        let svg = qr_svg("WIFI:T:ADB;S:tether-abc123;P:0123456789ab;;").unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("h1v1h-1z"));
    }
}
