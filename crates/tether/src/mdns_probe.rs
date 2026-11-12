//! Asks the phone directly which wireless-debugging ports it has open, without multicast.
//!
//! adb finds the phone by listening for multicast DNS, which firewalls (ufw, firewalld) and some
//! routers drop. We already know the phone's IP from KDE Connect, so we send a plain DNS question to
//! `<phone>:5353` from an ordinary port. RFC 6762 section 6.7 ("legacy unicast") says a responder
//! must answer such a query straight back to the sender, and a reply to our own outgoing packet gets
//! through stateful firewalls. Android's responders (mDNSResponder, and the newer advertiser in
//! Android 14+) answer these.

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use tokio::net::UdpSocket;

use crate::apps::{Mdns, MdnsKind};

pub const PAIRING: &str = "_adb-tls-pairing._tcp.local";
pub const CONNECT: &str = "_adb-tls-connect._tcp.local";
const PTR: u16 = 12;
const SRV: u16 = 33;

/// Services seen at `ips` within `wait`. Best effort: errors just mean nothing found.
///
/// Asks twice at once: from an ordinary port (legacy unicast, answered straight back), and from our
/// own address's port 5353 with the "unicast reply" bit set (RFC 6762 sections 5.4 and 5.5). Some
/// responders ignore the first kind; the second is a normal mDNS question, and since the answer
/// comes back to the exact address and port we sent from, stateful firewalls let it in.
pub async fn probe(ips: &[IpAddr], wait: Duration) -> Vec<Mdns> {
    let ips: Vec<IpAddr> = ips.iter().copied().filter(IpAddr::is_ipv4).collect();
    if ips.is_empty() {
        return Vec::new();
    }
    let legacy = async {
        match UdpSocket::bind("0.0.0.0:0").await {
            Ok(sock) => ask(&sock, &ips, 5353, false, wait).await,
            Err(_) => Vec::new(),
        }
    };
    let qu = async {
        match mdns_socket(ips[0]).await {
            Some(sock) => ask(&sock, &ips, 5353, true, wait).await,
            None => Vec::new(),
        }
    };
    let (mut found, more) = tokio::join!(legacy, qu);
    for m in more {
        if !found.contains(&m) {
            found.push(m);
        }
    }
    found
}

/// A socket on `<our address towards phone>:5353`. Binding the exact address (not 0.0.0.0) makes the
/// kernel hand unicast replies to us rather than to avahi or adb, which share the port with
/// SO_REUSEADDR on the wildcard address.
async fn mdns_socket(phone: IpAddr) -> Option<UdpSocket> {
    let probe = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    probe.connect((phone, 5353)).await.ok()?;
    let local = SocketAddr::new(probe.local_addr().ok()?.ip(), 5353);
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .ok()?;
    sock.set_reuse_address(true).ok()?;
    sock.set_nonblocking(true).ok()?;
    sock.bind(&local.into()).ok()?;
    UdpSocket::from_std(sock.into()).ok()
}

async fn ask(sock: &UdpSocket, ips: &[IpAddr], port: u16, qu: bool, wait: Duration) -> Vec<Mdns> {
    let mut found = Vec::new();
    let ask = query(&[(PAIRING, PTR), (CONNECT, PTR)], qu);
    for ip in ips {
        let _ = sock.send_to(&ask, (*ip, port)).await;
    }
    let mut asked_srv = HashSet::new();
    let mut buf = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(Ok((n, from))) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await
    {
        if !ips.contains(&from.ip()) {
            continue;
        }
        let Some(reply) = parse(&buf[..n]) else {
            continue;
        };
        for (instance, port) in &reply.srv {
            if let Some(m) = to_mdns(instance, from.ip(), *port) {
                if !found.contains(&m) {
                    found.push(m);
                }
            }
        }
        // Some responders answer the PTR question without the SRV record that holds the port.
        let missing: Vec<&str> = reply
            .ptr
            .iter()
            .filter(|i| !reply.srv.contains_key(*i) && asked_srv.insert((*i).clone()))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            let qs: Vec<(&str, u16)> = missing.iter().map(|i| (*i, SRV)).collect();
            let _ = sock.send_to(&query(&qs, qu), from).await;
        }
    }
    found
}

/// TCP ports open on the phone in Android's ephemeral range, where adbd puts its wireless-debugging
/// ports. The last resort when the phone can't be heard at all: one of these is the pairing port.
/// Closed ports answer at once with a reset, so a sweep takes a second or two on a home network.
pub async fn open_ports(ip: IpAddr) -> Vec<u16> {
    // Android's net.ipv4.ip_local_port_range (the Linux default).
    open_ports_in(ip, 32768, 60999).await
}

async fn open_ports_in(ip: IpAddr, first: u16, last: u16) -> Vec<u16> {
    // Well under the usual 1024 open-file limit, shared with the rest of the service.
    const WORKERS: u16 = 256;
    let mut tasks = tokio::task::JoinSet::new();
    for w in 0..WORKERS {
        tasks.spawn(async move {
            let mut open = Vec::new();
            let Some(mut port) = first.checked_add(w) else {
                return Vec::new();
            };
            while port <= last {
                let dial = tokio::net::TcpStream::connect((ip, port));
                if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(400), dial).await {
                    open.push(port);
                }
                port = match port.checked_add(WORKERS) {
                    Some(p) => p,
                    None => break,
                };
            }
            open
        });
    }
    let mut open = Vec::new();
    while let Some(r) = tasks.join_next().await {
        open.extend(r.unwrap_or_default());
    }
    open.sort_unstable();
    open
}

fn to_mdns(full: &str, ip: IpAddr, port: u16) -> Option<Mdns> {
    let lower = full.to_ascii_lowercase();
    // ASCII lowercasing keeps byte offsets, so the length found in `lower` slices `full` safely.
    let (instance, kind) = [(PAIRING, MdnsKind::Pairing), (CONNECT, MdnsKind::Connect)]
        .into_iter()
        .find_map(|(svc, kind)| Some((lower.strip_suffix(&format!(".{svc}"))?.len(), kind)))?;
    Some(Mdns {
        instance: full[..instance].to_string(),
        kind,
        addr: SocketAddr::new(ip, port),
    })
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let b = &label.as_bytes()[..label.len().min(63)];
        out.push(b.len() as u8);
        out.extend_from_slice(b);
    }
    out.push(0);
}

/// `qu`: ask for a unicast reply (top bit of the class), for questions sent from port 5353.
fn query(questions: &[(&str, u16)], qu: bool) -> Vec<u8> {
    // A non-zero id: legacy-unicast replies echo it, and some responders ignore id 0 from port != 5353.
    let mut out = vec![0x7e, 0x7e, 0, 0, 0, questions.len() as u8, 0, 0, 0, 0, 0, 0];
    for (name, kind) in questions {
        put_name(&mut out, name);
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&(if qu { 0x8001u16 } else { 1 }).to_be_bytes());
    }
    out
}

#[derive(Debug, Default)]
struct Reply {
    /// Instance names (full, like `adb-R5C...-abc._adb-tls-connect._tcp.local`) from PTR records.
    ptr: Vec<String>,
    /// Instance name to port, from SRV records.
    srv: HashMap<String, u16>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// Reads a possibly compressed name at `at`; returns it and the offset just past it.
fn read_name(b: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let len = *b.get(at)? as usize;
        match len {
            0 => return Some((labels.join("."), end.unwrap_or(at + 1))),
            l if l & 0xc0 == 0xc0 => {
                let ptr = (u16_at(b, at)? & 0x3fff) as usize;
                end.get_or_insert(at + 2);
                at = ptr;
            }
            l if l < 64 => {
                let label = b.get(at + 1..at + 1 + l)?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                at += 1 + l;
            }
            _ => return None,
        }
    }
    None
}

fn parse(b: &[u8]) -> Option<Reply> {
    let flags = u16_at(b, 2)?;
    if flags & 0x8000 == 0 {
        return None; // a question, not an answer
    }
    let qd = u16_at(b, 4)? as usize;
    let rr = u16_at(b, 6)? as usize + u16_at(b, 8)? as usize + u16_at(b, 10)? as usize;
    let mut at = 12;
    for _ in 0..qd {
        at = read_name(b, at)?.1 + 4;
    }
    let mut reply = Reply::default();
    for _ in 0..rr {
        let (name, next) = read_name(b, at)?;
        let kind = u16_at(b, next)?;
        let len = u16_at(b, next + 8)? as usize;
        let data = next + 10;
        b.get(data..data + len)?;
        match kind {
            PTR => {
                let (target, _) = read_name(b, data)?;
                if !reply.ptr.contains(&target) {
                    reply.ptr.push(target);
                }
            }
            SRV => {
                reply.srv.insert(name, u16_at(b, data + 4)?);
            }
            _ => {}
        }
        at = data + len;
    }
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply like Android's: PTR answer, SRV in the additional section, names compressed.
    fn reply(with_srv: bool) -> Vec<u8> {
        let mut b = vec![0x7e, 0x7e, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, u8::from(with_srv)];
        let type_at = b.len();
        put_name(&mut b, CONNECT);
        b.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 120]);
        let rdata = [&[3u8][..], b"adb", &[0xc0, type_at as u8]].concat();
        b.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        let inst_at = b.len();
        b.extend_from_slice(&rdata);
        if with_srv {
            b.extend_from_slice(&[0xc0, inst_at as u8, 0, 33, 0x80, 1, 0, 0, 0, 120, 0, 8]);
            b.extend_from_slice(&[0, 0, 0, 0, 0x91, 0x5f, 0xc0, type_at as u8]);
            // port 37215
        }
        b
    }

    #[test]
    fn parses_android_reply() {
        let r = parse(&reply(true)).unwrap();
        assert_eq!(r.ptr, vec![format!("adb.{CONNECT}")]);
        assert_eq!(r.srv.get(&format!("adb.{CONNECT}")), Some(&37215));
        let m = to_mdns(&r.ptr[0], "192.168.1.5".parse().unwrap(), 37215).unwrap();
        assert_eq!(m.kind, MdnsKind::Connect);
        assert_eq!(m.instance, "adb");
        assert_eq!(m.addr, "192.168.1.5:37215".parse().unwrap());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&[0; 5]).is_none());
        assert!(parse(&query(&[(PAIRING, PTR)], false)).is_none());
        let mut loop_ = reply(false);
        loop_.truncate(12);
        loop_.extend_from_slice(&[0xc0, 12]); // points at itself
        loop_[7] = 1;
        assert!(parse(&loop_).is_none());
        let mut cut = reply(true);
        cut.truncate(cut.len() - 3);
        assert!(parse(&cut).is_none());
    }

    #[tokio::test]
    async fn asks_for_the_port_when_only_ptr_comes_back() {
        let phone = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = phone.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            let (_, from) = phone.recv_from(&mut buf).await.unwrap();
            phone.send_to(&reply(false), from).await.unwrap();
            let (n, from) = phone.recv_from(&mut buf).await.unwrap();
            assert_eq!(u16_at(&buf[..n], 4), Some(1)); // one SRV question
            phone.send_to(&reply(true), from).await.unwrap();
        });
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let found = ask(&sock, &[ip], port, false, Duration::from_millis(500)).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].addr.port(), 37215);
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[tokio::test]
    async fn finds_a_listening_port() {
        // A narrow range around our own listener: this box is shared, so no wide sweep in tests.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let open = open_ports_in(
            "127.0.0.1".parse().unwrap(),
            port.saturating_sub(2),
            port.saturating_add(2),
        )
        .await;
        assert!(open.contains(&port), "{open:?}");
    }
}
