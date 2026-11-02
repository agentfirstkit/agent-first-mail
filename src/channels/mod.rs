//! Transport-independent message facts and reply routing.
//!
//! Content is untrusted. A route names a configured adapter, never a payload-
//! supplied filesystem path or URL. Email retains its native MIME evidence;
//! non-email channels use the native JSON message contract.
mod files;
pub(crate) use files::pull;
pub use files::{FileSourceConfig, FilesSection, NativeMessage};
pub(crate) use files::{
    materialize, output_confirmed, prepare, publish, record_sent, validate_draft,
};

use crate::error::{AppError, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    File,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReplyRoute {
    pub channel: ChannelKind,
    pub source_id: String,
    pub conversation_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageOrigin {
    pub reply_route: ReplyRoute,
    pub external_message_id: String,
    #[serde(default)]
    pub reply_to_external_message_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct ChannelCapabilities {
    pub reply: bool,
    pub attachments: bool,
    pub acknowledge_input: bool,
    pub remote_mailboxes: bool,
    pub idempotent_publish: bool,
}
impl ChannelKind {
    pub fn capabilities(self) -> ChannelCapabilities {
        match self {
            Self::File => ChannelCapabilities {
                reply: true,
                attachments: false,
                acknowledge_input: false,
                remote_mailboxes: false,
                idempotent_publish: true,
            },
        }
    }
}

pub(crate) fn validate_identifier(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(AppError::new(
            "channel_message_invalid",
            "channel identifiers must contain 1–512 printable UTF-8 bytes",
        ));
    }
    Ok(())
}
pub(crate) fn require_files() -> Result<()> {
    if !cfg!(feature = "files") {
        return Err(AppError::new(
            "feature_disabled",
            "file channels require the files feature",
        ));
    }
    Ok(())
}
