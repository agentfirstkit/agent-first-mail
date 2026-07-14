use crate::config::ActionStep;
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PushKind {
    Draft,
    Message,
}

impl PushKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "draft" => Ok(Self::Draft),
            "message" => Ok(Self::Message),
            other => Err(AppError::new(
                "push_item_invalid",
                format!("unsupported push item kind: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Message => "message",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PushItem {
    pub schema_name: String,
    pub schema_version: u64,
    pub push_id: String,
    pub enqueue_seq: u64,
    #[serde(flatten)]
    pub payload: PushPayload,
    #[serde(skip)]
    pub planned_outbound: Option<OutboundPush>,
    #[serde(skip)]
    pub planned_outbound_message: Option<PreparedOutbound>,
    #[serde(skip)]
    pub planned_message_action: Option<MessageActionPush>,
    #[serde(skip)]
    pub planned_message_rfc822: BTreeMap<String, Vec<u8>>,
    pub created_rfc3339: String,
    pub updated_rfc3339: String,
    #[serde(default)]
    pub attempt_count: u64,
    pub step_states: Vec<PushStepState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_stage: Option<PushLocalStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_pending: Option<PushAuditPending>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PushStepState {
    pub index: usize,
    pub label: String,
    pub status: PushStepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_rfc3339: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_rfc3339: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PushStepStatus {
    Pending,
    Succeeded,
    Failed,
    RemoteOutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PushLocalStage {
    RemoteSucceeded,
    OutboundRecorded,
    DraftRemoved,
    PendingCleared,
    AuditPending,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PushAuditPending {
    pub event_id: String,
    pub created_rfc3339: String,
    pub push_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_ids: Vec<String>,
    pub succeeded_step_count: usize,
    pub attempt_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl PushItem {
    pub fn parse_json(data: &str) -> Result<Self> {
        let value: Value =
            serde_json::from_str(data).map_err(|e| AppError::json("parse push item", &e))?;
        validate_push_item_keys(&value)?;
        serde_json::from_value(value).map_err(|e| AppError::json("parse push item", &e))
    }

    pub fn push_kind(&self) -> PushKind {
        match &self.payload {
            PushPayload::Draft(_) => PushKind::Draft,
            PushPayload::Message(_) => PushKind::Message,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.push_kind().as_str()
    }

    pub fn display_kind(&self) -> String {
        if let Some(outbound) = &self.planned_outbound {
            return outbound.action.as_str().to_string();
        }
        if let Some(action) = &self.planned_message_action {
            return action.action.kind().to_string();
        }
        match &self.payload {
            PushPayload::Draft(_) => "draft".to_string(),
            PushPayload::Message(_) => "message".to_string(),
        }
    }

    pub fn draft(&self) -> Option<&DraftPush> {
        match &self.payload {
            PushPayload::Draft(draft) => Some(draft.as_ref()),
            PushPayload::Message(_) => None,
        }
    }

    pub fn draft_mut(&mut self) -> Option<&mut DraftPush> {
        match &mut self.payload {
            PushPayload::Draft(draft) => Some(draft.as_mut()),
            PushPayload::Message(_) => None,
        }
    }

    pub fn message(&self) -> Option<&MessagePush> {
        match &self.payload {
            PushPayload::Message(message) => Some(message),
            PushPayload::Draft(_) => None,
        }
    }

    pub fn outbound(&self) -> Option<&OutboundPush> {
        self.planned_outbound.as_ref()
    }

    pub fn message_action(&self) -> Option<&MessageActionPush> {
        self.planned_message_action.as_ref()
    }

    pub fn message_ids(&self) -> &[String] {
        if let Some(action) = self.message_action() {
            return action.message_ids.as_slice();
        }
        match &self.payload {
            PushPayload::Message(message) => std::slice::from_ref(&message.message_id),
            PushPayload::Draft(_) => &[],
        }
    }

    pub fn locations(&self) -> &[PushLocation] {
        self.message_action()
            .map(|action| action.locations.as_slice())
            .unwrap_or(&[])
    }

    pub fn steps(&self) -> &[ActionStep] {
        self.message_action()
            .map(|action| action.steps.as_slice())
            .unwrap_or(&[])
    }

    pub fn reply_to_message_id(&self) -> Option<&str> {
        self.message_action()
            .and_then(|action| action.reply_to_message_id.as_deref())
            .or_else(|| {
                self.outbound()
                    .and_then(|outbound| outbound.reply_to_message_id.as_deref())
            })
    }

    pub fn succeeded_step_count(&self) -> usize {
        let mut completed = 0usize;
        for state in &self.step_states {
            if state.index == completed && state.status == PushStepStatus::Succeeded {
                completed += 1;
            }
        }
        completed
    }

    pub fn has_started_steps(&self) -> bool {
        self.step_states.iter().any(|state| {
            state.status != PushStepStatus::Pending
                || state.started_rfc3339.is_some()
                || state.completed_rfc3339.is_some()
        })
    }
}

fn validate_push_item_keys(value: &Value) -> Result<()> {
    let Some(obj) = value.as_object() else {
        return Err(AppError::new(
            "push_item_invalid",
            "push item must be an object",
        ));
    };
    let Some(raw_kind) = obj.get("kind").and_then(Value::as_str) else {
        return Err(AppError::new(
            "push_item_invalid",
            "push item requires kind",
        ));
    };
    if obj.get("schema_name").and_then(Value::as_str) != Some("push_item")
        || obj.get("schema_version").and_then(Value::as_u64) != Some(1)
    {
        return Err(AppError::new(
            "push_item_invalid",
            "push item requires schema_name push_item and schema_version 1",
        ));
    }
    let kind = PushKind::parse(raw_kind)?.as_str();
    let common = [
        "schema_name",
        "schema_version",
        "push_id",
        "enqueue_seq",
        "kind",
        "created_rfc3339",
        "updated_rfc3339",
        "attempt_count",
        "step_states",
        "local_stage",
        "audit_pending",
        "last_error",
    ];
    for key in obj.keys() {
        if !common.contains(&key.as_str()) && !push_payload_key_allowed(kind, key) {
            return Err(AppError::new(
                "push_item_invalid",
                format!("unsupported push item field: {key}"),
            ));
        }
    }
    Ok(())
}

fn push_payload_key_allowed(kind: &str, key: &str) -> bool {
    match kind {
        "draft" => matches!(
            key,
            "case_uid" | "draft_name" | "draft_uid_validity" | "draft_uid"
        ),
        "message" => key == "message_id",
        _ => false,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushPayload {
    Draft(Box<DraftPush>),
    Message(MessagePush),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboundAction {
    SaveDraft,
    Send,
}

impl OutboundAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SaveDraft => "save_draft",
            Self::Send => "send",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DraftPush {
    pub case_uid: String,
    pub draft_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_uid_validity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_uid: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessagePush {
    pub message_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundPush {
    pub action: OutboundAction,
    pub case_uid: String,
    pub draft_name: String,
    pub draft_uid_validity: Option<u64>,
    pub draft_uid: Option<u64>,
    pub steps: Vec<ActionStep>,
    pub reply_to_message_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedOutbound {
    pub message_id: String,
    pub raw: Vec<u8>,
    pub envelope_from: String,
    pub envelope_to: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageActionPush {
    pub action: MessagePushAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<PushLocation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ActionStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessagePushAction {
    CaseAdd,
    Notification,
    Spam,
    Trash,
    Unspam,
    Untrash,
}

impl MessagePushAction {
    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "case.add" | "case_add" => Some(Self::CaseAdd),
            "message.notification" | "notification" => Some(Self::Notification),
            "message.spam" | "spam" => Some(Self::Spam),
            "message.trash" | "trash" => Some(Self::Trash),
            "message.unspam" | "unspam" => Some(Self::Unspam),
            "message.untrash" | "untrash" => Some(Self::Untrash),
            _ => None,
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::CaseAdd => "case.add",
            Self::Notification => "message.notification",
            Self::Spam => "message.spam",
            Self::Trash => "message.trash",
            Self::Unspam => "message.unspam",
            Self::Untrash => "message.untrash",
        }
    }

    pub fn mode_label(self) -> &'static str {
        match self {
            Self::CaseAdd => "case",
            Self::Notification => "notification",
            Self::Spam => "spam",
            Self::Trash => "trash",
            Self::Unspam => "unspam",
            Self::Untrash => "untrash",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PushLocation {
    pub message_id: String,
    pub mailbox_name: String,
    pub uid_validity: u64,
    pub uid: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_item_requires_step_states() {
        let data = r#"{
          "schema_name": "push_item",
          "schema_version": 1,
          "push_id": "push_20260609T000000Z",
          "enqueue_seq": 1,
          "kind": "message",
          "message_id": "message_1",
          "created_rfc3339": "2026-06-09T00:00:00Z",
          "updated_rfc3339": "2026-06-09T00:00:00Z",
          "attempt_count": 1
        }"#;
        let err = PushItem::parse_json(data)
            .err()
            .unwrap_or_else(|| AppError::new("test_failure", "expected missing steps to fail"));
        assert_eq!(err.error_code, "store_error");
    }

    #[test]
    fn invalid_push_kind_is_rejected() {
        let data = r#"{
          "schema_name": "push_item",
          "schema_version": 1,
          "push_id": "push_bad",
          "kind": "surprise",
          "created_rfc3339": "2026-06-09T00:00:00Z",
          "updated_rfc3339": "2026-06-09T00:00:00Z"
        }"#;
        let err = PushItem::parse_json(data)
            .err()
            .unwrap_or_else(|| AppError::new("test_failure", "expected invalid kind to fail"));
        assert_eq!(err.error_code, "push_item_invalid");
    }
}
