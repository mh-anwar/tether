//! KDE Connect protocol (v8) implementation: discovery, TLS links, pairing and plugins.
//!
//! Upstream references are cited as `kdeconnect-kde:<path>` (desktop) and
//! `kdeconnect-android:<path>` (phone) so behaviour can be checked against the source.

pub mod cache;
pub mod cert;
pub mod discovery;
pub mod error;
pub mod identity;
pub mod link;
pub mod netwatch;
pub mod packet;
pub mod payload;
pub mod plugins;
pub mod service;
pub mod store;
pub mod tls;

pub use error::{Error, Result};
pub use identity::Identity;
pub use packet::Packet;
pub use service::{Config, DeviceSummary, Event, PairState, Service};
