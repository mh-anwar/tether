//! XEmbed tray icon (i3bar, polybar, stalonetray) and its right-click menu, drawn with core X
//! fonts so it needs no toolkit. X11 sessions only.

use std::{
    ffi::OsStr,
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, watch};
use tracing::{debug, info};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use super::{
    icon,
    menu::{entries, Action, Entry},
    title, TrayState, SNI_ACTIVE, XEMBED_DOCKED,
};
use std::sync::atomic::Ordering;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Only X11 sessions have XEmbed trays. On Wayland, DISPLAY may still point at Xwayland, which
/// has no bar to dock into, and connecting to it can start Xwayland for nothing.
pub(super) fn wanted(
    display: Option<&OsStr>,
    session_type: Option<&str>,
    wayland_display: Option<&OsStr>,
) -> bool {
    if display.is_none_or(|d| d.is_empty()) {
        return false;
    }
    match session_type {
        Some("x11") => true,
        Some("wayland") => false,
        _ => wayland_display.is_none_or(|d| d.is_empty()),
    }
}

struct Docked {
    win: Window,
    gc: Gcontext,
    depth: u8,
    size: u16,
}

pub(super) fn run(state: watch::Receiver<TrayState>, actions: mpsc::UnboundedSender<Action>) {
    let Ok((conn, screen)) = x11rb::connect(None) else {
        return;
    };
    if let Err(e) = serve(&conn, screen, state, actions) {
        debug!("tray: X11 tray stopped: {e}");
    }
    XEMBED_DOCKED.store(false, Ordering::Relaxed);
}

fn atom(conn: &RustConnection, name: &str) -> Res<Atom> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

fn serve(
    conn: &RustConnection,
    screen_num: usize,
    state: watch::Receiver<TrayState>,
    actions: mpsc::UnboundedSender<Action>,
) -> Res<()> {
    let selection = atom(conn, &format!("_NET_SYSTEM_TRAY_S{screen_num}"))?;
    let mut docked: Option<Docked> = None;
    let mut popup: Option<Popup> = None;
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
            XEMBED_DOCKED.store(docked.is_some(), Ordering::Relaxed);
        }
        let mut redraw = false;
        while let Some(event) = conn.poll_for_event()? {
            if let Some(p) = popup.as_mut() {
                match p.handle(conn, &event)? {
                    PopupResult::Open => continue,
                    PopupResult::Ignored => {}
                    PopupResult::Closed(chosen) => {
                        popup = None;
                        if let Some(a) = chosen {
                            let _ = actions.send(a);
                        }
                        continue;
                    }
                }
            }
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
                    let _ = actions.send(Action::Open);
                }
                Event::ButtonRelease(e) if e.event == d.win && e.detail == 3 => {
                    let rows = entries(&state.borrow());
                    match Popup::open(conn, screen_num, rows, e.root_x, e.root_y) {
                        Ok(p) => popup = Some(p),
                        Err(err) => debug!("tray: menu didn't open: {err}"),
                    }
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
        let n = state.borrow().unread;
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
        // Follow the pointer closely while the menu is open; otherwise a slow poll is enough.
        let pause = if popup.is_some() { 15 } else { 150 };
        std::thread::sleep(Duration::from_millis(pause));
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
    let request = ClientMessageEvent::new(32, owner, opcode, [x11rb::CURRENT_TIME, 0, win, 0, 0]);
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

// ---------- right-click menu ----------

/// Core fonts, best first; "fixed" exists on every X server.
const FONTS: [&str; 3] = [
    "-misc-fixed-medium-r-normal--13-*-*-*-*-*-iso10646-1",
    "-*-dejavu sans-medium-r-normal--13-*-*-*-*-*-iso10646-1",
    "fixed",
];

const PAD_X: i16 = 10;
const CHECK_W: i16 = 18;
const SEP_H: u16 = 9;

// keysyms (X11/keysymdef.h).
const XK_UP: u32 = 0xff52;
const XK_DOWN: u32 = 0xff54;
const XK_RETURN: u32 = 0xff0d;
const XK_KP_ENTER: u32 = 0xff8d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_SPACE: u32 = 0x20;

struct Colours {
    bg: u32,
    fg: u32,
    dim: u32,
    hover: u32,
    line: u32,
}

struct Popup {
    win: Window,
    gc: Gcontext,
    font: Font,
    rows: Vec<Entry>,
    /// Top of each row, and its height.
    spans: Vec<(i16, u16)>,
    width: u16,
    height: u16,
    ascent: i16,
    hover: Option<usize>,
    colours: Colours,
    /// keycode -> first keysym, for arrow keys, Enter and Escape.
    keys: Vec<u32>,
    min_keycode: u8,
}

enum PopupResult {
    /// The event was the menu's; it is still open.
    Open,
    /// Not the menu's event.
    Ignored,
    Closed(Option<Action>),
}

fn utf16(text: &str) -> Vec<Char2b> {
    text.encode_utf16()
        .map(|u| Char2b {
            byte1: (u >> 8) as u8,
            byte2: (u & 0xff) as u8,
        })
        .collect()
}

/// Row tops and heights, and the total height, for rows of `row_h` and thin separators.
fn layout_rows(rows: &[Entry], row_h: u16) -> (Vec<(i16, u16)>, u16) {
    let mut y = 4i16;
    let spans = rows
        .iter()
        .map(|r| {
            let h = if r.is_separator() { SEP_H } else { row_h };
            let span = (y, h);
            y += h as i16;
            span
        })
        .collect();
    (spans, (y + 4) as u16)
}

/// Where the menu goes: at the pointer, flipped up or left when it would leave the screen
/// (bars sit at the bottom as often as at the top).
fn place(pointer: (i16, i16), size: (u16, u16), screen: (u16, u16)) -> (i16, i16) {
    let (px, py) = (pointer.0 as i32, pointer.1 as i32);
    let (w, h) = (size.0 as i32, size.1 as i32);
    let (sw, sh) = (screen.0 as i32, screen.1 as i32);
    let x = if px + w > sw { px - w } else { px };
    let y = if py + h > sh { py - h } else { py };
    (
        x.clamp(0, (sw - w).max(0)) as i16,
        y.clamp(0, (sh - h).max(0)) as i16,
    )
}

impl Popup {
    fn open(
        conn: &RustConnection,
        screen_num: usize,
        rows: Vec<Entry>,
        x: i16,
        y: i16,
    ) -> Res<Popup> {
        let screen = &conn.setup().roots[screen_num];
        let font = conn.generate_id()?;
        let mut opened = false;
        for name in FONTS {
            if conn.open_font(font, name.as_bytes())?.check().is_ok() {
                opened = true;
                break;
            }
        }
        if !opened {
            return Err("no core X font".into());
        }
        let info = conn.query_font(font)?.reply()?;
        let (ascent, descent) = (info.font_ascent, info.font_descent);
        // "fixed" is often Latin-1 only, where an ellipsis would draw as a box.
        let wide = info.max_byte1 > 0;
        let rows: Vec<Entry> = rows
            .into_iter()
            .map(|mut r| {
                if !wide {
                    r.label = r.label.replace('\u{2026}', "...");
                }
                r
            })
            .collect();
        let row_h = (ascent + descent + 10).max(20) as u16;
        let mut text_w = 0i16;
        for r in rows.iter().filter(|r| !r.is_separator()) {
            let ext = conn.query_text_extents(font, &utf16(&r.label))?.reply()?;
            text_w = text_w.max(ext.overall_width.min(1000) as i16);
        }
        let width = (PAD_X + CHECK_W + text_w + PAD_X * 2) as u16;
        let (spans, height) = layout_rows(&rows, row_h);
        let (wx, wy) = place(
            (x, y),
            (width, height),
            (screen.width_in_pixels, screen.height_in_pixels),
        );

        let colour = |r: u8, g: u8, b: u8| -> Res<u32> {
            let rgb = |c: u8| u16::from(c) * 257;
            Ok(conn
                .alloc_color(screen.default_colormap, rgb(r), rgb(g), rgb(b))?
                .reply()?
                .pixel)
        };
        let colours = Colours {
            bg: colour(0x2b, 0x2f, 0x33)?,
            fg: colour(0xf0, 0xf0, 0xf0)?,
            dim: colour(0x8c, 0x92, 0x98)?,
            hover: colour(0x2f, 0x5f, 0xc8)?,
            line: colour(0x48, 0x4e, 0x55)?,
        };

        let win = conn.generate_id()?;
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(colours.bg)
            .border_pixel(colours.line)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::LEAVE_WINDOW
                    | EventMask::KEY_PRESS,
            );
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            screen.root,
            wx,
            wy,
            width,
            height,
            1,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"tether-menu\0Tether\0",
        )?;
        let gc = conn.generate_id()?;
        conn.create_gc(
            gc,
            win,
            &CreateGCAux::new()
                .font(font)
                .line_width(2)
                .graphics_exposures(0),
        )?;
        conn.map_window(win)?;
        conn.flush()?;

        let setup = conn.setup();
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        let mapping = conn
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)?
            .reply()?;
        let per = mapping.keysyms_per_keycode.max(1) as usize;
        let keys = mapping.keysyms.chunks(per).map(|c| c[0]).collect();

        // Take the pointer and keyboard so a click elsewhere or Escape closes the menu. The bar
        // may still hold them for a moment after the click that opened it.
        let events = EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::LEAVE_WINDOW;
        for _ in 0..20 {
            let p = conn
                .grab_pointer(
                    false,
                    win,
                    events,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    x11rb::NONE,
                    x11rb::NONE,
                    x11rb::CURRENT_TIME,
                )?
                .reply()?;
            if p.status == GrabStatus::SUCCESS {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = conn
            .grab_keyboard(
                false,
                win,
                x11rb::CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?
            .reply();
        let p = Popup {
            win,
            gc,
            font,
            rows,
            spans,
            width,
            height,
            ascent,
            hover: None,
            colours,
            keys,
            min_keycode,
        };
        p.draw(conn)?;
        Ok(p)
    }

    fn row_at(&self, x: i16, y: i16) -> Option<usize> {
        if x < 0 || x >= self.width as i16 {
            return None;
        }
        self.spans
            .iter()
            .position(|(top, h)| y >= *top && y < *top + *h as i16)
            .filter(|i| !self.rows[*i].is_separator() && self.rows[*i].enabled)
    }

    fn inside(&self, x: i16, y: i16) -> bool {
        x >= 0 && y >= 0 && x < self.width as i16 && y < self.height as i16
    }

    /// The next usable row after (or before) the hovered one, wrapping around.
    fn step(&self, down: bool) -> Option<usize> {
        let n = self.rows.len();
        let usable = |i: &usize| !self.rows[*i].is_separator() && self.rows[*i].enabled;
        let start = self.hover.unwrap_or(if down { n - 1 } else { 0 });
        (1..=n)
            .map(|k| {
                if down {
                    (start + k) % n
                } else {
                    (start + n - k) % n
                }
            })
            .find(usable)
    }

    fn handle(&mut self, conn: &RustConnection, event: &Event) -> Res<PopupResult> {
        let result = match event {
            Event::Expose(e) if e.window == self.win => {
                if e.count == 0 {
                    self.draw(conn)?;
                }
                PopupResult::Open
            }
            Event::MotionNotify(e) if e.event == self.win => {
                self.set_hover(conn, self.row_at(e.event_x, e.event_y))?;
                PopupResult::Open
            }
            Event::LeaveNotify(e) if e.event == self.win => {
                self.set_hover(conn, None)?;
                PopupResult::Open
            }
            Event::ButtonPress(e) if e.event == self.win => {
                if self.inside(e.event_x, e.event_y) {
                    PopupResult::Open
                } else {
                    PopupResult::Closed(None)
                }
            }
            Event::ButtonRelease(e) if e.event == self.win => {
                if !self.inside(e.event_x, e.event_y) {
                    PopupResult::Open
                } else {
                    match self.row_at(e.event_x, e.event_y) {
                        Some(i) => PopupResult::Closed(self.rows[i].action.clone()),
                        None => PopupResult::Open,
                    }
                }
            }
            Event::KeyPress(e) if e.event == self.win => {
                let sym = self
                    .keys
                    .get(e.detail.wrapping_sub(self.min_keycode) as usize)
                    .copied()
                    .unwrap_or(0);
                match sym {
                    XK_ESCAPE => PopupResult::Closed(None),
                    XK_UP | XK_DOWN => {
                        self.set_hover(conn, self.step(sym == XK_DOWN))?;
                        PopupResult::Open
                    }
                    XK_RETURN | XK_KP_ENTER | XK_SPACE => match self.hover {
                        Some(i) => PopupResult::Closed(self.rows[i].action.clone()),
                        None => PopupResult::Open,
                    },
                    _ => PopupResult::Open,
                }
            }
            _ => PopupResult::Ignored,
        };
        if let PopupResult::Closed(_) = result {
            self.close(conn)?;
        }
        Ok(result)
    }

    fn set_hover(&mut self, conn: &RustConnection, row: Option<usize>) -> Res<()> {
        if row != self.hover {
            self.hover = row;
            self.draw(conn)?;
        }
        Ok(())
    }

    fn draw(&self, conn: &RustConnection) -> Res<()> {
        let c = &self.colours;
        let gc = self.gc;
        conn.change_gc(gc, &ChangeGCAux::new().foreground(c.bg))?;
        conn.poly_fill_rectangle(
            self.win,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            }],
        )?;
        for (i, (row, (top, h))) in self.rows.iter().zip(&self.spans).enumerate() {
            if row.is_separator() {
                let mid = top + (*h as i16) / 2;
                conn.change_gc(gc, &ChangeGCAux::new().foreground(c.line).line_width(1))?;
                conn.poly_segment(
                    self.win,
                    gc,
                    &[Segment {
                        x1: PAD_X,
                        y1: mid,
                        x2: self.width as i16 - PAD_X,
                        y2: mid,
                    }],
                )?;
                continue;
            }
            let hovered = self.hover == Some(i);
            let back = if hovered { c.hover } else { c.bg };
            if hovered {
                conn.change_gc(gc, &ChangeGCAux::new().foreground(back))?;
                conn.poly_fill_rectangle(
                    self.win,
                    gc,
                    &[Rectangle {
                        x: 4,
                        y: *top,
                        width: self.width - 8,
                        height: *h,
                    }],
                )?;
            }
            let text = if row.enabled { c.fg } else { c.dim };
            if row.check == Some(true) {
                // A drawn tick, not a glyph: core fonts may not have one.
                let (x, y) = (PAD_X + 2, top + *h as i16 / 2);
                conn.change_gc(gc, &ChangeGCAux::new().foreground(text).line_width(2))?;
                conn.poly_line(
                    CoordMode::ORIGIN,
                    self.win,
                    gc,
                    &[
                        Point { x, y },
                        Point { x: x + 3, y: y + 3 },
                        Point { x: x + 9, y: y - 4 },
                    ],
                )?;
            }
            conn.change_gc(gc, &ChangeGCAux::new().foreground(text).background(back))?;
            let baseline = top + (*h as i16 + self.ascent) / 2 - 1;
            conn.image_text16(self.win, gc, PAD_X + CHECK_W, baseline, &utf16(&row.label))?;
        }
        conn.flush()?;
        Ok(())
    }

    fn close(&self, conn: &RustConnection) -> Res<()> {
        conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
        conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
        conn.destroy_window(self.win)?;
        conn.free_gc(self.gc)?;
        conn.close_font(self.font)?;
        conn.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xembed_only_on_x11() {
        let d = Some(OsStr::new(":0"));
        let w = Some(OsStr::new("wayland-0"));
        assert!(wanted(d, Some("x11"), None));
        assert!(wanted(d, None, None));
        // Xwayland's DISPLAY on a Wayland desktop.
        assert!(!wanted(d, Some("wayland"), w));
        assert!(!wanted(d, None, w));
        assert!(!wanted(None, Some("x11"), None));
        assert!(!wanted(Some(OsStr::new("")), None, None));
        // An X11 session that inherited a stray WAYLAND_DISPLAY.
        assert!(wanted(d, Some("x11"), w));
    }

    #[test]
    fn menu_fits_on_screen() {
        // Bar at the top: below the pointer, and left of it at the right edge.
        assert_eq!(place((300, 30), (200, 150), (1400, 900)), (300, 30));
        assert_eq!(place((1230, 30), (200, 150), (1400, 900)), (1030, 30));
        // Bar at the bottom: above it.
        assert_eq!(place((300, 890), (200, 150), (1400, 900)), (300, 740));
        let rows = vec![
            Entry {
                id: 1,
                label: "Open Tether".into(),
                enabled: true,
                check: None,
                action: Some(Action::Open),
            },
            Entry {
                id: 2,
                label: String::new(),
                enabled: true,
                check: None,
                action: None,
            },
        ];
        let (spans, height) = layout_rows(&rows, 24);
        assert_eq!(spans, [(4, 24), (28, SEP_H)]);
        assert_eq!(height, 4 + 24 + SEP_H + 4);
    }
}
