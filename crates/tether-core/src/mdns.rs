//! KDE Connect DNS-SD. TXT keys and instance naming follow kdeconnect-kde
//! core/backends/lan/mdnshdiscovery.cpp and Android backends/lan/MdnsDiscovery.kt.
use crate::{
    dns::{put_name, read_name, u16_at},
    Identity,
};
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::net::UdpSocket;

pub const SERVICE: &str = "_kdeconnect._udp.local";
const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);

fn query(questions: &[(&str, u16)], qu: bool) -> Vec<u8> {
    let mut bytes = crate::dns::query(questions, qu);
    // Unlike adb legacy unicast, mDNS questions from port 5353 use transaction ID zero.
    bytes[..2].fill(0);
    bytes
}

#[derive(Debug)]
pub struct Record {
    pub name: String,
    pub ttl: u32,
    pub data: Data,
}
#[derive(Debug)]
pub enum Data {
    Ptr(String),
    Srv(u16, String),
    Txt(HashMap<String, String>),
    A(Ipv4Addr),
}
#[derive(Debug, Default)]
pub struct Message {
    pub questions: Vec<(String, u16, bool)>,
    pub records: Vec<Record>,
    pub response: bool,
}

pub fn parse(b: &[u8]) -> Option<Message> {
    let mut m = Message {
        response: u16_at(b, 2)? & 0x8000 != 0,
        ..Message::default()
    };
    let qd = u16_at(b, 4)?;
    let rr = usize::from(u16_at(b, 6)?) + usize::from(u16_at(b, 8)?) + usize::from(u16_at(b, 10)?);
    let mut at = 12;
    for _ in 0..qd {
        let (name, next) = read_name(b, at)?;
        let class = u16_at(b, next + 2)?;
        m.questions.push((
            name.to_ascii_lowercase(),
            u16_at(b, next)?,
            class & 0x8000 != 0,
        ));
        at = next + 4;
    }
    for _ in 0..rr {
        let (name, next) = read_name(b, at)?;
        let kind = u16_at(b, next)?;
        let class = u16_at(b, next + 2)? & 0x7fff;
        let ttl = u32::from_be_bytes(b.get(next + 4..next + 8)?.try_into().ok()?);
        let len = usize::from(u16_at(b, next + 8)?);
        let start = next + 10;
        let end = start + len;
        let bytes = b.get(start..end)?;
        let data = match kind {
            12 => {
                let (target, stop) = read_name(b, start)?;
                if stop > end {
                    return None;
                }
                Some(Data::Ptr(target.to_ascii_lowercase()))
            }
            33 if len >= 7 => {
                let (target, stop) = read_name(b, start + 6)?;
                if stop > end {
                    return None;
                }
                Some(Data::Srv(
                    u16_at(b, start + 4)?,
                    target.to_ascii_lowercase(),
                ))
            }
            16 => {
                let mut txt = HashMap::new();
                let mut offset = 0;
                while offset < bytes.len() {
                    let n = usize::from(bytes[offset]);
                    offset += 1;
                    let text = std::str::from_utf8(bytes.get(offset..offset + n)?).ok()?;
                    if let Some((k, v)) = text.split_once('=') {
                        txt.insert(k.to_ascii_lowercase(), v.to_string());
                    }
                    offset += n;
                }
                Some(Data::Txt(txt))
            }
            1 if len == 4 => Some(Data::A(Ipv4Addr::new(
                bytes[0], bytes[1], bytes[2], bytes[3],
            ))),
            _ => None,
        };
        if class == 1 {
            if let Some(data) = data {
                m.records.push(Record {
                    name: name.to_ascii_lowercase(),
                    ttl,
                    data,
                });
            }
        }
        at = end;
    }
    Some(m)
}

fn record(out: &mut Vec<u8>, name: &str, kind: u16, data: &[u8]) {
    put_name(out, name);
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&(if kind == 12 { 1u16 } else { 0x8001 }).to_be_bytes());
    out.extend_from_slice(&120u32.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

pub fn announcement(id: &Identity, ip: Ipv4Addr) -> Vec<u8> {
    let instance = format!("{}.{SERVICE}", id.device_id);
    let host = format!("{}.local", id.device_id);
    let mut out = vec![0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0];
    let mut ptr = Vec::new();
    put_name(&mut ptr, &instance);
    record(&mut out, SERVICE, 12, &ptr);
    let mut srv = vec![0; 4];
    srv.extend_from_slice(&id.tcp_port.unwrap_or(1716).to_be_bytes());
    put_name(&mut srv, &host);
    record(&mut out, &instance, 33, &srv);
    let mut txt = Vec::new();
    for (k, v) in [
        ("id", id.device_id.clone()),
        ("name", id.device_name.clone()),
        ("type", id.device_type.clone()),
        ("protocol", id.protocol_version.to_string()),
    ] {
        let mut text = format!("{k}={v}").into_bytes();
        text.truncate(255);
        txt.push(text.len() as u8);
        txt.extend(text);
    }
    record(&mut out, &instance, 16, &txt);
    record(&mut out, &host, 1, &ip.octets());
    out
}

/// getifaddrs gives addresses as well as interface flags; no shell commands or runtime dependency.
fn interfaces() -> Vec<Ipv4Addr> {
    let mut result = Vec::new();
    let mut first = std::ptr::null_mut();
    // SAFETY: libc owns this linked list until freeifaddrs; pointers are checked before access.
    unsafe {
        if libc::getifaddrs(&mut first) != 0 {
            return result;
        }
        let mut current = first;
        while let Some(interface) = current.as_ref() {
            if interface.ifa_flags & libc::IFF_UP as u32 != 0
                && !interface.ifa_addr.is_null()
                && (*interface.ifa_addr).sa_family as i32 == libc::AF_INET
            {
                let addr = &*(interface.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
                if !result.contains(&ip) {
                    result.push(ip);
                }
            }
            current = interface.ifa_next;
        }
        libc::freeifaddrs(first);
    }
    result
}

pub fn bind() -> std::io::Result<UdpSocket> {
    let s = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    s.set_reuse_address(true)?;
    s.set_reuse_port(true)?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(255)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 5353)).into())?;
    UdpSocket::from_std(s.into())
}

#[derive(Default)]
struct Peer {
    port: Option<u16>,
    host: String,
    txt: HashMap<String, String>,
}

/// Advertisements are hints only; the caller uses the usual TLS identity and pin checks.
pub async fn run(
    id: Identity,
    found: tokio::sync::mpsc::Sender<(Identity, SocketAddr)>,
) -> std::io::Result<()> {
    let sock = bind()?;
    let mut joined = Vec::new();
    let mut peers: HashMap<String, Peer> = HashMap::new();
    let mut addresses = HashMap::new();
    let mut asked = HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(15));
    let mut buf = [0; 9000];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Refresh memberships after DHCP/interface changes; also bound untrusted cache lifetime.
                peers.clear(); addresses.clear(); asked.clear();
                let ips = interfaces();
                for old in &joined { if !ips.contains(old) { let _ = sock.leave_multicast_v4(GROUP,*old); } }
                joined.retain(|ip| ips.contains(ip));
                for ip in ips {
                    if !joined.contains(&ip) && sock.join_multicast_v4(GROUP,ip).is_ok() { joined.push(ip); }
                }
                for ip in &joined {
                    let _ = socket2::SockRef::from(&sock).set_multicast_if_v4(ip);
                    let _ = sock.send_to(&query(&[(SERVICE,12)],false),(GROUP,5353)).await;
                    let _ = sock.send_to(&announcement(&id,*ip),(GROUP,5353)).await;
                }
            },
            incoming = sock.recv_from(&mut buf) => {
                let (n,from) = incoming?;
                let Some(m) = parse(&buf[..n]) else { continue; };
                if !m.response {
                    let instance = format!("{}.{SERVICE}",id.device_id).to_ascii_lowercase();
                    let host = format!("{}.local",id.device_id).to_ascii_lowercase();
                    if m.questions.iter().any(|(name,kind,_)| (name == SERVICE && [12,255].contains(kind)) || (name == &instance && [16,33,255].contains(kind)) || (name == &host && [1,255].contains(kind))) {
                        let unicast = from.port() != 5353 || m.questions.iter().any(|q| q.2);
                        let route = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
                            .and_then(|s| { s.connect(from)?; s.local_addr() }).ok();
                        for ip in &joined {
                            if unicast && route.is_some_and(|addr| addr.ip() != *ip) { continue; }
                            let mut answer = announcement(&id,*ip);
                            let legacy = from.port() != 5353;
                            if legacy {
                                answer[..2].copy_from_slice(&buf[..2]);
                                // Legacy DNS clients expect the original question section echoed.
                                let mut end = 12;
                                for _ in &m.questions {
                                    if let Some((_, next)) = read_name(&buf[..n], end) { end = next + 4; }
                                }
                                answer[4..6].copy_from_slice(&buf[4..6]);
                                answer.splice(12..12, buf[12..end].iter().copied());
                            }
                            let target = if legacy || m.questions.iter().any(|q| q.2) {from} else {(GROUP,5353).into()};
                            let _ = socket2::SockRef::from(&sock).set_multicast_if_v4(ip);
                            let _ = sock.send_to(&answer,target).await;
                        }
                    }
                    continue;
                }
                for r in m.records {
                    if r.ttl == 0 {
                        if let Data::Ptr(target) = &r.data { peers.remove(target); }
                        peers.remove(&r.name); addresses.remove(&r.name); continue;
                    }
                    if peers.len() >= 256 || addresses.len() >= 256 { break; }
                    match r.data {
                        Data::Ptr(instance) if r.name == SERVICE => {
                            peers.entry(instance.clone()).or_default();

                        },
                        Data::Srv(port,host) if r.name.ends_with(&format!(".{SERVICE}")) => {
                            let peer = peers.entry(r.name).or_default(); peer.port = Some(port); peer.host = host.clone();

                        },
                        Data::Txt(txt) if r.name.ends_with(&format!(".{SERVICE}")) => { peers.entry(r.name).or_default().txt = txt; },
                        Data::A(ip) => { addresses.insert(r.name,ip); },
                        _ => {},
                    }
                }
                for (instance,p) in &peers {
                    let mut missing = Vec::new();
                    if p.port.is_none() { missing.push((instance.as_str(),33)); }
                    if p.txt.is_empty() { missing.push((instance.as_str(),16)); }
                    if !p.host.is_empty() && !addresses.contains_key(&p.host) { missing.push((p.host.as_str(),1)); }
                    missing.retain(|(name,kind)| asked.len() < 256 && asked.insert((name.to_string(),*kind)));
                    if !missing.is_empty() { let _ = sock.send_to(&query(&missing,true),from).await; }

                    let Some(device_id) = p.txt.get("id") else { continue; };
                    if !instance.eq_ignore_ascii_case(&format!("{device_id}.{SERVICE}")) { continue; }
                    let (Some(port),Some(ip)) = (p.port, addresses.get(&p.host)) else { continue; };
                    let peer = Identity { device_id: device_id.clone(), device_name: p.txt.get("name").cloned().unwrap_or_default(), device_type: p.txt.get("type").cloned().unwrap_or_default(), protocol_version: p.txt.get("protocol").and_then(|v|v.parse().ok()).unwrap_or(0), incoming_capabilities: vec![], outgoing_capabilities: vec![], tcp_port: Some(port) };
                    let _ = found.try_send((peer,SocketAddr::from((*ip,port))));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        Identity {
            device_id: crate::identity::new_device_id(),
            device_name: "Test phone".into(),
            device_type: "phone".into(),
            protocol_version: 8,
            tcp_port: Some(1720),
            incoming_capabilities: vec![],
            outgoing_capabilities: vec![],
        }
    }
    #[test]
    fn codec() {
        let id = identity();
        let bytes = announcement(&id, Ipv4Addr::LOCALHOST);
        let m = parse(&bytes).unwrap();
        assert!(m.response);
        assert_eq!(m.records.len(), 4);
        assert!(matches!(&m.records[1].data, Data::Srv(1720, _)));
        let Data::Txt(txt) = &m.records[2].data else {
            panic!()
        };
        assert_eq!(txt.get("id"), Some(&id.device_id));
        assert_eq!(txt["protocol"], "8");
        assert_eq!(
            parse(&query(&[(SERVICE, 12)], true)).unwrap().questions,
            vec![(SERVICE.into(), 12, true)]
        );
        for n in 0..bytes.len() {
            assert!(parse(&bytes[..n]).is_none());
        }
        assert!(read_name(&[0xc0, 0], 0).is_none());
    }
    #[tokio::test]
    async fn loopback() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        a.send_to(&query(&[(SERVICE, 12)], true), b.local_addr().unwrap())
            .await
            .unwrap();
        let mut buf = [0; 2048];
        let (n, from) = tokio::time::timeout(Duration::from_secs(2), b.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(!parse(&buf[..n]).unwrap().response);
        b.send_to(&announcement(&identity(), Ipv4Addr::LOCALHOST), from)
            .await
            .unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), a.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parse(&buf[..n]).unwrap().records.len(), 4);
    }
}
