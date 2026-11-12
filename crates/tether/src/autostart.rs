//! Start the background service at login, so the phone stays linked without the window open.
//!
//! A systemd user unit where systemd runs the session (most distros, including i3 setups, which
//! skip XDG autostart), otherwise an XDG autostart entry.

use std::{path::PathBuf, process::Command};

use anyhow::{Context, Result};

const UNIT: &str = "tether.service";

fn unit_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("systemd/user").join(UNIT))
}

fn desktop_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart/tether.desktop"))
}

/// The program and argument that start the daemon. Inside an AppImage the binaries live in a mount
/// point that changes every run, so point at the AppImage file itself (its window binary forwards
/// `--daemon`). The window re-runs `autostart on` at each launch, so a moved AppImage is followed.
fn daemon_command() -> Result<(String, &'static str)> {
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        return Ok((appimage, "--daemon"));
    }
    let exe = std::env::current_exe().context("finding the tether binary")?;
    Ok((exe.to_string_lossy().into_owned(), "daemon"))
}

/// The program and leading arguments that run a `tether` subcommand from a desktop entry. An AppImage
/// forwards everything after `--tether` to its bundled `tether`.
pub fn cli_command() -> Result<(String, Vec<&'static str>)> {
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        return Ok((appimage, vec!["--tether"]));
    }
    let exe = std::env::current_exe().context("finding the tether binary")?;
    Ok((exe.to_string_lossy().into_owned(), vec![]))
}

fn is_plain(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
}

/// systemd ExecStart: `%` starts a specifier and `$` a variable, both doubled to be literal.
fn quote_systemd(s: &str) -> String {
    if is_plain(s) {
        return s.to_string();
    }
    let inner = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{inner}\"")
}

/// Desktop-entry Exec: inside quotes `"` `` ` `` `$` `\` take a backslash, then the value's own
/// escaping doubles every backslash again; `%` is a field code and is doubled.
pub fn quote_desktop(s: &str) -> String {
    if is_plain(s) {
        return s.to_string();
    }
    let mut inner = String::new();
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            inner.push('\\');
        }
        inner.push(c);
    }
    format!("\"{}\"", inner.replace('\\', "\\\\").replace('%', "%%"))
}

fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn has_systemd_user() -> bool {
    // "is-system-running" exits non-zero for "degraded" too, which is still usable.
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn enable() -> Result<&'static str> {
    let (prog, arg) = daemon_command()?;
    if prog.contains(['\n', '\r']) {
        anyhow::bail!("Tether's path contains a line break; move it somewhere else first.");
    }
    if has_systemd_user() {
        let path = unit_path().context("no config directory")?;
        std::fs::create_dir_all(path.parent().context("bad unit path")?)?;
        // Not started now: the window may already run a daemon, and the lock would make a second
        // copy exit. It starts at the next login.
        std::fs::write(
            &path,
            format!(
                "[Unit]\nDescription=Tether, your phone on your desktop\n\
                 StartLimitIntervalSec=300\nStartLimitBurst=5\n\n\
                 [Service]\nExecStart={} {arg}\nRestart=on-failure\nRestartSec=10\n\n\
                 [Install]\nWantedBy=default.target\n",
                quote_systemd(&prog)
            ),
        )?;
        systemctl(&["daemon-reload"]);
        if systemctl(&["enable", UNIT]) {
            return Ok("systemd");
        }
        let _ = std::fs::remove_file(&path);
    }
    let path = desktop_path().context("no config directory")?;
    std::fs::create_dir_all(path.parent().context("bad autostart path")?)?;
    std::fs::write(
        &path,
        format!(
            "[Desktop Entry]\nType=Application\nName=Tether\nComment=Keeps your phone linked\n\
             Exec={} {arg}\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            quote_desktop(&prog)
        ),
    )?;
    Ok("autostart")
}

pub fn disable() -> Result<()> {
    if let Some(p) = unit_path().filter(|p| p.exists()) {
        systemctl(&["disable", UNIT]);
        std::fs::remove_file(p)?;
        systemctl(&["daemon-reload"]);
    }
    if let Some(p) = desktop_path().filter(|p| p.exists()) {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

pub fn is_enabled() -> bool {
    unit_path().is_some_and(|p| p.exists()) || desktop_path().is_some_and(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::{quote_desktop, quote_systemd};

    #[test]
    fn quotes_for_systemd() {
        assert_eq!(quote_systemd("/usr/bin/tether"), "/usr/bin/tether");
        assert_eq!(
            quote_systemd("/home/a b/T.AppImage"),
            "\"/home/a b/T.AppImage\""
        );
        assert_eq!(quote_systemd("/x/100% $y"), "\"/x/100%% $$y\"");
        assert_eq!(quote_systemd("/x/\"q\""), "\"/x/\\\"q\\\"\"");
    }

    #[test]
    fn quotes_for_desktop_entries() {
        assert_eq!(quote_desktop("/usr/bin/tether"), "/usr/bin/tether");
        assert_eq!(quote_desktop("/home/a b/T"), "\"/home/a b/T\"");
        // Spec example: a literal $ inside quotes is written \\$ in the file.
        assert_eq!(quote_desktop("/a $b"), "\"/a \\\\$b\"");
        assert_eq!(quote_desktop("/a 5%"), "\"/a 5%%\"");
    }
}
