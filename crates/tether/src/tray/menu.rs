//! The tray menu: one model, drawn by the desktop over com.canonical.dbusmenu (StatusNotifierItem)
//! or by our own popup window on XEmbed bars.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

use tokio::sync::{mpsc, watch};
use zbus::{fdo, interface, object_server::SignalEmitter, OwnedVariant, Variant};

use super::TrayState;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Open,
    /// Ring the phone with this device id.
    Ring(String),
    SendFiles,
    TogglePause,
    Quit,
}

/// One menu row. A row without a label is a separator.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    /// A checkmark row and whether it is ticked.
    pub check: Option<bool>,
    pub action: Option<Action>,
}

impl Entry {
    fn item(id: i32, label: impl Into<String>, enabled: bool, action: Action) -> Entry {
        Entry {
            id,
            label: label.into(),
            enabled,
            check: None,
            action: Some(action),
        }
    }

    fn separator(id: i32) -> Entry {
        Entry {
            id,
            label: String::new(),
            enabled: true,
            check: None,
            action: None,
        }
    }

    pub fn is_separator(&self) -> bool {
        self.label.is_empty()
    }
}

/// Ids stay the same for the same row, so a host that is showing the menu while it changes keeps
/// the right row under the pointer. Ring rows follow the phones (one per connected phone).
const OPEN: i32 = 1;
const SEND: i32 = 3;
const PAUSE: i32 = 5;
const QUIT: i32 = 7;
const RING: i32 = 100;

pub(crate) fn entries(state: &TrayState) -> Vec<Entry> {
    let mut out = vec![Entry::item(OPEN, "Open Tether", true, Action::Open)];
    out.push(Entry::separator(2));
    let connected: Vec<_> = state.connected().collect();
    match connected.as_slice() {
        [] => out.push(Entry::item(
            RING,
            "Ring my phone",
            false,
            Action::Ring(String::new()),
        )),
        [one] => out.push(Entry::item(
            RING,
            "Ring my phone",
            true,
            Action::Ring(one.id.clone()),
        )),
        many => {
            for (i, p) in many.iter().enumerate() {
                out.push(Entry::item(
                    RING + i as i32,
                    format!("Ring {}", p.name),
                    true,
                    Action::Ring(p.id.clone()),
                ));
            }
        }
    }
    out.push(Entry::item(
        SEND,
        "Send files\u{2026}",
        !connected.is_empty(),
        Action::SendFiles,
    ));
    out.push(Entry::separator(4));
    out.push(Entry {
        check: Some(state.paused && state.mirroring),
        ..Entry::item(
            PAUSE,
            "Pause notification pop-ups",
            state.mirroring,
            Action::TogglePause,
        )
    });
    out.push(Entry::separator(6));
    out.push(Entry::item(QUIT, "Quit Tether", true, Action::Quit));
    out
}

/// The action for a click on row `id`, if that row exists and can be used now.
pub(crate) fn action_for(state: &TrayState, id: i32) -> Option<Action> {
    entries(state)
        .into_iter()
        .find(|e| e.id == id && e.enabled)
        .and_then(|e| e.action)
}

// ---------- com.canonical.dbusmenu ----------

pub(super) const MENU_PATH: &str = "/MenuBar";

type Props = HashMap<String, OwnedVariant>;
type Layout = (i32, Props, Vec<OwnedVariant>);

fn put<'a>(props: &mut Props, key: &str, value: impl Into<Variant<'a>>) {
    if let Ok(v) = OwnedVariant::try_from(value.into()) {
        props.insert(key.into(), v);
    }
}

/// An entry's properties in dbusmenu terms, leaving out defaults (enabled, visible, standard).
pub(crate) fn properties(e: &Entry, wanted: &[String]) -> Props {
    let mut all = Props::new();
    if e.is_separator() {
        put(&mut all, "type", "separator");
    } else {
        // An underscore marks the access key; a phone named "my_phone" keeps its underscore.
        put(&mut all, "label", e.label.replace('_', "__"));
        if !e.enabled {
            put(&mut all, "enabled", false);
        }
        if let Some(on) = e.check {
            put(&mut all, "toggle-type", "checkmark");
            put(&mut all, "toggle-state", i32::from(on));
        }
    }
    if !wanted.is_empty() {
        all.retain(|k, _| wanted.contains(k));
    }
    all
}

fn root_properties() -> Props {
    let mut p = Props::new();
    put(&mut p, "children-display", "submenu");
    p
}

/// GetLayout's answer for `parent` (0 is the root), or None for an id that isn't in the menu.
pub(crate) fn layout(
    state: &TrayState,
    parent: i32,
    depth: i32,
    wanted: &[String],
) -> Option<Layout> {
    let items = entries(state);
    if parent != 0 {
        let e = items.iter().find(|e| e.id == parent)?;
        return Some((e.id, properties(e, wanted), vec![]));
    }
    let children = if depth == 0 {
        vec![]
    } else {
        items
            .iter()
            .filter_map(|e| {
                let child: Layout = (e.id, properties(e, wanted), vec![]);
                OwnedVariant::try_from(Variant::from(child)).ok()
            })
            .collect()
    };
    Some((0, root_properties(), children))
}

pub(super) struct DbusMenu {
    pub state: watch::Receiver<TrayState>,
    pub actions: mpsc::UnboundedSender<Action>,
    /// Raised each time the rows change; hosts compare it to know their copy is stale.
    pub revision: Arc<AtomicU32>,
}

impl DbusMenu {
    fn clicked(&self, id: i32, event: &str) -> bool {
        let state = self.state.borrow().clone();
        if !entries(&state).iter().any(|e| e.id == id) {
            return false;
        }
        if event == "clicked" {
            if let Some(action) = action_for(&state, id) {
                let _ = self.actions.send(action);
            }
        }
        true
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl DbusMenu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        vec![]
    }

    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> fdo::Result<(u32, Layout)> {
        let state = self.state.borrow().clone();
        let layout = layout(&state, parent_id, recursion_depth, &property_names)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no menu item {parent_id}")))?;
        Ok((self.revision.load(Ordering::Relaxed), layout))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Props)> {
        let state = self.state.borrow().clone();
        entries(&state)
            .iter()
            .filter(|e| ids.is_empty() || ids.contains(&e.id))
            .map(|e| (e.id, properties(e, &property_names)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedVariant> {
        let state = self.state.borrow().clone();
        entries(&state)
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| properties(e, &[]).remove(&name))
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no property {name} on item {id}")))
    }

    fn event(
        &self,
        id: i32,
        event_id: String,
        _data: OwnedVariant,
        _timestamp: u32,
    ) -> fdo::Result<()> {
        if self.clicked(id, &event_id) {
            Ok(())
        } else {
            Err(fdo::Error::InvalidArgs(format!("no menu item {id}")))
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedVariant, u32)>) -> Vec<i32> {
        events
            .iter()
            .filter(|(id, event, _, _)| !self.clicked(*id, event))
            .map(|(id, ..)| *id)
            .collect()
    }

    /// The rows are always current, so a host never needs to wait for an update.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (vec![], vec![])
    }

    #[zbus(signal)]
    pub async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::Phone;

    fn state(connected: &[&str], mirroring: bool, paused: bool) -> TrayState {
        TrayState {
            unread: 0,
            phones: connected
                .iter()
                .map(|n| Phone {
                    id: format!("id-{n}"),
                    name: (*n).into(),
                    connected: true,
                    battery: None,
                })
                .collect(),
            mirroring,
            paused,
        }
    }

    fn labels(s: &TrayState) -> Vec<String> {
        entries(s)
            .iter()
            .map(|e| match (e.is_separator(), e.enabled, e.check) {
                (true, ..) => "-".into(),
                (_, true, Some(true)) => format!("[x] {}", e.label),
                (_, true, _) => e.label.clone(),
                (_, false, _) => format!("({})", e.label),
            })
            .collect()
    }

    #[test]
    fn menu_with_one_phone() {
        let s = state(&["Galaxy"], true, false);
        assert_eq!(
            labels(&s),
            [
                "Open Tether",
                "-",
                "Ring my phone",
                "Send files\u{2026}",
                "-",
                "Pause notification pop-ups",
                "-",
                "Quit Tether"
            ]
        );
        assert_eq!(action_for(&s, RING), Some(Action::Ring("id-Galaxy".into())));
        assert_eq!(action_for(&s, QUIT), Some(Action::Quit));
        assert_eq!(action_for(&s, 2), None, "separators do nothing");
    }

    #[test]
    fn menu_without_a_phone_greys_out_phone_rows() {
        let s = state(&[], false, true);
        let l = labels(&s);
        assert_eq!(l[2], "(Ring my phone)");
        assert_eq!(l[3], "(Send files\u{2026})");
        // Pop-ups are off in Settings: nothing to pause, and no stale tick.
        assert_eq!(l[5], "(Pause notification pop-ups)");
        assert_eq!(action_for(&s, RING), None);
        assert_eq!(action_for(&s, SEND), None);
    }

    #[test]
    fn menu_with_two_phones_names_them_and_shows_pause() {
        let s = state(&["Galaxy", "Pixel"], true, true);
        let l = labels(&s);
        assert_eq!(l[2..4], ["Ring Galaxy", "Ring Pixel"]);
        assert_eq!(l[6], "[x] Pause notification pop-ups");
        assert_eq!(
            action_for(&s, RING + 1),
            Some(Action::Ring("id-Pixel".into()))
        );
        let ids: Vec<i32> = entries(&s).iter().map(|e| e.id).collect();
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(ids, unique);
    }

    #[test]
    fn dbusmenu_properties_and_layout() {
        let s = state(&["my_phone", "Pixel"], true, true);
        let rows = entries(&s);
        let ring = &rows[2];
        let p = properties(ring, &[]);
        assert_eq!(
            p.get("label").and_then(|v| v.downcast_ref::<&str>().ok()),
            Some("Ring my__phone")
        );
        assert!(!p.contains_key("enabled"));
        let pause = rows.iter().find(|e| e.id == PAUSE).expect("pause row");
        let p = properties(pause, &[]);
        assert_eq!(
            p.get("toggle-state")
                .and_then(|v| v.downcast_ref::<i32>().ok()),
            Some(1)
        );
        let sep = properties(&rows[1], &[]);
        assert_eq!(
            sep.get("type").and_then(|v| v.downcast_ref::<&str>().ok()),
            Some("separator")
        );
        // Only the asked-for properties.
        assert_eq!(
            properties(pause, &["label".into()])
                .keys()
                .collect::<Vec<_>>(),
            ["label"]
        );

        let (id, root, children) = layout(&s, 0, -1, &[]).expect("root");
        assert_eq!(id, 0);
        assert!(root.contains_key("children-display"));
        assert_eq!(children.len(), rows.len());
        let (_, _, none) = layout(&s, 0, 0, &[]).expect("root only");
        assert!(none.is_empty());
        assert!(layout(&s, 9999, -1, &[]).is_none());
        let (one, props, _) = layout(&s, QUIT, -1, &[]).expect("quit row");
        assert_eq!(one, QUIT);
        assert!(props.contains_key("label"));
    }
}
