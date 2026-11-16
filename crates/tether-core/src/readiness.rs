//! Capability-based setup hints, not a remote permission audit.
//!
//! kdeconnect-android: src/main/java/org/kde/kdeconnect/plugins/PluginFactory.kt
//! advertises all plugin metadata; Device.kt gates loaded plugins separately on settings
//! and permissions. An advertised capability cannot prove that a plugin is enabled.

use crate::Identity;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FeatureReadiness {
    pub id: &'static str,
    pub label: &'static str,
    /// Required packet type is advertised in the correct direction, not permission status.
    pub ready: bool,
    pub capability: &'static str,
    pub direction: &'static str,
    pub instruction: &'static str,
}

/// Incoming/outgoing are from the phone's perspective.
pub fn phone_readiness(identity: &Identity) -> Vec<FeatureReadiness> {
    let features = [
        ("texts", "Texts", "kdeconnect.sms.messages", "out", "Send SMS (SMS in older versions): turn it on and allow SMS and contacts."),
        ("notifications", "Notifications", "kdeconnect.notification", "out", "Notification sync: turn it on, tap Open settings, and allow notification access for KDE Connect."),
        ("contacts", "Contacts", "kdeconnect.contacts.response_vcards", "out", "Contacts Synchronizer: turn it on and allow Contacts."),
        ("clipboard", "Clipboard", "kdeconnect.clipboard", "out", "Clipboard sync: turn it on. Android may require Send clipboard from the notification."),
        ("share", "Sharing", "kdeconnect.share.request", "in", "Share and receive: turn it on and allow requested file access."),
        ("battery", "Battery", "kdeconnect.battery", "out", "Battery report: turn it on."),
        ("telephony", "Calls", "kdeconnect.telephony", "out", "Telephony notifier: turn it on and allow Phone and call log permissions."),
        ("mpris", "Phone media player", "kdeconnect.mpris", "out", "Media Player Control: turn it on. Your phone must also support sharing its players."),
        ("sftp", "Files and Photos", "kdeconnect.sftp", "out", "Filesystem expose (Filesystem access in newer versions): turn it on. Open its settings > Add storage location, select a folder > Use this folder > Allow."),
        ("keyboard", "Remote keyboard", "kdeconnect.mousepad.keyboardstate", "out", "Remote keyboard (Keyboard receiver in newer versions): turn it on. In Android keyboard settings, enable KDE Connect Remote Keyboard, then select it in a text field."),
        ("runcommand", "Run computer commands", "kdeconnect.runcommand.request", "out", "Run Command: turn it on. Add allowed commands in Tether Settings."),
        ("mousepad", "Control computer input", "kdeconnect.mousepad.request", "out", "Remote input: turn it on. Allow remote input in Tether Settings."),
        ("presenter", "Presentation remote", "kdeconnect.presenter", "out", "Presentation remote: turn it on. Allow presentation control in Tether Settings."),
        ("lock", "Lock computer", "kdeconnect.lock.request", "out", "Lock screen: if this plugin is available, turn it on and allow locking in Tether Settings. Some Android versions do not offer it."),
        ("systemvolume", "Computer volume", "kdeconnect.systemvolume.request", "out", "System volume: turn it on. Allow volume control in Tether Settings."),
        ("findmyphone", "Find computer", "kdeconnect.findmyphone.request", "out", "Find remote device: turn it on. Allow ringing in Tether Settings."),
        ("connectivity", "Mobile signal", "kdeconnect.connectivity_report", "out", "Connectivity report: turn it on and allow requested Phone permissions."),
    ];
    features
        .into_iter()
        .map(|(id, label, capability, direction, instruction)| {
            let advertised = if direction == "in" {
                &identity.incoming_capabilities
            } else {
                &identity.outgoing_capabilities
            };
            FeatureReadiness {
                id,
                label,
                ready: advertised.iter().any(|c| c == capability),
                capability,
                direction,
                instruction,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        Identity {
            device_id: crate::identity::new_device_id(),
            device_name: "Phone".into(),
            device_type: "phone".into(),
            protocol_version: 8,
            incoming_capabilities: vec![],
            outgoing_capabilities: vec![],
            tcp_port: None,
        }
    }
    #[test]
    fn missing_and_wrong_direction_are_not_ready() {
        let mut phone = identity();
        assert!(phone_readiness(&phone).iter().all(|f| !f.ready));
        phone
            .incoming_capabilities
            .push("kdeconnect.sms.messages".into());
        assert!(
            !phone_readiness(&phone)
                .iter()
                .find(|f| f.id == "texts")
                .unwrap()
                .ready
        );
        phone
            .outgoing_capabilities
            .push("kdeconnect.sms.messages".into());
        assert!(
            phone_readiness(&phone)
                .iter()
                .find(|f| f.id == "texts")
                .unwrap()
                .ready
        );
    }
    #[test]
    fn every_mapping_matches_only_its_own_capability() {
        for feature in phone_readiness(&identity()) {
            let mut phone = identity();
            if feature.direction == "in" {
                phone.incoming_capabilities.push(feature.capability.into());
            } else {
                phone.outgoing_capabilities.push(feature.capability.into());
            }
            let ready = phone_readiness(&phone);
            assert_eq!(
                ready.iter().filter(|f| f.ready).count(),
                1,
                "{}",
                feature.id
            );
            assert!(ready.iter().find(|f| f.id == feature.id).unwrap().ready);
        }
    }
}
