use crate::Packet;
use serde_json::json;
/// kdeconnect-kde plugins/battery/batteryplugin.cpp uses <=15 while discharging.
pub fn state(charge: i64, charging: bool, discharging: bool) -> Packet {
    Packet::new(
        super::BATTERY,
        json!({"currentCharge":charge.clamp(0,100),"isCharging":charging,"thresholdEvent":if charge<=15 && discharging {1}else{0}}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets() {
        assert_eq!(state(15, false, true).body["thresholdEvent"], 1);
        assert_eq!(state(15, true, false).body["thresholdEvent"], 0);
        assert_eq!(state(101, false, false).body["currentCharge"], 100);
    }
}
