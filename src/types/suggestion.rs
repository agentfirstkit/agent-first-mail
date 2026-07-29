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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanSuggestionOutcome {
    Accepted,
    Changed,
    Rejected,
    Deferred,
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageHumanSuggestion {
    pub human_suggestion_uid: String,
    pub based_on_agent_suggestion_uid: String,
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseHumanSuggestion {
    pub human_suggestion_uid: String,
    pub based_on_agent_suggestion_uid: String,
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotificationHumanSuggestion {
    pub human_suggestion_uid: String,
    pub based_on_agent_suggestion_uid: String,
    pub notification_review_base_hash: String,
    pub outcome: HumanSuggestionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<NotificationSuggestedAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_rfc3339: String,
}
