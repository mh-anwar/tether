//! Tray icon with the number of unread conversations, a menu, and the unread count on the
//! launcher badge.
//!
//! Two kinds of tray exist on Linux and a desktop usually has one of them:
//! - StatusNotifierItem over D-Bus (KDE, GNOME with the AppIndicator extension, waybar, Xfce),
//!   with its menu over com.canonical.dbusmenu.
//! - The older XEmbed system tray (i3bar, polybar, stalonetray): a small X11 window handed to the
//!   bar, with a menu window of our own on right-click. X11 sessions only.
//!
//! The service owns the icon, so it is there with the window closed. StatusNotifierItem is used
//! when a watcher is on the bus; otherwise the XEmbed tray if the screen has one. KDE turns
//! XEmbed icons into StatusNotifierItems itself, so showing both would give two icons.

mod menu;
mod sni;
mod xembed;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use image::{imageops::FilterType, Rgba, RgbaImage};
use serde_json::{json, Value};
use tether_core::{
    plugins::{self, sms, Update},
    Event, PairState, Service,
};
use tokio::sync::{broadcast::error::RecvError, mpsc, watch, Notify};
use tracing::{debug, info};

use menu::Action;

/// thread id -> date of the newest message the user has seen in the window.
static SEEN: Mutex<Option<HashMap<i64, i64>>> = Mutex::new(None);
/// Something the tray shows changed outside the phone's events (seen texts, pause, settings).
static CHANGED: Notify = Notify::const_new();
/// A window answered "show yourself"; otherwise a click starts one.
static WINDOW_SHOWN: Notify = Notify::const_new();
static SNI_ACTIVE: AtomicBool = AtomicBool::new(false);
static XEMBED_DOCKED: AtomicBool = AtomicBool::new(false);
/// The bus has been asked once whether the desktop has a tray, so "no tray" is an answer.
static TRAY_CHECKED: AtomicBool = AtomicBool::new(false);
static SEEN_FILE: OnceLock<PathBuf> = OnceLock::new();
/// What a window started from the tray should do once it is up ("send_files"), and when it was
/// asked for. A running window gets it as an event instead.
static PENDING: Mutex<Option<(Instant, Value)>> = Mutex::new(None);

/// The window reports what has been read there, so the count matches its unread dots.
pub fn set_seen(seen: HashMap<i64, i64>) {
    let mut guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    for (thread, date) in seen {
        let slot = map.entry(thread).or_insert(0);
        *slot = (*slot).max(date);
    }
    if let Some(path) = SEEN_FILE.get() {
        if let Ok(bytes) = serde_json::to_vec(map) {
            let _ = std::fs::write(path, bytes);
        }
    }
    drop(guard);
    CHANGED.notify_one();
}

/// A setting the menu shows changed (notification pop-ups on or off).
pub fn refresh() {
    CHANGED.notify_one();
}

pub fn window_shown() {
    WINDOW_SHOWN.notify_one();
}

/// The action a window started from the tray should run, if it was asked for in the last 30 s.
pub fn take_pending() -> Value {
    let taken = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    match taken {
        Some((at, v)) if at.elapsed() < Duration::from_secs(30) => v,
        _ => Value::Null,
    }
}

/// "AppIndicator and KStatusNotifierItem Support", the extension that gives GNOME a tray.
pub const EXTENSION_URL: &str = "https://extensions.gnome.org/extension/615/";

/// GNOME without the AppIndicator extension has no tray at all; the window says so once.
pub fn missing() -> bool {
    TRAY_CHECKED.load(Ordering::Relaxed)
        && !SNI_ACTIVE.load(Ordering::Relaxed)
        && !XEMBED_DOCKED.load(Ordering::Relaxed)
        && desktop_is(&["gnome", "unity"])
}

/// XDG_CURRENT_DESKTOP is a colon-separated list such as "ubuntu:GNOME" or "KDE".
fn desktop_is(names: &[&str]) -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    desktop.split(':').any(|d| names.contains(&d))
}

/// A paired phone as the tray shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Phone {
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// Charge in percent and whether it is charging, once the phone has said.
    pub battery: Option<(i64, bool)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TrayState {
    pub unread: usize,
    pub phones: Vec<Phone>,
    /// Phone notifications are shown on the desktop (Settings).
    pub mirroring: bool,
    /// ... but paused from the tray for now.
    pub paused: bool,
}

impl TrayState {
    pub fn connected(&self) -> impl Iterator<Item = &Phone> {
        self.phones.iter().filter(|p| p.connected)
    }
}

fn snapshot(service: &Service) -> TrayState {
    let phones: Vec<Phone> = service
        .devices()
        .into_iter()
        .filter(|d| d.pair == PairState::Paired)
        .map(|d| {
            let battery = service
                .battery(&d.id)
                .filter(|b| (0..=100).contains(&b.charge))
                .map(|b| (b.charge, b.charging));
            Phone {
                id: d.id,
                name: d.name,
                connected: d.connected,
                battery,
            }
        })
        .collect();
    TrayState {
        unread: unread(service, &phones),
        phones,
        mirroring: crate::notify::ENABLED.load(Ordering::Relaxed),
        paused: crate::notify::PAUSED.load(Ordering::Relaxed),
    }
}

fn unread(service: &Service, phones: &[Phone]) -> usize {
    let guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.as_ref();
    phones
        .iter()
        .flat_map(|d| service.threads(&d.id).unwrap_or_default())
        .filter(|m| {
            m.kind == sms::TYPE_INBOX
                && m.read == 0
                && m.date > seen.and_then(|s| s.get(&m.thread_id)).copied().unwrap_or(0)
        })
        .count()
}

pub fn start(service: Service) {
    let path = service.data_dir().join("seen.json");
    let saved = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = saved;
    let _ = SEEN_FILE.set(path);
    let icons = service.data_dir().join("tray-icons");
    if let Err(e) = write_theme_icons(&icons) {
        debug!("tray: could not write icons for the tray ({e})");
    }

    let (state_tx, state_rx) = watch::channel(snapshot(&service));
    let (action_tx, mut action_rx) = mpsc::unbounded_channel::<Action>();

    let svc = service.clone();
    tokio::spawn(async move {
        let mut events = svc.subscribe();
        loop {
            tokio::select! {
                e = events.recv() => match e {
                    Ok(Event::Update {
                        update: Update::Messages { .. } | Update::Battery(_),
                        ..
                    })
                    | Ok(Event::Device(_)) => {}
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                },
                _ = CHANGED.notified() => {}
            }
            let next = snapshot(&svc);
            state_tx.send_if_modified(|old| {
                let changed = *old != next;
                *old = next;
                changed
            });
        }
    });

    let svc = service.clone();
    tokio::spawn(async move {
        while let Some(action) = action_rx.recv().await {
            run(&svc, action).await;
        }
    });

    tokio::spawn(sni::serve(state_rx.clone(), action_tx.clone(), icons));
    if xembed::wanted(
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    ) {
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || xembed::run(state_rx, action_tx))
            .ok();
    }
}

async fn run(service: &Service, action: Action) {
    match action {
        Action::Open => {
            open_window().await;
        }
        Action::Ring(device) => {
            if let Err(e) = service.send(&device, plugins::ring_phone()) {
                info!("tray: could not ring the phone: {e}");
            }
        }
        Action::SendFiles => {
            let event = json!({"event": "send_files"});
            if open_window().await {
                crate::api::ui_event(event);
            } else {
                *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), event));
            }
        }
        Action::TogglePause => {
            let paused = !crate::notify::PAUSED.load(Ordering::Relaxed);
            crate::notify::PAUSED.store(paused, Ordering::Relaxed);
            info!(
                "tray: notification pop-ups {}",
                if paused { "paused" } else { "resumed" }
            );
            refresh();
        }
        Action::Quit => {
            info!("tray: quitting");
            // Open windows close too; otherwise they would start the service again.
            crate::api::ui_event(json!({"event": "quit"}));
            crate::api::exit_soon();
        }
    }
}

/// Brings the window forward, or starts it when none is open. True when a running window
/// answered (it can take events right away).
pub(crate) async fn open_window() -> bool {
    crate::api::ui_event(json!({"event": "show_window"}));
    if tokio::time::timeout(Duration::from_millis(700), WINDOW_SHOWN.notified())
        .await
        .is_ok()
    {
        return true;
    }
    // An AppImage opens its window when run without arguments; otherwise the window program
    // sits next to this one.
    let program = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| {
        let sibling = std::env::current_exe().ok()?.with_file_name("tether-app");
        sibling.is_file().then_some(sibling)
    });
    let program = program.unwrap_or_else(|| PathBuf::from("tether-app"));
    let started = std::process::Command::new(&program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = started {
        info!(
            "tray: could not open the window ({}): {e}",
            program.display()
        );
    }
    false
}

fn title(unread: usize) -> String {
    match unread {
        0 => "Tether".into(),
        1 => "Tether: 1 unread conversation".into(),
        n => format!("Tether: {n} unread conversations"),
    }
}

/// Tooltip lines: each paired phone with its battery, then unread texts and a pause.
pub(crate) fn tooltip(state: &TrayState) -> Vec<String> {
    let mut lines: Vec<String> = state
        .phones
        .iter()
        .map(|p| match (p.connected, p.battery) {
            (true, Some((charge, true))) => format!("{}: {charge}% battery, charging", p.name),
            (true, Some((charge, false))) => format!("{}: {charge}% battery", p.name),
            (true, None) => format!("{}: connected", p.name),
            (false, _) => format!("{}: not connected", p.name),
        })
        .collect();
    if lines.is_empty() {
        lines.push("No phone paired yet".into());
    }
    match state.unread {
        0 => {}
        1 => lines.push("1 unread conversation".into()),
        n => lines.push(format!("{n} unread conversations")),
    }
    if state.paused && state.mirroring {
        lines.push("Notification pop-ups paused".into());
    }
    lines
}

// ---------- icon ----------

/// 3x5 digits and a plus sign for the badge; a font would be a dependency for ten glyphs.
const GLYPHS: [[u8; 5]; 11] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
    [0b000, 0b010, 0b111, 0b010, 0b000],
];

fn blend(img: &mut RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f32) {
    if x >= img.width() || y >= img.height() || alpha <= 0.0 {
        return;
    }
    let a = alpha.min(1.0);
    let px = img.get_pixel_mut(x, y);
    let under = px.0[3] as f32 / 255.0;
    let out = a + under * (1.0 - a);
    for (i, c) in color.iter().enumerate() {
        px.0[i] = ((*c as f32 * a + px.0[i] as f32 * under * (1.0 - a)) / out.max(0.001)) as u8;
    }
    px.0[3] = (out * 255.0) as u8;
}

/// The app icon at `size` pixels, with a red badge holding the unread count.
pub fn icon(size: u32, unread: usize) -> RgbaImage {
    static BASE: OnceLock<Option<RgbaImage>> = OnceLock::new();
    let base = BASE.get_or_init(|| {
        image::load_from_memory(include_bytes!("../../../tether-app/icons/icon.png"))
            .ok()
            .map(|i| i.to_rgba8())
    });
    let size = size.clamp(8, 256);
    let mut img = match base {
        Some(b) => image::imageops::resize(b, size, size, FilterType::Triangle),
        None => RgbaImage::from_pixel(size, size, Rgba([36, 89, 214, 255])),
    };
    if unread == 0 {
        return img;
    }
    // Badge: a filled circle in the lower right corner, edge smoothed by coverage.
    let s = size as f32;
    let two = unread > 9;
    let r = s * if two { 0.34 } else { 0.30 };
    let (cx, cy) = (s - r, s - r);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            blend(&mut img, x, y, [214, 48, 43], r - d + 0.5);
        }
    }
    let glyphs: Vec<usize> = if two { vec![9, 10] } else { vec![unread] };
    // Whole-pixel cells keep the digits crisp at tray sizes.
    let cell = ((r * 1.2) / 5.0).floor().max(1.0) as u32;
    let width = glyphs.len() as u32 * 3 * cell + (glyphs.len() as u32 - 1) * cell;
    let x0 = (cx - width as f32 / 2.0).round().max(0.0) as u32;
    let y0 = (cy - (5 * cell) as f32 / 2.0).round().max(0.0) as u32;
    for (i, g) in glyphs.iter().enumerate() {
        for (row, bits) in GLYPHS[*g].iter().enumerate() {
            for col in 0..3u32 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for dy in 0..cell {
                    for dx in 0..cell {
                        let x = x0 + i as u32 * 4 * cell + col * cell + dx;
                        let y = y0 + row as u32 * cell + dy;
                        blend(&mut img, x, y, [255, 255, 255], 1.0);
                    }
                }
            }
        }
    }
    img
}

/// One-colour version of the app icon (phone and signal arc) for panels that draw tray icons in
/// their text colour: GNOME recolours any "-symbolic" icon, KDE recolours the ColorScheme-Text
/// class. Only filled shapes: GTK fills every shape of a symbolic icon, so strokes would turn
/// into solid blocks.
const SYMBOLIC_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
<style id="current-color-scheme">.ColorScheme-Text { color:#232629; }</style>
<g class="ColorScheme-Text" fill="currentColor">
<path fill-rule="evenodd" d="M4 1.5h3a1.5 1.5 0 0 1 1.5 1.5v10a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 13V3A1.5 1.5 0 0 1 4 1.5zM4 3v10h3V3z"/>
<circle cx="5.5" cy="11.6" r="0.75"/>
<path d="M11.76 4.48A4.6 4.6 0 0 1 11.76 11.52L10.79 10.37A3.1 3.1 0 0 0 10.79 5.63z"/>
</g>
</svg>
"##;

pub(crate) const SYMBOLIC_NAME: &str = "tether-tray-symbolic";
pub(crate) const COLOUR_NAME: &str = "tether-tray";

/// Icons for the tray's own icon theme folder (IconThemePath), in a hicolor tree for KDE and at
/// the top for GTK-based hosts that look there for unthemed icons.
fn write_theme_icons(dir: &Path) -> std::io::Result<()> {
    let put = |path: PathBuf, bytes: &[u8]| -> std::io::Result<()> {
        if std::fs::read(&path).ok().as_deref() == Some(bytes) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)
    };
    let svg = format!("{SYMBOLIC_NAME}.svg");
    put(
        dir.join("hicolor/scalable/apps").join(&svg),
        SYMBOLIC_SVG.as_bytes(),
    )?;
    put(dir.join(&svg), SYMBOLIC_SVG.as_bytes())?;
    for size in [32u32, 64, 128] {
        let mut png = Vec::new();
        icon(size, 0)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(std::io::Error::other)?;
        put(
            dir.join(format!("hicolor/{size}x{size}/apps/{COLOUR_NAME}.png")),
            &png,
        )?;
        if size == 128 {
            put(dir.join(format!("{COLOUR_NAME}.png")), &png)?;
        }
    }
    Ok(())
}

/// The app's own icon when it is installed in the icon theme (install.sh puts "tether" in
/// hicolor; Flatpak exports its app id).
fn installed_icon() -> Option<String> {
    if let Ok(id) = std::env::var("FLATPAK_ID") {
        return Some(id);
    }
    data_dirs().into_iter().find_map(|d| {
        let hicolor = d.join("icons/hicolor");
        std::fs::read_dir(hicolor).ok()?.flatten().find_map(|size| {
            ["png", "svg"]
                .iter()
                .any(|ext| size.path().join(format!("apps/tether.{ext}")).is_file())
                .then(|| "tether".to_string())
        })
    })
}

/// $XDG_DATA_HOME, then $XDG_DATA_DIRS, with the spec's defaults.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        system
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// Desktop entries that launch the window, for the launcher badge (`application://<id>`).
fn launcher_ids() -> Vec<String> {
    let mut ids = Vec::new();
    if let Ok(id) = std::env::var("FLATPAK_ID") {
        ids.push(format!("{id}.desktop"));
    }
    let appimage = std::env::var("APPIMAGE").unwrap_or_default();
    for dir in data_dirs() {
        let Ok(list) = std::fs::read_dir(dir.join("applications")) else {
            continue;
        };
        for entry in list.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".desktop") || ids.contains(&name) {
                continue;
            }
            // Only the window's own entry, not the app shortcuts (tether-app-<package>.desktop).
            if name.starts_with("tether-app-") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if launches_window(&text, &appimage) {
                ids.push(name);
            }
        }
    }
    if ids.is_empty() {
        ids.push("tether.desktop".into());
    }
    ids
}

fn launches_window(desktop_entry: &str, appimage: &str) -> bool {
    desktop_entry.lines().any(|l| {
        let l = l.trim();
        l == "StartupWMClass=tether-app"
            || (l.starts_with("Exec=")
                && (l.contains("tether-app") || (!appimage.is_empty() && l.contains(appimage))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_appears_only_with_unread() {
        let plain = icon(24, 0);
        let one = icon(24, 3);
        let many = icon(24, 12);
        assert_eq!(plain.dimensions(), (24, 24));
        // Lower right corner area turns red with a count.
        let red = |i: &RgbaImage| i.get_pixel(16, 20).0;
        assert_ne!(red(&plain), red(&one));
        assert_ne!(one, many);
        assert_eq!(title(0), "Tether");
        assert_eq!(title(1), "Tether: 1 unread conversation");
        assert_eq!(title(4), "Tether: 4 unread conversations");
    }

    fn phone(name: &str, connected: bool, battery: Option<(i64, bool)>) -> Phone {
        Phone {
            id: name.to_lowercase(),
            name: name.into(),
            connected,
            battery,
        }
    }

    #[test]
    fn tooltip_names_phone_battery_and_unread() {
        let mut s = TrayState {
            unread: 2,
            phones: vec![phone("Galaxy", true, Some((82, true)))],
            mirroring: true,
            paused: false,
        };
        assert_eq!(
            tooltip(&s),
            ["Galaxy: 82% battery, charging", "2 unread conversations"]
        );
        s.phones = vec![phone("Galaxy", true, None), phone("Pixel", false, None)];
        s.unread = 1;
        s.paused = true;
        assert_eq!(
            tooltip(&s),
            [
                "Galaxy: connected",
                "Pixel: not connected",
                "1 unread conversation",
                "Notification pop-ups paused"
            ]
        );
        let empty = TrayState::default();
        assert_eq!(tooltip(&empty), ["No phone paired yet"]);
    }

    #[test]
    fn theme_icons_are_written_once() {
        let dir = std::env::temp_dir().join(format!("tether-tray-icons-{}", std::process::id()));
        write_theme_icons(&dir).expect("write icons");
        let svg = dir.join("hicolor/scalable/apps/tether-tray-symbolic.svg");
        let written = std::fs::metadata(&svg).and_then(|m| m.modified()).ok();
        assert!(dir.join("hicolor/64x64/apps/tether-tray.png").is_file());
        assert!(dir.join("tether-tray.png").is_file());
        write_theme_icons(&dir).expect("write icons again");
        assert_eq!(
            std::fs::metadata(&svg).and_then(|m| m.modified()).ok(),
            written
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_the_windows_desktop_entry() {
        assert!(launches_window(
            "[Desktop Entry]\nExec=/home/a/.local/bin/tether-app\n",
            ""
        ));
        assert!(launches_window(
            "[Desktop Entry]\nExec=/x/Tether.AppImage %U\n",
            "/x/Tether.AppImage"
        ));
        assert!(launches_window("StartupWMClass=tether-app\n", ""));
        assert!(!launches_window("Exec=tether apps com.whatsapp\n", ""));
    }
}
