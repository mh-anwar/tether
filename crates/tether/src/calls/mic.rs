//! Muting this computer's microphone during a call. HFP has no mute the phone carries out for a headset
//! (a headset mutes its own microphone), and PipeWire's Telephony API has none either, so Tether mutes
//! the default microphone: the one PipeWire feeds into the call. Through `wpctl` (WirePlumber, the
//! session manager every PipeWire desktop runs) or, failing that, `pactl` (pipewire-pulse or
//! PulseAudio), so no audio library is linked in.

use std::{path::PathBuf, time::Duration};

use anyhow::{bail, Result};

const WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq)]
pub enum Mic {
    Wpctl(PathBuf),
    Pactl(PathBuf),
    None,
}

impl Mic {
    /// `TETHER_WPCTL` points at a stand-in for tests, the way `TETHER_ADB` does for phone apps.
    pub fn detect() -> Self {
        if let Some(p) = std::env::var_os("TETHER_WPCTL").map(PathBuf::from) {
            return Mic::Wpctl(p);
        }
        if let Some(p) = crate::exec::which("wpctl") {
            return Mic::Wpctl(p);
        }
        crate::exec::which("pactl").map_or(Mic::None, Mic::Pactl)
    }

    pub fn available(&self) -> bool {
        !matches!(self, Mic::None)
    }

    /// Whether the default microphone is muted; None when that can't be read.
    pub async fn muted(&self) -> Option<bool> {
        let (prog, args): (_, &[&str]) = match self {
            Mic::Wpctl(p) => (p, &["get-volume", "@DEFAULT_AUDIO_SOURCE@"]),
            Mic::Pactl(p) => (p, &["get-source-mute", "@DEFAULT_SOURCE@"]),
            Mic::None => return None,
        };
        let (ok, out) = run(prog, args).await.ok()?;
        ok.then(|| parse_muted(self, &out))
    }

    pub async fn set(&self, muted: bool) -> Result<()> {
        let v = if muted { "1" } else { "0" };
        let (prog, args): (_, [&str; 3]) = match self {
            Mic::Wpctl(p) => (p, ["set-mute", "@DEFAULT_AUDIO_SOURCE@", v]),
            Mic::Pactl(p) => (p, ["set-source-mute", "@DEFAULT_SOURCE@", v]),
            Mic::None => bail!("Muting needs PipeWire's wpctl or pactl on this computer"),
        };
        match run(prog, &args).await {
            Ok((true, _)) => Ok(()),
            Ok((false, out)) => {
                tracing::debug!("mute failed: {}", out.trim());
                bail!("This computer has no microphone to mute")
            }
            Err(e) => bail!("Couldn't change the microphone: {e}"),
        }
    }
}

/// pactl translates its output, so both run in the C locale.
async fn run(prog: &std::path::Path, args: &[&str]) -> Result<(bool, String)> {
    crate::exec::run_env(prog, args, &[("LC_ALL", std::ffi::OsStr::new("C"))], WAIT).await
}

/// `wpctl get-volume` prints "Volume: 0.40 [MUTED]"; `pactl get-source-mute` prints "Mute: yes".
fn parse_muted(mic: &Mic, out: &str) -> bool {
    match mic {
        Mic::Wpctl(_) => out.contains("[MUTED]"),
        _ => out
            .lines()
            .any(|l| l.trim().strip_prefix("Mute:").is_some_and(|v| v.trim() == "yes")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_tools() {
        let w = Mic::Wpctl("wpctl".into());
        let p = Mic::Pactl("pactl".into());
        assert!(parse_muted(&w, "Volume: 0.40 [MUTED]\n"));
        assert!(!parse_muted(&w, "Volume: 0.40\n"));
        assert!(parse_muted(&p, "Mute: yes\n"));
        assert!(!parse_muted(&p, "Mute: no\n"));
    }
}
