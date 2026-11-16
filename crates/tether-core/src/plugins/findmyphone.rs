use crate::Packet;
pub fn parse(p: &Packet) -> bool {
    p.kind == super::FIND_PHONE
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() {
        assert!(parse(&super::super::ring_phone()));
        assert!(!parse(&super::super::ping()));
    }
}
