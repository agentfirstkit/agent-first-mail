use crate::error::{AppError, Result};
use crate::types::{ContactFrontmatter, MessageAgentSuggestion, MessageHumanSuggestion};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MESSAGE_SCHEMA_VERSION: u64 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaseRelationshipEvidence {
    ReplyHeadersMatchedLocalCase,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseRelationshipSuggestion {
    pub case_uids: Vec<String>,
    pub evidence: CaseRelationshipEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MessageStatus {
    Triage,
    Case,
    Notification,
    Archived,
    Spam,
    Trashed,
    DeletedRemote,
    PushQueued,
}

impl MessageStatus {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "triage" => Ok(Self::Triage),
            "case" => Ok(Self::Case),
            "notification" => Ok(Self::Notification),
            "archived" => Ok(Self::Archived),
            "spam" => Ok(Self::Spam),
            "trashed" => Ok(Self::Trashed),
            "deleted_remote" => Ok(Self::DeletedRemote),
            "push_queued" => Ok(Self::PushQueued),
            other => Err(AppError::new(
                "message_status_invalid",
                format!("unsupported message workspace status: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Case => "case",
            Self::Notification => "notification",
            Self::Archived => "archived",
            Self::Spam => "spam",
            Self::Trashed => "trashed",
            Self::DeletedRemote => "deleted_remote",
            Self::PushQueued => "push_queued",
        }
    }

    pub fn is_terminal_local(self) -> bool {
        matches!(
            self,
            Self::Spam | Self::Trashed | Self::DeletedRemote | Self::PushQueued
        )
    }
}

impl fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MailDirection {
    Inbound,
    Outbound,
}

impl MailDirection {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "inbound" | "received" => Ok(Self::Inbound),
            "outbound" | "sent" => Ok(Self::Outbound),
            other => Err(AppError::new(
                "mail_direction_invalid",
                format!("unsupported mail direction: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageFile {
    pub schema_name: String,
    pub schema_version: u64,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc822_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_suggestion: Option<MessageAgentSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_suggestion: Option<MessageHumanSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_relationship_suggestion: Option<CaseRelationshipSuggestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivered_to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub x_original_to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelope_to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mailing_list_headers: Vec<String>,
    #[serde(default)]
    pub authentication: MessageAuthentication,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_rfc3339: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_rfc3339: Option<String>,
    #[serde(default)]
    pub body_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eml_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentRef>,
    /// Exact contact frontmatter associated from the sender address at
    /// render/materialize time. Read-only on `message show`; refreshed when the
    /// canonical contact changes, or by `afmail render refresh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<ContactFrontmatter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_match: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identity_candidates: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_recipient_emails: Vec<String>,
    pub workspace: WorkspaceState,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageMetadata {
    pub schema_name: String,
    pub schema_version: u64,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_suggestion: Option<MessageAgentSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_suggestion: Option<MessageHumanSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_relationship_suggestion: Option<CaseRelationshipSuggestion>,
}

/// Result of a single email authentication mechanism (SPF / DKIM / DMARC).
///
/// `Missing` means no `Authentication-Results` entry for the mechanism was
/// present at all — which is a distinct, and arguably more suspicious, state
/// than an explicit `none`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthVerdict {
    Pass,
    Fail,
    SoftFail,
    Neutral,
    None,
    TempError,
    PermError,
    #[default]
    Missing,
}

impl AuthVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthVerdict::Pass => "pass",
            AuthVerdict::Fail => "fail",
            AuthVerdict::SoftFail => "softfail",
            AuthVerdict::Neutral => "neutral",
            AuthVerdict::None => "none",
            AuthVerdict::TempError => "temperror",
            AuthVerdict::PermError => "permerror",
            AuthVerdict::Missing => "missing",
        }
    }
}

/// Whether the DMARC-authenticated domain lines up with the visible `From`
/// domain. `Unknown` when there is nothing authenticated to compare against.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthAlignment {
    Aligned,
    Mismatch,
    #[default]
    Unknown,
}

impl AuthAlignment {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthAlignment::Aligned => "aligned",
            AuthAlignment::Mismatch => "mismatch",
            AuthAlignment::Unknown => "unknown",
        }
    }
}

/// Structured view of a message's `Authentication-Results` headers.
///
/// afmail does not classify mail; this only reports what the receiving server
/// asserted (which domain authenticated, and whether it aligns with `From`).
/// Pass authenticates the *domain*, not the legitimacy of the contents.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct MessageAuthentication {
    #[serde(default)]
    pub spf: AuthVerdict,
    #[serde(default)]
    pub dkim: AuthVerdict,
    #[serde(default)]
    pub dmarc: AuthVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmarc_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authenticated_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_domain: Option<String>,
    #[serde(default)]
    pub alignment: AuthAlignment,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<String>,
}

impl MessageAuthentication {
    /// Whether any `Authentication-Results` header was present on the message.
    pub fn has_results(&self) -> bool {
        !self.raw.is_empty()
    }

    /// Whether the result should be surfaced in a warning tone: a hard failure,
    /// a permanent error, or a domain that authenticated but does not align
    /// with the visible `From`.
    pub fn is_warning(&self) -> bool {
        let hard = [self.spf, self.dkim, self.dmarc]
            .into_iter()
            .any(|v| matches!(v, AuthVerdict::Fail | AuthVerdict::PermError));
        let aligned_failure = self.alignment == AuthAlignment::Mismatch
            && [self.spf, self.dkim, self.dmarc]
                .into_iter()
                .any(|v| v == AuthVerdict::Pass);
        hard || aligned_failure
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImapRef {
    pub mailbox_name: String,
    pub uid_validity: u64,
    pub uid: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<RemoteLocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteLocation {
    pub mailbox_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid_validity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    pub observed_rfc3339: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_rfc3339: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRef {
    pub part_id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_rfc3339: Option<String>,
    /// Provenance tag for messages whose archived status comes from a remote
    /// source rather than an explicit local disposition (e.g. "imap-archive").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_sync: Option<RemoteSyncState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<WorkspacePushState>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteSyncState {
    pub archive_eligible: bool,
    pub checked_rfc3339: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePushState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<WorkspacePendingPush>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_completed_rfc3339: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePendingPush {
    pub push_id: String,
    pub kind: String,
    pub queued_rfc3339: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
