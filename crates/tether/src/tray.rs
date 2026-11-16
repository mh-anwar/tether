//! Tray icon with the number of unread conversations; a click opens the window.
//!
//! Two kinds of tray exist on Linux and a desktop usually has one of them:
//! - StatusNotifierItem over D-Bus (KDE, GNOME with the AppIndicator extension, waybar, Xfce).
//! - The older XEmbed system tray (i3bar, polybar, stalonetray): a small X11 window handed to the bar.
//!
//! The service owns the icon, so it is there with the window closed. StatusNotifierItem is used
//! when a watcher is on the bus; otherwise the XEmbed tray if the screen has one. KDE turns
//! XEmbed icons into StatusNotifierItems itself, so showing both would give two icons.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};

use image::{imageops::FilterType, Rgba, RgbaImage};
use serde_json::json;
use tether_core::{
    plugins::{sms, Update},
    Event, PairState, Service,
};
use tokio::sync::{broadcast::error::RecvError, mpsc, watch, Notify};
use tracing::{debug, info};

/// thread id -> date of the newest message the user has seen in the window.
static SEEN: Mutex<Option<HashMap<i64, i64>>> = Mutex::new(None);
static SEEN_CHANGED: Notify = Notify::const_new();
/// A window answered "show yourself"; otherwise a click starts one.
static WINDOW_SHOWN: Notify = Notify::const_new();
static SNI_ACTIVE: AtomicBool = AtomicBool::new(false);
static SEEN_FILE: OnceLock<PathBuf> = OnceLock::new();

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
    SEEN_CHANGED.notify_one();
}

pub fn window_shown() {
    WINDOW_SHOWN.notify_one();
}

fn unread(service: &Service) -> usize {
    let guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.as_ref();
    service
        .devices()
        .iter()
        .filter(|d| d.pair == PairState::Paired)
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

    let (count_tx, count_rx) = watch::channel(unread(&service));
    let (click_tx, mut click_rx) = mpsc::unbounded_channel::<()>();

    let svc = service.clone();
    tokio::spawn(async move {
        let mut events = svc.subscribe();
        loop {
            tokio::select! {
                e = events.recv() => match e {
                    Ok(Event::Update { update: Update::Messages { .. }, .. }) | Ok(Event::Device(_)) => {}
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                },
                _ = SEEN_CHANGED.notified() => {}
            }
            let n = unread(&svc);
            count_tx.send_if_modified(|old| std::mem::replace(old, n) != n);
        }
    });

    tokio::spawn(async move {
        while click_rx.recv().await.is_some() {
            open_window().await;
        }
    });

    tokio::spawn(sni(count_rx.clone(), click_tx.clone()));
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || xembed::run(count_rx, click_tx))
        .ok();
}

/// Brings the window forward, or starts it when none is open.
pub(crate) async fn open_window() {
    crate::api::ui_event(json!({"event": "show_window"}));
    if tokio::time::timeout(Duration::from_millis(700), WINDOW_SHOWN.notified())
        .await
        .is_ok()
    {
        return;
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
}

fn title(unread: usize) -> String {
    match unread {
        0 => "Tether".into(),
        1 => "Tether: 1 unread conversation".into(),
        n => format!("Tether: {n} unread conversations"),
    }
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
        image::load_from_memory(include_bytes!("../../tether-app/icons/icon.png"))
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

// ---------- StatusNotifierItem ----------

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

struct Item {
    unread: watch::Receiver<usize>,
    click: mpsc::UnboundedSender<()>,
}

impl Item {
    fn pixmaps(&self) -> Pixmaps {
        let n = *self.unread.borrow();
        [24u32, 48]
            .iter()
            .map(|size| {
                // ARGB32 in network byte order (the StatusNotifierItem spec).
                let bytes = icon(*size, n)
                    .pixels()
                    .flat_map(|p| [p.0[3], p.0[0], p.0[1], p.0[2]])
                    .collect();
                (*size as i32, *size as i32, bytes)
            })
            .collect()
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "Communications"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "tether"
    }
    #[zbus(property)]
    fn title(&self) -> String {
        title(*self.unread.borrow())
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        self.pixmaps()
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (
            String::new(),
            vec![],
            title(*self.unread.borrow()),
            String::new(),
        )
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.click.send(());
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.click.send(());
    }
    /// There is no menu; hosts that only offer this on a click still open the window.
    fn context_menu(&self, _x: i32, _y: i32) {
        let _ = self.click.send(());
    }
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    #[zbus(signal)]
    async fn new_icon(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_title(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
}

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";

/// A Flatpak may only own names under its own app id without a wildcard permission, and watchers
/// take any name in `RegisterStatusNotifierItem` (KDE's statusnotifierwatcher.cpp, the GNOME
/// AppIndicator extension, waybar), so inside the sandbox the item lives under the app id.
fn item_name(flatpak_id: Option<&str>, pid: u32) -> String {
    match flatpak_id.filter(|id| !id.is_empty()) {
        Some(id) => format!("{id}.StatusNotifierItem-{pid}-1"),
        None => format!("org.kde.StatusNotifierItem-{pid}-1"),
    }
}

async fn sni(mut unread: watch::Receiver<usize>, click: mpsc::UnboundedSender<()>) {
    let name = item_name(std::env::var("FLATPAK_ID").ok().as_deref(), std::process::id());
    let built = zbus::conn::Builder::session()
        .name(name.as_str())
        .serve_at(
            ITEM_PATH,
            Item {
                unread: unread.clone(),
                click,
            },
        )
        .build();
    let conn = match built.await {
        Ok(c) => c,
        Err(e) => {
            debug!("tray: no session bus ({e})");
            return;
        }
    };
    let Ok(bus) = zbus::fdo::DBusProxy::new(&conn).await else {
        return;
    };
    let Ok(watcher) = zbus::names::BusName::try_from(WATCHER) else {
        return;
    };
    // The watcher (the desktop's tray) can start after us or restart; register with each new one.
    let mut registered_with: Option<String> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let owner = bus.get_name_owner(watcher.clone()).await.ok().map(|o| o.to_string());
                if owner != registered_with {
                    if owner.is_some() {
                        let r = conn
                            .call_method(Some(WATCHER), "/StatusNotifierWatcher", Some(WATCHER),
                                "RegisterStatusNotifierItem", &(name.as_str(),))
                            .await;
                        if let Err(e) = r {
                            debug!("tray: registering with the desktop's tray failed: {e}");
                            continue;
                        }
                        info!("tray: icon shown in the desktop's tray");
                    }
                    SNI_ACTIVE.store(owner.is_some(), Ordering::Relaxed);
                    registered_with = owner;
                }
            }
            changed = unread.changed() => {
                if changed.is_err() {
                    return;
                }
                if let Ok(iface) = conn.object_server().interface::<_, Item>(ITEM_PATH).await {
                    let em = iface.signal_emitter();
                    let _ = Item::new_icon(em).await;
                    let _ = Item::new_tool_tip(em).await;
                    let _ = Item::new_title(em).await;
                }
            }
        }
    }
}

// ---------- XEmbed tray ----------

mod xembed {
    use std::time::{Duration, Instant};

    use tokio::sync::{mpsc, watch};
    use tracing::{debug, info};
    use x11rb::{
        connection::Connection,
        protocol::{xproto::*, Event},
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    use super::{icon, title, Ordering, SNI_ACTIVE};

    type Res<T> = Result<T, Box<dyn std::error::Error>>;

    struct Docked {
        win: Window,
        gc: Gcontext,
        depth: u8,
        size: u16,
    }

    pub fn run(unread: watch::Receiver<usize>, click: mpsc::UnboundedSender<()>) {
        // Wayland-only sessions have no X server; StatusNotifierItem covers those.
        let Ok((conn, screen)) = x11rb::connect(None) else {
            return;
        };
        if let Err(e) = serve(&conn, screen, unread, click) {
            debug!("tray: X11 tray stopped: {e}");
        }
    }

    fn atom(conn: &RustConnection, name: &str) -> Res<Atom> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    }

    fn serve(
        conn: &RustConnection,
        screen_num: usize,
        unread: watch::Receiver<usize>,
        click: mpsc::UnboundedSender<()>,
    ) -> Res<()> {
        let selection = atom(conn, &format!("_NET_SYSTEM_TRAY_S{screen_num}"))?;
        let mut docked: Option<Docked> = None;
        let mut shown = usize::MAX;
        let mut last_look: Option<Instant> = None;
        loop {
            // Dock when a tray is there and the D-Bus kind isn't in use; leave when it is.
            if last_look.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                last_look = Some(Instant::now());
                let sni = SNI_ACTIVE.load(Ordering::Relaxed);
                match (&docked, sni) {
                    (Some(d), true) => {
                        conn.destroy_window(d.win)?;
                        conn.flush()?;
                        docked = None;
                    }
                    (None, false) => {
                        let owner = conn.get_selection_owner(selection)?.reply()?.owner;
                        if owner != x11rb::NONE {
                            docked = Some(dock(conn, screen_num, owner)?);
                            shown = usize::MAX;
                            info!("tray: icon shown in the bar's tray");
                        }
                    }
                    _ => {}
                }
            }
            let mut redraw = false;
            while let Some(event) = conn.poll_for_event()? {
                let Some(d) = docked.as_mut() else { continue };
                match event {
                    Event::Expose(e) if e.window == d.win && e.count == 0 => redraw = true,
                    Event::ConfigureNotify(e) if e.window == d.win => {
                        let size = e.width.min(e.height);
                        if size != d.size && size > 0 {
                            d.size = size;
                            redraw = true;
                        }
                    }
                    Event::ButtonRelease(e) if e.event == d.win && e.detail == 1 => {
                        let _ = click.send(());
                    }
                    // The bar went away (restart, crash): the window is gone or back on the root.
                    Event::DestroyNotify(e) if e.window == d.win => docked = None,
                    Event::ReparentNotify(e)
                        if e.window == d.win && e.parent == conn.setup().roots[screen_num].root =>
                    {
                        conn.destroy_window(d.win)?;
                        docked = None;
                        last_look = None;
                    }
                    _ => {}
                }
            }
            let n = *unread.borrow();
            if let Some(d) = &docked {
                if redraw || n != shown {
                    if n != shown {
                        let name = title(n);
                        conn.change_property8(
                            PropMode::REPLACE,
                            d.win,
                            AtomEnum::WM_NAME,
                            AtomEnum::STRING,
                            name.as_bytes(),
                        )?;
                    }
                    // A bar that just went away makes drawing fail; the next look re-docks.
                    if draw(conn, d, n).is_ok() {
                        shown = n;
                    }
                    conn.flush()?;
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Hands a new icon window to the tray (freedesktop System Tray Protocol, "docking").
    fn dock(conn: &RustConnection, screen_num: usize, owner: Window) -> Res<Docked> {
        let screen = &conn.setup().roots[screen_num];
        let win = conn.generate_id()?;
        let events = EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::BUTTON_RELEASE;
        // A tray that draws with transparency names a 32-bit visual; icons must use it.
        let visual_atom = atom(conn, "_NET_SYSTEM_TRAY_VISUAL")?;
        let wanted = conn
            .get_property(false, owner, visual_atom, AtomEnum::VISUALID, 0, 1)?
            .reply()
            .ok()
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        let argb = wanted.filter(|v| {
            screen
                .allowed_depths
                .iter()
                .any(|d| d.depth == 32 && d.visuals.iter().any(|t| t.visual_id == *v))
        });
        let size = 24u16;
        let depth = if let Some(visual) = argb {
            let colormap = conn.generate_id()?;
            conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual)?;
            let aux = CreateWindowAux::new()
                .event_mask(events)
                .background_pixel(0)
                .border_pixel(0)
                .colormap(colormap);
            conn.create_window(
                32,
                win,
                screen.root,
                0,
                0,
                size,
                size,
                0,
                WindowClass::INPUT_OUTPUT,
                visual,
                &aux,
            )?;
            32
        } else {
            // The bar's own colour shows through around the icon.
            let aux = CreateWindowAux::new()
                .event_mask(events)
                .background_pixmap(u32::from(BackPixmap::PARENT_RELATIVE));
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                win,
                screen.root,
                0,
                0,
                size,
                size,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &aux,
            )?;
            screen.root_depth
        };
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"tether\0Tether\0",
        )?;
        // XEmbed: protocol version 0, and "map me" (XEMBED_MAPPED).
        let info = atom(conn, "_XEMBED_INFO")?;
        conn.change_property32(PropMode::REPLACE, win, info, info, &[0, 1])?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;
        let opcode = atom(conn, "_NET_SYSTEM_TRAY_OPCODE")?;
        // SYSTEM_TRAY_REQUEST_DOCK = 0.
        let request =
            ClientMessageEvent::new(32, owner, opcode, [x11rb::CURRENT_TIME, 0, win, 0, 0]);
        conn.send_event(false, owner, EventMask::NO_EVENT, request)?;
        conn.flush()?;
        Ok(Docked {
            win,
            gc,
            depth,
            size,
        })
    }

    fn draw(conn: &RustConnection, d: &Docked, unread: usize) -> Res<()> {
        let size = d.size;
        let img = icon(size as u32, unread);
        let mut data = Vec::with_capacity(img.len());
        if d.depth == 32 {
            // Premultiplied BGRA.
            for p in img.pixels() {
                let a = p.0[3] as u32;
                let pre = |c: u8| (c as u32 * a / 255) as u8;
                data.extend_from_slice(&[pre(p.0[2]), pre(p.0[1]), pre(p.0[0]), p.0[3]]);
            }
        } else {
            // No transparency: read the bar's colour from under the icon and mix it in.
            conn.clear_area(false, d.win, 0, 0, 0, 0)?;
            let under = conn
                .get_image(ImageFormat::Z_PIXMAP, d.win, 0, 0, size, size, !0)?
                .reply()
                .map(|r| r.data)
                .unwrap_or_default();
            for (i, p) in img.pixels().enumerate() {
                let bg = under.get(i * 4..i * 4 + 3).unwrap_or(&[0, 0, 0]);
                let a = p.0[3] as u32;
                let mix = |c: u8, b: u8| ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8;
                data.extend_from_slice(&[
                    mix(p.0[2], bg[0]),
                    mix(p.0[1], bg[1]),
                    mix(p.0[0], bg[2]),
                    0,
                ]);
            }
        }
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            d.win,
            d.gc,
            size,
            size,
            0,
            0,
            0,
            d.depth,
            &data,
        )?
        .check()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_name_stays_inside_the_flatpak_app_id() {
        assert_eq!(item_name(None, 42), "org.kde.StatusNotifierItem-42-1");
        assert_eq!(
            item_name(Some("io.github.mh_anwar.Tether"), 2),
            "io.github.mh_anwar.Tether.StatusNotifierItem-2-1"
        );
        assert!(zbus::names::WellKnownName::try_from(item_name(Some("io.github.mh_anwar.Tether"), 2)).is_ok());
    }

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
}
