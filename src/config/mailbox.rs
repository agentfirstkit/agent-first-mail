use super::defaults::{default_imap_port, default_true};
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImapSection {
    pub host: Option<String>,
    #[serde(default = "default_imap_port")]
    pub port: u16,
    #[serde(default = "default_true")]
    pub tls: bool,
    pub username: Option<String>,
    pub password_secret: Option<String>,
    pub password_secret_env: Option<String>,
}

impl Default for ImapSection {
    fn default() -> Self {
        Self {
            host: None,
            port: default_imap_port(),
            tls: true,
            username: None,
            password_secret: None,
            password_secret_env: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImapMailboxConfig {
    #[serde(default)]
    pub mailbox_name: Option<String>,
    #[serde(default)]
    pub special_use: Option<String>,
}

impl ImapMailboxConfig {
    pub(super) fn empty() -> Self {
        Self {
            mailbox_name: None,
            special_use: None,
        }
    }

    pub(super) fn validate(&self, id: &str) -> Result<()> {
        match (&self.mailbox_name, &self.special_use) {
            (Some(mailbox), None) if !mailbox.trim().is_empty() => {}
            (None, Some(special_use)) if !special_use.trim().is_empty() => {}
            (Some(_), Some(_)) => {
                return Err(AppError::new(
                    "config_invalid",
                    format!("mailboxes.{id}.mailbox_name and special_use are mutually exclusive"),
                ));
            }
            _ => {
                return Err(AppError::new(
                    "config_invalid",
                    format!("mailboxes.{id} must set exactly one of mailbox_name or special_use"),
                ));
            }
        }
        Ok(())
    }

    pub fn offline_mailbox_name(&self) -> Option<String> {
        if let Some(mailbox) = &self.mailbox_name {
            return Some(mailbox.clone());
        }
        self.special_use
            .as_deref()
            .and_then(SpecialUseKind::from_attribute)
            .map(|kind| kind.fallback_names()[0].to_string())
    }

    pub fn matches_mailbox_offline(&self, mailbox_name: &str) -> bool {
        if self.mailbox_name.as_deref() == Some(mailbox_name) {
            return true;
        }
        self.special_use
            .as_deref()
            .and_then(SpecialUseKind::from_attribute)
            .is_some_and(|kind| {
                kind.fallback_names()
                    .iter()
                    .any(|name| mailbox_name.eq_ignore_ascii_case(name))
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecialUseKind {
    Archive,
    Junk,
    Trash,
    Sent,
    Drafts,
    All,
    Flagged,
}

impl SpecialUseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecialUseKind::Archive => "archive",
            SpecialUseKind::Junk => "junk",
            SpecialUseKind::Trash => "trash",
            SpecialUseKind::Sent => "sent",
            SpecialUseKind::Drafts => "drafts",
            SpecialUseKind::All => "all",
            SpecialUseKind::Flagged => "flagged",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "archive" => Some(SpecialUseKind::Archive),
            "junk" | "spam" => Some(SpecialUseKind::Junk),
            "trash" => Some(SpecialUseKind::Trash),
            "sent" => Some(SpecialUseKind::Sent),
            "drafts" => Some(SpecialUseKind::Drafts),
            "all" => Some(SpecialUseKind::All),
            "flagged" | "flag" => Some(SpecialUseKind::Flagged),
            _ => None,
        }
    }

    pub fn from_attribute(value: &str) -> Option<Self> {
        special_use_kinds()
            .iter()
            .copied()
            .find(|kind| value.eq_ignore_ascii_case(kind.attribute()))
    }

    pub fn attribute(self) -> &'static str {
        match self {
            SpecialUseKind::Archive => "\\Archive",
            SpecialUseKind::Junk => "\\Junk",
            SpecialUseKind::Trash => "\\Trash",
            SpecialUseKind::Sent => "\\Sent",
            SpecialUseKind::Drafts => "\\Drafts",
            SpecialUseKind::All => "\\All",
            SpecialUseKind::Flagged => "\\Flagged",
        }
    }

    pub fn fallback_names(self) -> &'static [&'static str] {
        match self {
            SpecialUseKind::Archive => &["Archive", "Archives"],
            SpecialUseKind::Junk => &["Junk", "Spam", "Junk Email", "Junk E-mail"],
            SpecialUseKind::Trash => &["Trash", "Deleted Items", "Deleted Messages", "Bin"],
            SpecialUseKind::Sent => &["Sent", "Sent Mail", "Sent Messages"],
            SpecialUseKind::Drafts => &["Drafts", "Draft"],
            SpecialUseKind::All => &["All Mail", "All"],
            SpecialUseKind::Flagged => &["Flagged", "Starred"],
        }
    }

    pub fn can_move_to(self) -> bool {
        !matches!(self, SpecialUseKind::All | SpecialUseKind::Flagged)
    }
}

pub fn special_use_kinds() -> &'static [SpecialUseKind] {
    &[
        SpecialUseKind::All,
        SpecialUseKind::Archive,
        SpecialUseKind::Drafts,
        SpecialUseKind::Flagged,
        SpecialUseKind::Junk,
        SpecialUseKind::Sent,
        SpecialUseKind::Trash,
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecialUseTarget {
    pub kind: SpecialUseKind,
    pub mailbox_name: String,
    pub source: SpecialUseSource,
    pub attribute: &'static str,
    pub flag: Option<String>,
    pub can_move_to: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecialUseSource {
    Mailboxes,
    Rfc6154Attribute,
    FallbackName,
}

impl SpecialUseSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecialUseSource::Mailboxes => "mailboxes",
            SpecialUseSource::Rfc6154Attribute => "rfc6154_attribute",
            SpecialUseSource::FallbackName => "fallback_name",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: String,
    pub password_secret: String,
    pub mailboxes: Vec<String>,
}
