use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessageSuggestedAction {
    CaseCreate {
        case_name: String,
        group: String,
    },
    CaseAdd {
        case_ref: String,
    },
    NotificationCreate {
        notification_name: String,
        summary: String,
    },
    NotificationAdd {
        notification_ref: String,
        summary: String,
    },
    Spam,
    Trash,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaseSuggestedAction {
    CaseArchive,
    CaseUnarchive { group: String },
    CaseMove { group: String },
    CaseRename { case_name: String },
    CaseTag { tag: String },
    CaseUntag { tag: String },
    CaseTrash,
    CaseDelete { allow_notes: bool },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationSuggestedAction {
    NotificationRename { notification_name: String },
    NotificationDelete { allow_notes: bool },
}

/// A person's disposition of one Agent-suggestible item.
///
/// `Accepted` and `Changed` answer a proposal the Agent made; `Rejected` says
/// no (with a reason, whether or not there was a proposal to say no to);
/// `Decided` is the person originating a disposition the Agent never
/// suggested. There is no "deferred" or "submitted" value: silence about an
/// item just means it has not been settled yet, and a person can settle it,
/// and resettle it, at any time. See [`MessageHumanSuggestion`] for the
/// invariants each value carries.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanSuggestionOutcome {
    Accepted,
    Changed,
    Rejected,
    Decided,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageAgentSuggestion {
    pub agent_suggestion_uid: String,
    pub message_review_base_hash: String,
    pub suggested_action: MessageSuggestedAction,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_percent: Option<u8>,
    pub created_rfc3339: String,
}

/// A person's settled disposition of one message.
///
/// `based_on_agent_suggestion_uid` is `None` when the person originated this
/// disposition themselves rather than answering a proposal — the only case
/// where that is legal is `outcome: Decided`. Three invariants hold for every
/// value that reaches disk, checked right after deserialization rather than
/// left to each writer to remember:
///
/// 1. `Decided` ⇒ `based_on_agent_suggestion_uid` is `None` and
///    `suggested_action` is `Some`;
/// 2. `Accepted` / `Changed` ⇒ `based_on_agent_suggestion_uid` is `Some`;
/// 3. `Rejected` ⇒ `reason` is `Some`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageHumanSuggestion {
    pub human_suggestion_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on_agent_suggestion_uid: Option<String>,
    pub message_review_base_hash: String,
    pub outcome: HumanSuggestionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<MessageSuggestedAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseAgentSuggestion {
    pub agent_suggestion_uid: String,
    pub case_review_base_hash: String,
    pub suggested_action: CaseSuggestedAction,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_percent: Option<u8>,
    pub created_rfc3339: String,
}

/// A person's settled disposition of one case. Same invariants as
/// [`MessageHumanSuggestion`].
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseHumanSuggestion {
    pub human_suggestion_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on_agent_suggestion_uid: Option<String>,
    pub case_review_base_hash: String,
    pub outcome: HumanSuggestionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<CaseSuggestedAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotificationAgentSuggestion {
    pub agent_suggestion_uid: String,
    pub notification_review_base_hash: String,
    pub suggested_action: NotificationSuggestedAction,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_percent: Option<u8>,
    pub created_rfc3339: String,
}

/// A person's settled disposition of one notification collection. Same
/// invariants as [`MessageHumanSuggestion`].
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotificationHumanSuggestion {
    pub human_suggestion_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on_agent_suggestion_uid: Option<String>,
    pub notification_review_base_hash: String,
    pub outcome: HumanSuggestionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<NotificationSuggestedAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_rfc3339: String,
}
