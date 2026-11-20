//! StatusNotifierItem (the D-Bus tray of KDE, GNOME's AppIndicator extension, waybar, Xfce) and
//! the unread count on the launcher (com.canonical.Unity.LauncherEntry: Dash to Dock, Ubuntu's
//! dock, KDE's task manager). Wayland and X11 alike: nothing here touches the display server.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::sync::{mpsc, watch};
use tracing::{debug, info};
use zbus::{interface, object_server::SignalEmitter, ObjectPath, Variant};

use super::{
    desktop_is, icon, installed_icon, launcher_ids,
    menu::{entries, Action, DbusMenu, MENU_PATH},
    title, tooltip, TrayState, COLOUR_NAME, SNI_ACTIVE, SYMBOLIC_NAME, TRAY_CHECKED,
};

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// Panels draw the icon at 16 to 24 px (more on HiDPI); hosts pick the closest size.
const SIZES: [u32; 6] = [16, 22, 24, 32, 48, 64];

struct Item {
    state: watch::Receiver<TrayState>,
    actions: mpsc::UnboundedSender<Action>,
    icons: PathBuf,
    /// Themed icon to use with nothing unread ("" falls back to the pixmaps).
    name: String,
}

fn pixmaps(unread: usize) -> Pixmaps {
    SIZES
        .iter()
        .map(|size| {
            // ARGB32 in network byte order (the StatusNotifierItem spec).
            let bytes = icon(*size, unread)
                .pixels()
                .flat_map(|p| [p.0[3], p.0[0], p.0[1], p.0[2]])
                .collect();
            (*size as i32, *size as i32, bytes)
        })
        .collect()
}

/// GNOME and KDE draw tray icons in the panel's text colour, so a one-colour icon fits there in
/// light and dark themes; other bars (waybar, Xfce) show the coloured app icon.
fn themed_name() -> String {
    if desktop_is(&["gnome", "unity", "kde"]) {
        SYMBOLIC_NAME.into()
    } else {
        installed_icon().unwrap_or_else(|| COLOUR_NAME.into())
    }
}

/// Characters a host may read as markup in the tooltip body (KDE: HTML, waybar: Pango).
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn status(unread: usize) -> &'static str {
    if unread > 0 {
        "NeedsAttention"
    } else {
        "Active"
    }
}

#[interface(name = "org.kde.StatusNotifierItem")]
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
        title(self.state.borrow().unread)
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        status(self.state.borrow().unread)
    }
    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }
    /// The badge only exists as pixels, so with unread texts the name is left empty and hosts
    /// use the pixmaps.
    #[zbus(property)]
    fn icon_name(&self) -> String {
        if self.state.borrow().unread > 0 {
            String::new()
        } else {
            self.name.clone()
        }
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        pixmaps(self.state.borrow().unread)
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        self.icons.to_string_lossy().into_owned()
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        pixmaps(self.state.borrow().unread.max(1))
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let state = self.state.borrow();
        let mut lines = tooltip(&state);
        // People looking for a way to stop Tether hover the icon first.
        lines.push("Right-click for more, including Quit Tether".into());
        let body = lines
            .iter()
            .map(|l| escape(l))
            .collect::<Vec<_>>()
            .join("\n");
        (String::new(), vec![], "Tether".into(), body)
    }
    /// Left click opens the window; the menu is on right click.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'_> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.actions.send(Action::Open);
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.actions.send(Action::Open);
    }
    /// Only hosts that can't draw a dbusmenu call this; the window has everything the menu has.
    fn context_menu(&self, _x: i32, _y: i32) {
        let _ = self.actions.send(Action::Open);
    }
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_attention_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const LAUNCHER_PATH: &str = "/com/canonical/unity/launcherentry/tether";
const LAUNCHER_IFACE: &str = "com.canonical.Unity.LauncherEntry";

/// Sends the count to docks and task managers. They only listen for the signal, so it is sent
/// again now and then while there is a count, for a dock that started after us.
async fn launcher_badge(conn: &zbus::Connection, ids: &[String], unread: usize) {
    for id in ids {
        let props: HashMap<&str, Variant<'_>> = HashMap::from([
            ("count", Variant::from(unread as i64)),
            ("count-visible", Variant::from(unread > 0)),
        ]);
        let r = conn
            .emit_signal(
                None::<&str>,
                LAUNCHER_PATH,
                LAUNCHER_IFACE,
                "Update",
                &(format!("application://{id}"), props),
            )
            .await;
        if let Err(e) = r {
            debug!("tray: launcher badge not sent: {e}");
        }
    }
}

/// A Flatpak may only own names under its own app id without a wildcard permission, and watchers
/// take any name in `RegisterStatusNotifierItem` (KDE's statusnotifierwatcher.cpp, the GNOME
/// AppIndicator extension, waybar), so inside the sandbox the item lives under the app id.
fn item_name(flatpak_id: Option<&str>, pid: u32) -> String {
    match flatpak_id.filter(|id| !id.is_empty()) {
        Some(id) => format!("{id}.StatusNotifierItem-{pid}-1"),
        None => format!("org.kde.StatusNotifierItem-{pid}-1"),
    }
}

pub(super) async fn serve(
    mut state: watch::Receiver<TrayState>,
    actions: mpsc::UnboundedSender<Action>,
    icons: PathBuf,
) {
    let name = item_name(
        std::env::var("FLATPAK_ID").ok().as_deref(),
        std::process::id(),
    );
    let revision = Arc::new(AtomicU32::new(1));
    let conn = zbus::conn::Builder::session()
        .name(name.as_str())
        .serve_at(
            ITEM_PATH,
            Item {
                state: state.clone(),
                actions: actions.clone(),
                icons,
                name: themed_name(),
            },
        )
        .serve_at(
            MENU_PATH,
            DbusMenu {
                state: state.clone(),
                actions,
                revision: revision.clone(),
            },
        )
        .build()
        .await;
    let conn = match conn {
        Ok(c) => c,
        Err(e) => {
            debug!("tray: no session bus ({e})");
            TRAY_CHECKED.store(true, Ordering::Relaxed);
            return;
        }
    };
    let Ok(bus) = zbus::fdo::DBusProxy::new(&conn).await else {
        return;
    };
    let Ok(watcher) = zbus::names::BusName::try_from(WATCHER) else {
        return;
    };
    let launchers = launcher_ids();
    let mut shown = state.borrow().clone();
    if shown.unread > 0 {
        launcher_badge(&conn, &launchers, shown.unread).await;
    }
    // The watcher (the desktop's tray) can start after us or restart; register with each new one.
    let mut registered_with: Option<String> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                ticks += 1;
                if shown.unread > 0 && ticks.is_multiple_of(6) {
                    launcher_badge(&conn, &launchers, shown.unread).await;
                }
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
                TRAY_CHECKED.store(true, Ordering::Relaxed);
            }
            changed = state.changed() => {
                if changed.is_err() {
                    return;
                }
                let next = state.borrow_and_update().clone();
                if let Ok(iface) = conn.object_server().interface::<_, Item>(ITEM_PATH).await {
                    let em = iface.signal_emitter();
                    if next.unread != shown.unread {
                        let _ = Item::new_icon(em).await;
                        let _ = Item::new_attention_icon(em).await;
                        let _ = Item::new_title(em).await;
                        if status(next.unread) != status(shown.unread) {
                            let _ = Item::new_status(em, status(next.unread)).await;
                        }
                    }
                    let _ = Item::new_tool_tip(em).await;
                }
                if entries(&next) != entries(&shown) {
                    let rev = revision.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Ok(iface) = conn.object_server().interface::<_, DbusMenu>(MENU_PATH).await {
                        let _ = DbusMenu::layout_updated(iface.signal_emitter(), rev, 0).await;
                    }
                }
                if next.unread != shown.unread {
                    launcher_badge(&conn, &launchers, next.unread).await;
                }
                shown = next;
            }
        }
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
        assert!(zbus::names::WellKnownName::try_from(item_name(
            Some("io.github.mh_anwar.Tether"),
            2
        ))
        .is_ok());
    }

    #[test]
    fn needs_attention_with_unread() {
        assert_eq!(status(0), "Active");
        assert_eq!(status(3), "NeedsAttention");
        assert_eq!(escape("A&B <x>"), "A&amp;B &lt;x&gt;");
        let p = pixmaps(2);
        assert_eq!(p.len(), SIZES.len());
        assert!(p
            .iter()
            .all(|(w, h, d)| *w == *h && d.len() == (*w * *h * 4) as usize));
    }
}
