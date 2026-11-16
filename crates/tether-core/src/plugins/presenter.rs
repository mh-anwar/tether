use crate::Packet;
use serde_json::json;
pub const TYPE: &str = "kdeconnect.presenter";
pub fn parse(p: &Packet) -> Option<super::mousepad::Input> {
    if p.kind != TYPE {
        return None;
    }
    if p.bool("stop") == Some(true) {
        return Some(super::mousepad::Input::default());
    }
    super::mousepad::parse(&Packet::new(super::mousepad::REQUEST, json!(p.body)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() {
        assert_eq!(
            parse(&Packet::new(TYPE, json!({"dx":2.0}))).map(|i| i.dx),
            Some(2.)
        );
        assert_eq!(
            parse(&Packet::new(TYPE, json!({"stop":true,"dx":2}))).map(|i| i.dx),
            Some(0.)
        );
    }
}
