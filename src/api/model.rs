#![allow(
    dead_code,
    reason = "response DTO fields are consumed through Serde validation and JSON Schema generation"
)]

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::{
    CaseCommand, CaseCreateArgs, CaseDraftAction, CaseSuggestionAction, CaseSuggestionActionKind,
    CaseSuggestionSetArgs, Command, MessageAction, MessageListScope, MessageSuggestionAction,
    NotificationCommand, NotificationCreateArgs, NotificationSuggestionAction,
    NotificationSuggestionActionKind, NotificationSuggestionSetArgs, TriageAction,
    TriageSuggestionAction, TriageSuggestionSetArgs,
};
use crate::error::{AppError, Result};

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PullRequest {
    /// Configured mailbox ids. Empty uses actions.pull.default_mailbox_ids.
    pub mailbox_ids: Vec<String>,
    /// Estimate candidates and budgets without downloading or writing.
    pub dry_run: bool,
    /// Limit the initial remote candidate window to the last number of days.
    pub since_days: Option<u32>,
    /// Maximum number of new messages stored during this pull.
    pub max_new_messages: Option<usize>,
    /// Maximum total bytes stored for new raw messages during this pull.
    pub max_total_bytes: Option<u64>,
    /// Explicitly include all remote history.
    pub all_history: bool,
}

impl PullRequest {
    pub fn into_command(self) -> Result<Command> {
        if self.all_history && self.since_days.is_some() {
            return Err(AppError::new(
                "invalid_request",
                "all_history and since_days cannot be combined",
            ));
        }
        Ok(Command::Pull {
            ids: self.mailbox_ids,
            dry_run: self.dry_run,
            since_days: self.since_days,
            max_new_messages: self.max_new_messages,
            max_total_bytes: self.max_total_bytes,
            all_history: self.all_history,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TriageListQuery {
    pub limit: Option<usize>,
    pub details: bool,
}

impl TriageListQuery {
    pub fn into_command(self) -> Command {
        Command::Triage {
            action: TriageAction::List {
                limit: self.limit,
                details: self.details,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageScope {
    Triage,
    Case,
    Notification,
    #[default]
    All,
}

impl From<MessageScope> for MessageListScope {
    fn from(value: MessageScope) -> Self {
        match value {
            MessageScope::Triage => Self::Triage,
            MessageScope::Case => Self::Case,
            MessageScope::Notification => Self::Notification,
            MessageScope::All => Self::All,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MessageListQuery {
    pub sender: Option<String>,
    pub subject_contains: Option<String>,
    /// RFC 3339 timestamp or YYYY-MM-DD lower bound.
    pub since: Option<String>,
    pub scope: MessageScope,
}

impl MessageListQuery {
    pub fn into_command(self) -> Command {
        Command::Message {
            action: MessageAction::List {
                sender: self.sender,
                subject_contains: self.subject_contains,
                since: self.since,
                scope: self.scope.into(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDisposition {
    Spam,
    Trash,
    Unspam,
    Untrash,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageDispositionRequest {
    pub disposition: MessageDisposition,
    #[schemars(length(min = 1))]
    pub reason: String,
}

impl MessageDispositionRequest {
    pub fn into_command(self, message_id: String) -> Command {
        let message_id = vec![message_id];
        let reason = Some(self.reason);
        let action = match self.disposition {
            MessageDisposition::Spam => MessageAction::Spam { message_id, reason },
            MessageDisposition::Trash => MessageAction::Trash { message_id, reason },
            MessageDisposition::Unspam => MessageAction::Unspam { message_id, reason },
            MessageDisposition::Untrash => MessageAction::Untrash { message_id, reason },
        };
        Command::Message { action }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessageSuggestionInput {
    CaseCreate {
        #[schemars(length(min = 1))]
        case_name: String,
        #[schemars(length(min = 1))]
        group: String,
    },
    CaseAdd {
        #[schemars(length(min = 1))]
        case_ref: String,
    },
    NotificationCreate {
        #[schemars(length(min = 1))]
        notification_name: String,
        #[schemars(length(min = 1))]
        summary: String,
    },
    NotificationAdd {
        #[schemars(length(min = 1))]
        notification_ref: String,
        #[schemars(length(min = 1))]
        summary: String,
    },
    Spam,
    Trash,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageSuggestionSetRequest {
    #[schemars(length(min = 1))]
    pub message_review_base_hash: String,
    pub suggested_action: MessageSuggestionInput,
    #[schemars(length(min = 1))]
    pub reason: String,
    #[schemars(range(max = 100))]
    pub confidence_percent: Option<u8>,
}

impl MessageSuggestionSetRequest {
    pub fn into_command(self, message_id: String) -> Command {
        let action = match self.suggested_action {
            MessageSuggestionInput::CaseCreate { case_name, group } => {
                MessageSuggestionAction::CaseCreate {
                    name: case_name,
                    group,
                }
            }
            MessageSuggestionInput::CaseAdd { case_ref } => {
                MessageSuggestionAction::CaseAdd { case_ref }
            }
            MessageSuggestionInput::NotificationCreate {
                notification_name,
                summary,
            } => MessageSuggestionAction::NotificationCreate {
                name: notification_name,
                summary,
            },
            MessageSuggestionInput::NotificationAdd {
                notification_ref,
                summary,
            } => MessageSuggestionAction::NotificationAdd {
                notification_ref,
                summary,
            },
            MessageSuggestionInput::Spam => MessageSuggestionAction::Spam,
            MessageSuggestionInput::Trash => MessageSuggestionAction::Trash,
        };
        Command::Triage {
            action: TriageAction::Suggestion {
                action: TriageSuggestionAction::Set(TriageSuggestionSetArgs {
                    message_id,
                    message_review_base_hash: self.message_review_base_hash,
                    reason: self.reason,
                    confidence_percent: self.confidence_percent,
                    action,
                }),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestionClearQuery {
    #[schemars(length(min = 1))]
    pub agent_suggestion_uid: String,
}

pub fn message_suggestion_clear_command(
    message_id: String,
    query: SuggestionClearQuery,
) -> Command {
    Command::Triage {
        action: TriageAction::Suggestion {
            action: TriageSuggestionAction::Clear {
                message_id,
                agent_suggestion_uid: query.agent_suggestion_uid,
            },
        },
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseScope {
    #[default]
    Active,
    Archived,
    All,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CaseListQuery {
    pub scope: CaseScope,
}

impl CaseListQuery {
    pub fn into_command(self) -> Command {
        let (archived, all) = match self.scope {
            CaseScope::Active => (false, false),
            CaseScope::Archived => (true, false),
            CaseScope::All => (false, true),
        };
        Command::Case {
            action: CaseCommand::List { archived, all },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseCreateRequest {
    #[schemars(length(min = 1))]
    pub case_name: String,
    #[schemars(length(min = 1))]
    pub group: String,
    #[schemars(length(min = 1))]
    pub message_id: Option<String>,
    pub summary: Option<String>,
    pub reason: Option<String>,
}

impl CaseCreateRequest {
    pub fn into_command(self) -> Command {
        Command::Case {
            action: CaseCommand::Create(CaseCreateArgs {
                name: self.case_name,
                group: Some(self.group),
                message: self.message_id,
                summary: self.summary,
                reason: self.reason,
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseMessageAddRequest {
    #[schemars(length(min = 1))]
    pub message_id: String,
    pub summary: Option<String>,
    #[schemars(length(min = 1))]
    pub reason: String,
}

impl CaseMessageAddRequest {
    pub fn into_command(self, case_ref: String) -> Command {
        Command::Case {
            action: CaseCommand::Add {
                case_ref,
                message_id: vec![self.message_id],
                summary: self.summary,
                reason: Some(self.reason),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaseActionRequest {
    Archive {
        #[schemars(length(min = 1))]
        reason: String,
    },
    Unarchive {
        #[schemars(length(min = 1))]
        group: String,
        #[schemars(length(min = 1))]
        reason: String,
    },
    Move {
        #[schemars(length(min = 1))]
        group: String,
        #[schemars(length(min = 1))]
        reason: String,
    },
    Rename {
        #[schemars(length(min = 1))]
        case_name: String,
        #[schemars(length(min = 1))]
        reason: String,
    },
    Tag {
        #[schemars(length(min = 1))]
        tag: String,
        #[schemars(length(min = 1))]
        reason: String,
    },
    Untag {
        #[schemars(length(min = 1))]
        tag: String,
        #[schemars(length(min = 1))]
        reason: String,
    },
    Trash {
        #[schemars(length(min = 1))]
        reason: String,
    },
    Delete {
        allow_notes: bool,
        #[schemars(length(min = 1))]
        reason: String,
    },
}

impl CaseActionRequest {
    pub fn into_command(self, case_ref: String) -> Command {
        let action = match self {
            Self::Archive { reason } => CaseCommand::Archive {
                case_ref,
                reason: Some(reason),
            },
            Self::Unarchive { group, reason } => CaseCommand::Unarchive {
                case_ref,
                group,
                reason: Some(reason),
            },
            Self::Move { group, reason } => CaseCommand::Move {
                case_ref,
                group,
                reason: Some(reason),
            },
            Self::Rename { case_name, reason } => CaseCommand::Rename {
                case_ref,
                name: case_name,
                reason: Some(reason),
            },
            Self::Tag { tag, reason } => CaseCommand::Tag {
                case_ref,
                tag,
                reason: Some(reason),
            },
            Self::Untag { tag, reason } => CaseCommand::Untag {
                case_ref,
                tag,
                reason: Some(reason),
            },
            Self::Trash { reason } => CaseCommand::Trash {
                case_ref,
                reason: Some(reason),
            },
            Self::Delete {
                allow_notes,
                reason,
            } => CaseCommand::Delete {
                case_ref,
                allow_notes,
                reason: Some(reason),
            },
        };
        Command::Case { action }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaseSuggestionInput {
    #[serde(rename = "case_archive")]
    Archive,
    #[serde(rename = "case_unarchive")]
    Unarchive {
        #[schemars(length(min = 1))]
        group: String,
    },
    #[serde(rename = "case_move")]
    Move {
        #[schemars(length(min = 1))]
        group: String,
    },
    #[serde(rename = "case_rename")]
    Rename {
        #[schemars(length(min = 1))]
        case_name: String,
    },
    #[serde(rename = "case_tag")]
    Tag {
        #[schemars(length(min = 1))]
        tag: String,
    },
    #[serde(rename = "case_untag")]
    Untag {
        #[schemars(length(min = 1))]
        tag: String,
    },
    #[serde(rename = "case_trash")]
    Trash,
    #[serde(rename = "case_delete")]
    Delete { allow_notes: bool },
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseSuggestionSetRequest {
    #[schemars(length(min = 1))]
    pub case_review_base_hash: String,
    pub suggested_action: CaseSuggestionInput,
    #[schemars(length(min = 1))]
    pub reason: String,
    #[schemars(range(max = 100))]
    pub confidence_percent: Option<u8>,
}

impl CaseSuggestionSetRequest {
    pub fn into_command(self, case_ref: String) -> Command {
        let action = match self.suggested_action {
            CaseSuggestionInput::Archive => CaseSuggestionActionKind::Archive,
            CaseSuggestionInput::Unarchive { group } => {
                CaseSuggestionActionKind::Unarchive { group }
            }
            CaseSuggestionInput::Move { group } => CaseSuggestionActionKind::Move { group },
            CaseSuggestionInput::Rename { case_name } => {
                CaseSuggestionActionKind::Rename { name: case_name }
            }
            CaseSuggestionInput::Tag { tag } => CaseSuggestionActionKind::Tag { tag },
            CaseSuggestionInput::Untag { tag } => CaseSuggestionActionKind::Untag { tag },
            CaseSuggestionInput::Trash => CaseSuggestionActionKind::Trash,
            CaseSuggestionInput::Delete { allow_notes } => {
                CaseSuggestionActionKind::Delete { allow_notes }
            }
        };
        Command::Case {
            action: CaseCommand::Suggestion {
                action: CaseSuggestionAction::Set(CaseSuggestionSetArgs {
                    case_ref,
                    case_review_base_hash: self.case_review_base_hash,
                    reason: self.reason,
                    confidence_percent: self.confidence_percent,
                    action,
                }),
            },
        }
    }
}

pub fn case_suggestion_clear_command(case_ref: String, query: SuggestionClearQuery) -> Command {
    Command::Case {
        action: CaseCommand::Suggestion {
            action: CaseSuggestionAction::Clear {
                case_ref,
                agent_suggestion_uid: query.agent_suggestion_uid,
            },
        },
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationCreateRequest {
    #[schemars(length(min = 1))]
    pub notification_name: String,
    #[schemars(length(min = 1))]
    pub message_id: Option<String>,
    pub summary: Option<String>,
    pub reason: Option<String>,
}

impl NotificationCreateRequest {
    pub fn into_command(self) -> Command {
        Command::Notification {
            action: NotificationCommand::Create(NotificationCreateArgs {
                name: self.notification_name,
                message: self.message_id,
                summary: self.summary,
                reason: self.reason,
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationMessageAddRequest {
    #[schemars(length(min = 1))]
    pub message_id: String,
    #[schemars(length(min = 1))]
    pub summary: String,
    #[schemars(length(min = 1))]
    pub reason: String,
}

impl NotificationMessageAddRequest {
    pub fn into_command(self, notification_ref: String) -> Command {
        Command::Notification {
            action: NotificationCommand::Add {
                notification_ref,
                message_id: vec![self.message_id],
                summary: self.summary,
                reason: Some(self.reason),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationActionRequest {
    Rename {
        #[schemars(length(min = 1))]
        notification_name: String,
        #[schemars(length(min = 1))]
        reason: String,
    },
    Delete {
        allow_notes: bool,
        #[schemars(length(min = 1))]
        reason: String,
    },
}

impl NotificationActionRequest {
    pub fn into_command(self, notification_ref: String) -> Command {
        let action = match self {
            Self::Rename {
                notification_name,
                reason,
            } => NotificationCommand::Rename {
                notification_ref,
                name: notification_name,
                reason: Some(reason),
            },
            Self::Delete {
                allow_notes,
                reason,
            } => NotificationCommand::Delete {
                notification_ref,
                allow_notes,
                reason: Some(reason),
            },
        };
        Command::Notification { action }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationSuggestionInput {
    NotificationRename {
        #[schemars(length(min = 1))]
        notification_name: String,
    },
    NotificationDelete {
        allow_notes: bool,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationSuggestionSetRequest {
    #[schemars(length(min = 1))]
    pub notification_review_base_hash: String,
    pub suggested_action: NotificationSuggestionInput,
    #[schemars(length(min = 1))]
    pub reason: String,
    #[schemars(range(max = 100))]
    pub confidence_percent: Option<u8>,
}

impl NotificationSuggestionSetRequest {
    pub fn into_command(self, notification_ref: String) -> Command {
        let action = match self.suggested_action {
            NotificationSuggestionInput::NotificationRename { notification_name } => {
                NotificationSuggestionActionKind::Rename {
                    name: notification_name,
                }
            }
            NotificationSuggestionInput::NotificationDelete { allow_notes } => {
                NotificationSuggestionActionKind::Delete { allow_notes }
            }
        };
        Command::Notification {
            action: NotificationCommand::Suggestion {
                action: NotificationSuggestionAction::Set(NotificationSuggestionSetArgs {
                    notification_ref,
                    notification_review_base_hash: self.notification_review_base_hash,
                    reason: self.reason,
                    confidence_percent: self.confidence_percent,
                    action,
                }),
            },
        }
    }
}

pub fn notification_suggestion_clear_command(
    notification_ref: String,
    query: SuggestionClearQuery,
) -> Command {
    Command::Notification {
        action: NotificationCommand::Suggestion {
            action: NotificationSuggestionAction::Clear {
                notification_ref,
                agent_suggestion_uid: query.agent_suggestion_uid,
            },
        },
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftCreateRequest {
    New {
        #[schemars(length(min = 1))]
        to: Vec<String>,
        #[serde(default)]
        cc: Vec<String>,
        #[schemars(length(min = 1))]
        subject: String,
        identity: Option<String>,
        body: Option<String>,
    },
    Reply {
        #[schemars(length(min = 1))]
        message_id: String,
        identity: Option<String>,
        body: Option<String>,
        #[serde(default)]
        all: bool,
    },
}

impl DraftCreateRequest {
    pub fn into_command(self, case_ref: String) -> Result<Command> {
        let action = match self {
            Self::New {
                to,
                cc,
                subject,
                identity,
                body,
            } => {
                if to.is_empty() {
                    return Err(AppError::new(
                        "invalid_request",
                        "a new draft requires at least one to recipient",
                    ));
                }
                CaseDraftAction::New {
                    case_ref,
                    to,
                    cc,
                    subject,
                    identity,
                    body,
                    body_file: None,
                }
            }
            Self::Reply {
                message_id,
                identity,
                body,
                all,
            } => CaseDraftAction::Reply {
                case_ref,
                message_id,
                identity,
                body,
                body_file: None,
                all,
            },
        };
        Ok(Command::Case {
            action: CaseCommand::Draft { action },
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(extend("minProperties" = 1))]
pub struct DraftPatchRequest {
    pub subject: Option<String>,
    /// Replacement To recipients. An empty list is rejected.
    #[schemars(length(min = 1))]
    pub to: Option<Vec<String>>,
    /// Replacement Cc recipients. An empty list clears Cc.
    pub cc: Option<Vec<String>>,
    pub identity: Option<String>,
    pub body: Option<String>,
}

impl DraftPatchRequest {
    pub fn into_command(self, case_ref: String, draft_name: String) -> Result<Command> {
        if self.subject.is_none()
            && self.to.is_none()
            && self.cc.is_none()
            && self.identity.is_none()
            && self.body.is_none()
        {
            return Err(AppError::new(
                "invalid_request",
                "draft patch must include at least one field",
            ));
        }
        let to = match self.to {
            Some(to) if to.is_empty() => {
                return Err(AppError::new(
                    "invalid_request",
                    "draft to must contain at least one mailbox",
                ));
            }
            Some(to) => to,
            None => Vec::new(),
        };
        let (cc, clear_cc) = match self.cc {
            Some(cc) if cc.is_empty() => (Vec::new(), true),
            Some(cc) => (cc, false),
            None => (Vec::new(), false),
        };
        Ok(Command::Case {
            action: CaseCommand::Draft {
                action: CaseDraftAction::Change {
                    case_ref,
                    draft_name,
                    subject: self.subject,
                    to,
                    cc,
                    clear_cc,
                    identity: self.identity,
                    body: self.body,
                    body_file: None,
                },
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DraftQueueAction {
    SaveDraft,
    Send,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftQueueRequest {
    pub action: DraftQueueAction,
}

impl DraftQueueRequest {
    pub fn into_command(self, case_ref: String, draft_name: String) -> Command {
        let action = match self.action {
            DraftQueueAction::SaveDraft => CaseDraftAction::Upload {
                case_ref,
                draft_name,
            },
            DraftQueueAction::Send => CaseDraftAction::Send {
                case_ref,
                draft_name,
            },
        };
        Command::Case {
            action: CaseCommand::Draft { action },
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DraftDeleteQuery {
    #[schemars(length(min = 1))]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushConfirmRequest {
    #[schemars(length(min = 1))]
    pub plan_id: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct ApiErrorPayload {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub hint: Option<String>,
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiTrace {
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiErrorEnvelope {
    pub kind: ApiErrorKind,
    pub error: ApiErrorPayload,
    pub trace: ApiTrace,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorKind {
    Error,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct HealthResult {
    pub service: String,
    pub version: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PullResult {
    Applied(Box<PullAppliedResult>),
    DryRun(PullDryRunResult),
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PullAppliedResult {
    pub new_message_count: usize,
    pub triage_created_count: usize,
    pub archived_message_count: usize,
    pub spam_message_count: usize,
    pub trashed_message_count: usize,
    pub sent_message_count: usize,
    pub draft_message_count: usize,
    pub indexed_message_count: usize,
    pub flagged_message_count: usize,
    pub reclassified_message_count: usize,
    pub updated_location_count: usize,
    pub flags_updated_count: usize,
    pub mailbox_ids: Vec<String>,
    pub since_days: Option<u32>,
    pub all_history: bool,
    pub body_fetch_mode: String,
    pub max_new_messages: usize,
    pub max_total_bytes: u64,
    pub mailbox_count: usize,
    pub budget_limited: bool,
    pub budget_skipped_count: usize,
    pub remaining_message_budget: usize,
    pub remaining_bytes_budget: u64,
    pub stored_new_message_bytes: u64,
    pub skipped_message_count: usize,
    pub skipped_messages: Vec<Value>,
    pub mailbox_names: Vec<String>,
    pub duration_ms: u64,
    pub new_message_ids: Option<Vec<String>>,
    pub new_message_ids_truncated: Option<bool>,
    pub new_message_ids_count: Option<usize>,
    pub triage_created_message_ids: Option<Vec<String>>,
    pub triage_created_message_ids_truncated: Option<bool>,
    pub triage_created_message_ids_count: Option<usize>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PullDryRunResult {
    pub dry_run: bool,
    pub remote_read: bool,
    pub local_changed: bool,
    pub mailbox_ids: Vec<String>,
    pub since_days: Option<u32>,
    pub all_history: bool,
    pub max_new_messages: usize,
    pub max_total_bytes: u64,
    pub remote_envelope_count: usize,
    pub candidate_count: usize,
    pub candidate_declared_bytes: u64,
    pub candidates: Vec<Value>,
    pub budget_limited: bool,
    pub budget_skipped_count: usize,
    pub budget_skipped: Vec<Value>,
    pub remaining_message_budget: usize,
    pub remaining_bytes_budget: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum StatusResult {
    Ready(StatusReadyResult),
    Locked(StatusLockedResult),
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct StatusReadyResult {
    pub workspace_uid: String,
    pub workspace_path: String,
    pub triage_count: usize,
    pub case_count: usize,
    pub notification_count: usize,
    pub message_count: usize,
    pub push_count: usize,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct StatusLockedResult {
    pub workspace_locked: bool,
    pub progress: Value,
    pub hint: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct LocatorItem {
    pub message_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct TriageListResult {
    pub count: usize,
    pub returned_count: usize,
    pub items: Vec<LocatorItem>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageListResult {
    pub count: usize,
    pub filters: MessageListFilters,
    pub items: Vec<MessageListItem>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageListFilters {
    pub sender: Option<String>,
    pub subject_contains: Option<String>,
    pub since: Option<String>,
    pub scope: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageListItem {
    pub message_id: String,
    pub from: Option<String>,
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub direction: Option<String>,
    pub received_rfc3339: Option<String>,
    pub sent_rfc3339: Option<String>,
    pub workspace_status: String,
    pub notification_uid: Option<String>,
    pub case_refs: Vec<crate::render_document::CaseReferenceDocument>,
    pub json_path: String,
    pub view_path: Option<String>,
}

pub(crate) type MessageShowResult = crate::render_document::MessageShowDocument;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MessageDispositionResult {
    Discarded(MessageDiscardedResult),
    Restored(MessageRestoredResult),
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageDiscardedResult {
    pub message_id: String,
    pub special_use: String,
    pub message_ids: Vec<String>,
    pub queued: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageRestoredResult {
    pub message_id: String,
    pub from_status: String,
    pub status: String,
    pub queued: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageAgentSuggestionResult {
    pub agent_suggestion_uid: String,
    pub message_review_base_hash: String,
    pub suggested_action: MessageSuggestionInput,
    pub reason: String,
    pub confidence_percent: Option<u8>,
    pub created_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageSuggestionSetResult {
    pub message_id: String,
    pub agent_suggestion: MessageAgentSuggestionResult,
    pub human_suggestion_cleared: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct MessageSuggestionClearResult {
    pub message_id: String,
    pub agent_suggestion_uid: String,
    pub suggestion_cleared: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseListItem {
    pub case_uid: String,
    pub case_name: String,
    pub group: String,
    pub status: String,
    pub case_dir: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseListResult {
    pub scope: String,
    pub count: usize,
    pub active_count: usize,
    pub archiving_count: usize,
    pub archived_count: usize,
    pub path_templates: Value,
    pub items: Vec<CaseListItem>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub(crate) type CaseShowResult = crate::render_document::CollectionShowDocument;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseCreateResult {
    pub case_uid: String,
    pub case_name: String,
    pub group: String,
    pub message_ids: Vec<String>,
    pub message_count: usize,
    pub case_path: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseMessageAddResult {
    pub case_uid: String,
    pub message_id: String,
    pub group: String,
    pub message_count: usize,
    pub case_path: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseActionResult {
    pub case_uid: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseAgentSuggestionResult {
    pub agent_suggestion_uid: String,
    pub case_review_base_hash: String,
    pub suggested_action: CaseSuggestionInput,
    pub reason: String,
    pub confidence_percent: Option<u8>,
    pub created_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseSuggestionSetResult {
    pub case_uid: String,
    pub agent_suggestion: CaseAgentSuggestionResult,
    pub human_suggestion_cleared: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CaseSuggestionClearResult {
    pub case_uid: String,
    pub agent_suggestion_uid: String,
    pub suggestion_cleared: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationListItem {
    pub notification_uid: String,
    pub notification_name: String,
    pub notification_dir: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationListResult {
    pub count: usize,
    pub path_templates: Value,
    pub items: Vec<NotificationListItem>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub(crate) type NotificationShowResult = crate::render_document::CollectionShowDocument;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationCreateResult {
    pub notification_uid: String,
    pub notification_name: String,
    pub message_count: usize,
    pub path: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationMessageAddResult {
    pub message_id: String,
    pub notification_uid: String,
    pub path: String,
    pub queued: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationActionResult {
    pub notification_uid: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationAgentSuggestionResult {
    pub agent_suggestion_uid: String,
    pub notification_review_base_hash: String,
    pub suggested_action: NotificationSuggestionInput,
    pub reason: String,
    pub confidence_percent: Option<u8>,
    pub created_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationSuggestionSetResult {
    pub notification_uid: String,
    pub agent_suggestion: NotificationAgentSuggestionResult,
    pub human_suggestion_cleared: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct NotificationSuggestionClearResult {
    pub notification_uid: String,
    pub agent_suggestion_uid: String,
    pub suggestion_cleared: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftListItem {
    pub draft_name: String,
    pub draft_path: String,
    pub status: String,
    pub queued_action: Option<String>,
    pub push_id: Option<String>,
    pub remote_changed: bool,
    pub completeness: String,
    pub subject: Option<String>,
    pub identity: Option<String>,
    pub last_validated_hash: Option<String>,
    pub last_validated_rfc3339: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftListResult {
    pub case_uid: String,
    pub draft_count: usize,
    pub drafts: Vec<DraftListItem>,
}

pub(crate) type DraftShowResult = crate::render_document::DraftRenderDocument;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftCreateResult {
    pub case_uid: String,
    pub draft_name: String,
    pub draft_path: String,
    pub identity: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftPatchResult {
    pub case_uid: String,
    pub draft_name: String,
    pub draft_path: String,
    pub changed_fields: Vec<String>,
    pub draft_hash: String,
    pub completeness: String,
    pub last_validated_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftDeleteResult {
    pub case_uid: String,
    pub draft_name: String,
    pub draft_path: String,
    pub draft_deleted: bool,
    pub queued_removed: bool,
    pub mail_sent: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftValidateResult {
    pub case_uid: String,
    pub draft_name: String,
    pub draft_hash: String,
    pub completeness: String,
    pub last_validated_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct DraftQueueResult {
    pub push_id: String,
    pub kind: String,
    pub action: String,
    pub case_uid: String,
    pub draft_name: String,
    pub draft_path: String,
    pub completeness: String,
    pub last_validated_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PushPlanItemResult {
    pub push_id: String,
    pub queue_item_hash: String,
    pub kind: String,
    pub display_kind: String,
    pub effect_scope: String,
    pub action: Option<String>,
    pub case_uid: Option<String>,
    pub draft_name: Option<String>,
    pub message_id: Option<String>,
    pub actions: Vec<String>,
    pub steps: Vec<Value>,
    pub step_states: Vec<Value>,
    pub attempt_count: u64,
    pub local_stage: Option<Value>,
    pub audit_pending: bool,
    pub last_error: Option<String>,
    pub message_ids: Vec<String>,
    pub locations: Vec<Value>,
    pub outbound: Option<Value>,
    pub message_content_hashes: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PushPlanSkippedItemResult {
    pub push_id: String,
    pub queue_item_hash: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PushPlanResult {
    pub schema_name: String,
    pub schema_version: u32,
    pub confirmed: bool,
    pub workspace_uid: String,
    pub workspace_path: String,
    pub plan_id: String,
    pub config_effect_id: String,
    pub count: usize,
    pub items: Vec<PushPlanItemResult>,
    pub skipped_count: usize,
    pub skipped_items: Vec<PushPlanSkippedItemResult>,
    pub hint: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PushSucceededItemResult {
    pub push_id: String,
    pub kind: String,
    pub display_kind: String,
    pub case_uid: Option<String>,
    pub draft_name: Option<String>,
    pub action: Option<String>,
    pub message_ids: Vec<String>,
    pub succeeded_step_count: usize,
    pub audit_pending: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct PushConfirmResult {
    pub confirmed: bool,
    pub plan_id: String,
    pub workspace_uid: String,
    pub workspace_path: String,
    pub pushed_count: usize,
    pub failed_count: usize,
    pub partially_applied_count: usize,
    pub audit_pending_count: usize,
    pub stopped_after_failure: bool,
    pub remaining_count: usize,
    pub succeeded_items: Vec<PushSucceededItemResult>,
    pub failures: Vec<Value>,
    pub audit_warnings: Vec<Value>,
}
