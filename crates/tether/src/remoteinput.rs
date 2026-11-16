//! XTest on X11; compositor-independent helpers on Wayland. No shell interpolation of phone text.
use anyhow::{bail, Context, Result};
use tether_core::plugins::mousepad::{special_key, Input};
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{
            ConnectionExt as _, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT,
            KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
        },
        xtest::ConnectionExt as _,
    },
};

pub fn has(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|p| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p.join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}
pub fn available() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        has("ydotool") || has("dotool") || has("wtype")
    } else {
        std::env::var_os("DISPLAY").is_some()
    }
}
pub async fn apply(input: Input) -> Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        wayland(&input).await
    } else {
        tokio::task::spawn_blocking(move || x11(&input)).await?
    }
}
fn x11(i: &Input) -> Result<()> {
    let (c, screen) = x11rb::connect(None)?;
    let root = c.setup().roots[screen].root;
    let fake = |kind, detail, x, y| -> Result<()> {
        c.xtest_fake_input(kind, detail, 0, root, x, y, 0)?
            .check()?;
        Ok(())
    };
    let click = |b| -> Result<()> {
        fake(BUTTON_PRESS_EVENT, b, 0, 0)?;
        fake(BUTTON_RELEASE_EVENT, b, 0, 0)
    };
    if i.scroll {
        for _ in 0..((i.dy.abs() / 5.).ceil() as usize).clamp(0, 50) {
            click(if i.dy > 0. { 5 } else { 4 })?;
        }
        for _ in 0..((i.dx.abs() / 5.).ceil() as usize).clamp(0, 50) {
            click(if i.dx > 0. { 7 } else { 6 })?;
        }
    } else if i.dx != 0. || i.dy != 0. {
        let p = c.query_pointer(root)?.reply()?;
        fake(
            MOTION_NOTIFY_EVENT,
            0,
            (f64::from(p.root_x) + i.dx)
                .clamp(0., f64::from(c.setup().roots[screen].width_in_pixels) - 1.)
                as i16,
            (f64::from(p.root_y) + i.dy)
                .clamp(0., f64::from(c.setup().roots[screen].height_in_pixels) - 1.)
                as i16,
        )?;
    }
    if i.singleclick {
        click(1)?;
    }
    if i.doubleclick {
        click(1)?;
        click(1)?;
    }
    if i.middleclick {
        click(2)?;
    }
    if i.rightclick {
        click(3)?;
    }
    if i.singlehold {
        fake(BUTTON_PRESS_EVENT, 1, 0, 0)?;
    }
    if i.singlerelease {
        fake(BUTTON_RELEASE_EVENT, 1, 0, 0)?;
    }
    let min = c.setup().min_keycode;
    let max = c.setup().max_keycode;
    let mapping = c.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let width = usize::from(mapping.keysyms_per_keycode);
    let find = |sym: u32| {
        mapping
            .keysyms
            .chunks(width)
            .enumerate()
            .find_map(|(n, row)| {
                row.iter()
                    .take(2)
                    .position(|s| *s == sym)
                    .map(|col| (min + n as u8, col == 1))
            })
    };
    let modifiers = [
        (i.ctrl, 0xffe3),
        (i.alt, 0xffe9),
        (i.shift, 0xffe1),
        (i.super_key, 0xffeb),
    ]
    .into_iter()
    .filter_map(|(on, sym)| if on { find(sym).map(|v| v.0) } else { None })
    .collect::<Vec<_>>();
    for k in &modifiers {
        fake(KEY_PRESS_EVENT, *k, 0, 0)?;
    }
    let keys = if let Some(k) = special_key(i.special_key) {
        vec![k]
    } else {
        i.key
            .chars()
            .map(|ch| {
                if (ch as u32) <= 0xff {
                    ch as u32
                } else {
                    0x01000000 | ch as u32
                }
            })
            .collect()
    };
    let result = (|| -> Result<()> {
        for sym in keys {
            let spare = mapping
                .keysyms
                .chunks(width)
                .rposition(|r| r.iter().all(|s| *s == 0))
                .map(|n| min + n as u8);
            let mut restore = None;
            let (code, shift) = if let Some(found) = find(sym) {
                found
            } else {
                let code = spare.context("no unused X11 keycode")?;
                let original = mapping.keysyms
                    [(code - min) as usize * width..(code - min) as usize * width + width]
                    .to_vec();
                let mut row = vec![0; width];
                row[0] = sym;
                c.change_keyboard_mapping(1, code, mapping.keysyms_per_keycode, &row)?
                    .check()?;
                restore = Some((code, original));
                (code, false)
            };
            let shift_code = if shift && !i.shift {
                find(0xffe1).map(|v| v.0)
            } else {
                None
            };
            let sent = (|| -> Result<()> {
                if let Some(k) = shift_code {
                    fake(KEY_PRESS_EVENT, k, 0, 0)?;
                }
                fake(KEY_PRESS_EVENT, code, 0, 0)?;
                fake(KEY_RELEASE_EVENT, code, 0, 0)?;
                Ok(())
            })();
            if let Some(k) = shift_code {
                let _ = fake(KEY_RELEASE_EVENT, k, 0, 0);
            }
            // Round trip before restoring, so clients have consumed the temporary mapping.
            let _ = c.get_input_focus()?.reply();
            if let Some((code, row)) = restore {
                c.change_keyboard_mapping(1, code, mapping.keysyms_per_keycode, &row)?
                    .check()?;
            }
            sent?;
        }
        Ok(())
    })();
    for k in modifiers.iter().rev() {
        let _ = fake(KEY_RELEASE_EVENT, *k, 0, 0);
    }
    c.flush()?;
    result
}
fn key_name(sym: u32) -> Option<String> {
    Some(
        match sym {
            0xff08 => "BackSpace",
            0xff09 => "Tab",
            0xff0a => "Return",
            0xff51 => "Left",
            0xff52 => "Up",
            0xff53 => "Right",
            0xff54 => "Down",
            0xff55 => "Page_Up",
            0xff56 => "Page_Down",
            0xff50 => "Home",
            0xff57 => "End",
            0xff0d => "Return",
            0xffff => "Delete",
            0xff1b => "Escape",
            0xff15 => "Sys_Req",
            0xff14 => "Scroll_Lock",
            0xffe3 => "Control_L",
            0xffe9 => "Alt_L",
            0xffe1 => "Shift_L",
            0xffeb => "Super_L",
            0xffbe..=0xffc9 => return Some(format!("F{}", sym - 0xffbe + 1)),
            _ => return None,
        }
        .into(),
    )
}
async fn wayland(i: &Input) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if has("dotool") {
        let mut text = String::new();
        if i.scroll {
            text += &format!("wheel {}\nhwheel {}\n", i.dy as i32, i.dx as i32);
        } else {
            text += &format!("mousemove {} {}\n", i.dx as i32, i.dy as i32);
        }
        for (on, line) in [
            (i.singleclick, "click left\n"),
            (i.doubleclick, "click left\nclick left\n"),
            (i.middleclick, "click middle\n"),
            (i.rightclick, "click right\n"),
            (i.singlehold, "buttondown left\n"),
            (i.singlerelease, "buttonup left\n"),
        ] {
            if on {
                text += line;
            }
        }
        let mods = [
            (i.ctrl, "ctrl"),
            (i.alt, "alt"),
            (i.shift, "shift"),
            (i.super_key, "super"),
        ]
        .into_iter()
        .filter_map(|(on, s)| on.then_some(s))
        .collect::<Vec<_>>();
        let prefix = if mods.is_empty() {
            String::new()
        } else {
            format!("{}+", mods.join("+"))
        };
        if let Some(key) = special_key(i.special_key).and_then(key_name) {
            text += &format!("key {prefix}{key}\n");
        }
        // type is a line command: split newlines rather than allowing phone text to become commands.
        for (n, line) in i.key.split('\n').enumerate() {
            if n > 0 {
                text += "key Return\n";
            }
            if !line.is_empty() {
                if mods.is_empty() {
                    text += &format!("type {}\n", line.replace('\r', ""));
                } else {
                    for ch in line
                        .chars()
                        .filter(|c| !c.is_control() && !c.is_whitespace())
                    {
                        text += &format!("key {prefix}{ch}\n");
                    }
                }
            }
        }
        let mut child = tokio::process::Command::new("dotool")
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        child
            .stdin
            .take()
            .context("dotool stdin")?
            .write_all(text.as_bytes())
            .await?;
        anyhow::ensure!(child.wait().await?.success(), "dotool failed");
        return Ok(());
    }
    if has("ydotool") {
        if i.scroll {
            crate::computer::output(
                "ydotool",
                &[
                    "mousemove",
                    "--wheel",
                    "--",
                    &(i.dx as i32).to_string(),
                    &(i.dy as i32).to_string(),
                ],
            )
            .await?;
        } else if i.dx != 0. || i.dy != 0. {
            crate::computer::output(
                "ydotool",
                &[
                    "mousemove",
                    "--",
                    &(i.dx as i32).to_string(),
                    &(i.dy as i32).to_string(),
                ],
            )
            .await?;
        }
        for (on, code) in [
            (i.singleclick, "0xC0"),
            (i.doubleclick, "0xC0"),
            (i.middleclick, "0xC2"),
            (i.rightclick, "0xC1"),
            (i.singlehold, "0x40"),
            (i.singlerelease, "0x80"),
        ] {
            if on {
                crate::computer::output("ydotool", &["click", code]).await?;
                if i.doubleclick && code == "0xC0" {
                    crate::computer::output("ydotool", &["click", code]).await?;
                }
            }
        }
        // ydotool key uses Linux evdev numbers, not X keycodes.
        let codes: [u16; 33] = [
            0, 14, 15, 28, 105, 103, 106, 108, 104, 109, 102, 107, 28, 111, 1, 99, 70, 29, 56, 42,
            125, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88,
        ];
        let modifiers = [(i.ctrl, 29), (i.alt, 56), (i.shift, 42), (i.super_key, 125)]
            .into_iter()
            .filter_map(|(on, k)| on.then_some(k))
            .collect::<Vec<_>>();
        for k in &modifiers {
            crate::computer::output("ydotool", &["key", &format!("{k}:1")]).await?;
        }
        let result = async {
            if let Some(k) = codes.get(i.special_key as usize).filter(|k| **k != 0) {
                crate::computer::output("ydotool", &["key", &format!("{k}:1"), &format!("{k}:0")])
                    .await?;
            }
            if !i.key.is_empty() {
                crate::computer::output("ydotool", &["type", "--", &i.key]).await?;
            }
            Ok(())
        }
        .await;
        for k in modifiers.iter().rev() {
            let _ = crate::computer::output("ydotool", &["key", &format!("{k}:0")]).await;
        }
        return result;
    }
    if has("wtype")
        && i.dx == 0.
        && i.dy == 0.
        && !i.scroll
        && !i.singleclick
        && !i.doubleclick
        && !i.middleclick
        && !i.rightclick
        && !i.singlehold
        && !i.singlerelease
    {
        let mut args: Vec<String> = vec![];
        for (on, key) in [
            (i.ctrl, "ctrl"),
            (i.alt, "alt"),
            (i.shift, "shift"),
            (i.super_key, "logo"),
        ] {
            if on {
                args.extend(["-M".into(), key.into()]);
            }
        }
        if let Some(key) = special_key(i.special_key).and_then(key_name) {
            args.extend(["-k".into(), key]);
        }
        if !i.key.is_empty() {
            args.extend(["--".into(), i.key.clone()]);
        }
        crate::computer::output(
            "wtype",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await?;
        return Ok(());
    }
    bail!("remote input needs X11, dotool or ydotool")
}
