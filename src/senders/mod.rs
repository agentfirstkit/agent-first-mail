#[cfg(feature = "cloudflare")]
pub mod cloudflare;
#[cfg(not(feature = "cloudflare"))]
#[path = "cloudflare_disabled.rs"]
pub mod cloudflare;
#[cfg(feature = "smtp")]
pub mod smtp;
#[cfg(not(feature = "smtp"))]
#[path = "smtp_disabled.rs"]
pub mod smtp;

use serde::{Deserialize, Serialize};

/// Provider submission evidence. Acceptance and eventual delivery are distinct.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SendReceipt {
    pub message_id: String,
    pub delivered: Vec<String>,
    pub queued: Vec<String>,
    pub permanent_bounces: Vec<String>,
    #[serde(default)]
    pub suppressed_recipients: Vec<String>,
}
