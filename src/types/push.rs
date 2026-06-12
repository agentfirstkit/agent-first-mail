use crate::config::ActionStep;
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PushKind {
    Outbound,
    MessageAction,
}

impl PushKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "outbound" => Ok(Self::Outbound),
            "message_action" => Ok(Self::MessageAction),
            other => Err(AppError::new(
                "push_item_invalid",
                format!("unsupported push item kind: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::MessageAction => "message_action",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PushItem {
    pub schema_name: String,
    pub schema_version: u64,
    pub push_id: String,
    #[serde(flatten)]
    pub payload: PushPayload,
    pub created_rfc3339: String,
    pub updated_rfc3339: String,
    #[serde(default)]
    pub attempt_count: u64,
    pub step_states: Vec<PushStepState>,
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
            PushPayload::Outbound(_) => PushKind::Outbound,
            PushPayload::MessageAction(_) => PushKind::MessageAction,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.push_kind().as_str()
    }

    pub fn display_kind(&self) -> String {
        match &self.payload {
            PushPayload::Outbound(_) => "outbound".to_string(),
            PushPayload::MessageAction(action) => action.action.kind().to_string(),
        }
    }

    pub fn outbound(&self) -> Option<&OutboundPush> {
        match &self.payload {
            PushPayload::Outbound(outbound) => Some(outbound.as_ref()),
            PushPayload::MessageAction(_) => None,
        }
    }

    pub fn outbound_mut(&mut self) -> Option<&mut OutboundPush> {
        match &mut self.payload {
            PushPayload::Outbound(outbound) => Some(outbound.as_mut()),
            PushPayload::MessageAction(_) => None,
        }
    }

    pub fn message_action(&self) -> Option<&MessageActionPush> {
        match &self.payload {
            PushPayload::MessageAction(action) => Some(action),
            PushPayload::Outbound(_) => None,
        }
    }

    pub fn message_action_mut(&mut self) -> Option<&mut MessageActionPush> {
        match &mut self.payload {
            PushPayload::MessageAction(action) => Some(action),
            PushPayload::Outbound(_) => None,
        }
    }

    pub fn message_ids(&self) -> &[String] {
        self.message_action()
            .map(|action| action.message_ids.as_slice())
            .unwrap_or(&[])
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
        match &self.payload {
            PushPayload::Outbound(outbound) => outbound.reply_to_message_id.as_deref(),
            PushPayload::MessageAction(action) => action.reply_to_message_id.as_deref(),
        }
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
        self.step_states
            .iter()
            .any(|state| state.status != PushStepStatus::Pending)
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
        "kind",
        "created_rfc3339",
        "updated_rfc3339",
        "attempt_count",
        "step_states",
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
        "outbound" => matches!(
            key,
            "case_uid"
                | "draft_name"
                | "draft_hash"
                | "message_id"
                | "reply_to_message_id"
                | "eml_path"
                | "envelope_from"
                | "envelope_to"
                | "drafts_mailbox_name"
                | "sent_mailbox_name"
                | "draft_uid_validity"
                | "draft_uid"
                | "draft_save_steps"
                | "draft_send_steps"
        ),
        "message_action" => matches!(
            key,
            "action" | "message_ids" | "locations" | "steps" | "reply_to_message_id"
        ),
        _ => false,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushPayload {
    Outbound(Box<OutboundPush>),
    MessageAction(MessageActionPush),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutboundPush {
    pub case_uid: String,
    pub draft_name: String,
    pub draft_hash: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    pub eml_path: String,
    pub envelope_from: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelope_to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drafts_mailbox_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_mailbox_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_uid_validity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_uid: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draft_save_steps: Vec<ActionStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draft_send_steps: Vec<ActionStep>,
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
    Archive,
    Spam,
    Trash,
}

impl MessagePushAction {
    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "case.add" | "case_add" => Some(Self::CaseAdd),
            "message.archive" | "archive" => Some(Self::Archive),
            "message.spam" | "spam" => Some(Self::Spam),
            "message.trash" | "trash" => Some(Self::Trash),
            _ => None,
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::CaseAdd => "case.add",
            Self::Archive => "message.archive",
            Self::Spam => "message.spam",
            Self::Trash => "message.trash",
        }
    }

    pub fn mode_label(self) -> &'static str {
        match self {
            Self::CaseAdd => "case",
            Self::Archive => "archive",
            Self::Spam => "spam",
            Self::Trash => "trash",
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
          "kind": "message_action",
          "action": "spam",
          "message_ids": ["message_1"],
          "locations": [],
          "steps": [],
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
