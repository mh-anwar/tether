//! Small bounded DNS name codec shared by KDE Connect and adb discovery.

pub fn put_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let b = &label.as_bytes()[..label.len().min(63)];
        out.push(b.len() as u8);
        out.extend_from_slice(b);
    }
    out.push(0);
}

/// `qu`: ask for a unicast reply (top bit of the class), for questions sent from port 5353.
pub fn query(questions: &[(&str, u16)], qu: bool) -> Vec<u8> {
    // A non-zero id: legacy-unicast replies echo it, and some responders ignore id 0 from port != 5353.
    let mut out = vec![0x7e, 0x7e, 0, 0, 0, questions.len() as u8, 0, 0, 0, 0, 0, 0];
    for (name, kind) in questions {
        put_name(&mut out, name);
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&(if qu { 0x8001u16 } else { 1 }).to_be_bytes());
    }
    out
}

pub fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// Reads a possibly compressed name at `at`; returns it and the offset just past it.
pub fn read_name(b: &[u8], mut at: usize) -> Option<(String, usize)> {
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
