//! Notices the computer waking from sleep and the network changing, by polling cheap kernel
//! state each tick. No D-Bus or netlink: it works the same on every desktop, with or without
//! logind or NetworkManager.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::UdpSocket,
    time::{Duration, Instant},
};

/// Sleeps shorter than this keep their links: the phone usually still has them open.
const LONG_SLEEP: Duration = Duration::from_secs(20);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// Woke from a sleep long enough that open links are probably dead.
    pub woke: bool,
    pub network: bool,
}

pub struct NetWatch {
    start: Instant,
    /// Time asleep so far: boot clock minus the monotonic clock, which stops during suspend.
    slept: Option<f64>,
    net: Option<u64>,
}

impl Default for NetWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl NetWatch {
    pub fn new() -> Self {
        let mut w = NetWatch {
            start: Instant::now(),
            slept: None,
            net: None,
        };
        w.slept = w.slept_secs();
        w.net = network_fingerprint();
        w
    }

    pub fn poll(&mut self) -> Change {
        let mut c = Change::default();
        if let Some(now) = self.slept_secs() {
            if let Some(before) = self.slept {
                c.woke = now - before > LONG_SLEEP.as_secs_f64();
            }
            self.slept = Some(now);
        }
        let net = network_fingerprint();
        c.network = net.is_some() && self.net.is_some() && net != self.net;
        if net.is_some() {
            self.net = net;
        }
        c
    }

    fn slept_secs(&self) -> Option<f64> {
        Some(boot_uptime()? - self.start.elapsed().as_secs_f64())
    }
}

/// /proc/uptime counts time in suspend (CLOCK_BOOTTIME); `Instant` doesn't.
fn boot_uptime() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/uptime").ok()?;
    s.split_whitespace().next()?.parse().ok()
}

/// The routing table plus the address we'd use to reach the internet: changes when Wi-Fi
/// switches networks, a cable is plugged in, a VPN comes up, or DHCP hands out a new address.
fn network_fingerprint() -> Option<u64> {
    let mut h = DefaultHasher::new();
    let mut any = false;
    // Only the columns that describe a route; RefCnt and Use are counters.
    for (path, cols) in [
        ("/proc/net/route", &[0usize, 1, 2, 3, 6, 7][..]),
        ("/proc/net/ipv6_route", &[0, 1, 2, 3, 4, 5, 8, 9][..]),
    ] {
        if let Ok(s) = std::fs::read_to_string(path) {
            for line in s.lines() {
                let f: Vec<&str> = line.split_whitespace().collect();
                for &c in cols {
                    f.get(c).hash(&mut h);
                }
            }
            any = true;
        }
    }
    // Connecting a UDP socket sends nothing; it only picks the outgoing address.
    if let Ok(addr) = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
    {
        addr.ip().hash(&mut h);
        any = true;
    }
    any.then(|| h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_system_reports_nothing() {
        let mut w = NetWatch::new();
        assert_eq!(w.poll(), Change::default());
    }

    #[test]
    fn a_jump_in_the_boot_clock_is_a_wake() {
        let mut w = NetWatch::new();
        // Pretend we last looked before a 60 s sleep.
        w.slept = w.slept.map(|s| s - 60.0);
        assert!(w.poll().woke);
        assert!(!w.poll().woke);
    }
}
