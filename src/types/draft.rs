use super::{OutboundAction, PushItem, PushStepStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only remote intent that may be recorded in draft frontmatter.
///
/// This is deliberately distinct from a completed remote outcome: both values
/// mean that work is queued locally until a push succeeds.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftSyncIntent {
    SaveDraft,
    Send,
}

impl DraftSyncIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SaveDraft => "save_draft",
            Self::Send => "send",
        }
    }
}

impl From<OutboundAction> for DraftSyncIntent {
    fn from(value: OutboundAction) -> Self {
        match value {
            OutboundAction::SaveDraft => Self::SaveDraft,
            OutboundAction::Send => Self::Send,
        }
    }
}

impl From<DraftSyncIntent> for OutboundAction {
    fn from(value: DraftSyncIntent) -> Self {
        match value {
            DraftSyncIntent::SaveDraft => Self::SaveDraft,
            DraftSyncIntent::Send => Self::Send,
        }
    }
}

/// Whether the typed compose section contains agent/user-authored text.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftCompleteness {
    Complete,
    Incomplete,
}

/// Observable state of a local draft. A successful send consumes the draft, so
/// there is intentionally no `sent` variant here.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    Editable,
    QueuedUpload,
    QueuedSend,
    Pushing,
    Failed,
}

/// A broken relationship between draft frontmatter and the durable push queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftProjectionIssue {
    IntentWithoutPush,
    PushWithoutIntent,
}

impl DraftProjectionIssue {
    pub fn message(self) -> &'static str {
        match self {
            Self::IntentWithoutPush => "draft has sync_intent but no matching outbound push item",
            Self::PushWithoutIntent => "draft has an outbound push item but no sync_intent",
        }
    }
}

/// One canonical operational-state projection shared by CLI output and Review
/// documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftStateProjection {
    pub status: DraftStatus,
    pub queued_action: Option<DraftSyncIntent>,
    pub issue: Option<DraftProjectionIssue>,
}

impl DraftStateProjection {
    pub fn from_queue(intent: Option<DraftSyncIntent>, queued: Option<&PushItem>) -> Self {
        match (intent, queued) {
            (None, None) => Self {
                status: DraftStatus::Editable,
                queued_action: None,
                issue: None,
            },
            (Some(_), None) => Self {
                status: DraftStatus::Failed,
                queued_action: None,
                issue: Some(DraftProjectionIssue::IntentWithoutPush),
            },
            (None, Some(_)) => Self {
                status: DraftStatus::Failed,
                queued_action: None,
                issue: Some(DraftProjectionIssue::PushWithoutIntent),
            },
            (Some(intent), Some(item)) => Self {
                status: status_for_queued_item(intent, item),
                queued_action: Some(intent),
                issue: None,
            },
        }
    }
}

fn status_for_queued_item(intent: DraftSyncIntent, item: &PushItem) -> DraftStatus {
    let failed = item.last_error.is_some()
        || item.step_states.iter().any(|state| {
            matches!(
                state.status,
                PushStepStatus::Failed | PushStepStatus::RemoteOutcomeUnknown
            )
        });
    if failed {
        return DraftStatus::Failed;
    }
    if item.has_started_effects() {
        return DraftStatus::Pushing;
    }
    match intent {
        DraftSyncIntent::SaveDraft => DraftStatus::QueuedUpload,
        DraftSyncIntent::Send => DraftStatus::QueuedSend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DraftPush, PUSH_ITEM_SCHEMA_VERSION, PushPayload, PushStepState};

    fn push_item(status: Option<PushStepStatus>) -> PushItem {
        PushItem {
            schema_name: "push_item".to_string(),
            schema_version: PUSH_ITEM_SCHEMA_VERSION,
            push_id: "push_test".to_string(),
            enqueue_seq: 1,
            payload: PushPayload::Draft(Box::new(DraftPush {
                case_uid: "c20260711001".to_string(),
                draft_name: "reply.md".to_string(),
                draft_uid_validity: None,
                draft_uid: None,
            })),
            planned_outbound: None,
            planned_outbound_message: None,
            planned_message_action: None,
            planned_message_rfc822: std::collections::BTreeMap::new(),
            created_rfc3339: "2026-07-11T00:00:00Z".to_string(),
            updated_rfc3339: "2026-07-11T00:00:00Z".to_string(),
            attempt_count: 0,
            step_states: status
                .into_iter()
                .map(|status| PushStepState {
                    index: 0,
                    label: "smtp_send".to_string(),
                    status,
                    started_rfc3339: (status != PushStepStatus::Pending)
                        .then(|| "2026-07-11T00:00:01Z".to_string()),
                    completed_rfc3339: None,
                    result_summary: None,
                    error_code: None,
                    error: None,
                    retryable: None,
                })
                .collect(),
            local_stage: None,
            audit_pending: None,
            last_error: None,
        }
    }

    #[test]
    fn sync_intent_serializes_only_canonical_values() {
        assert_eq!(
            serde_json::to_string(&DraftSyncIntent::SaveDraft)
                .ok()
                .as_deref(),
            Some("\"save_draft\"")
        );
        assert!(serde_json::from_str::<DraftSyncIntent>("\"save\"").is_err());
    }

    #[test]
    fn queued_send_is_not_sent() {
        let item = push_item(None);
        let projected = DraftStateProjection::from_queue(Some(DraftSyncIntent::Send), Some(&item));
        assert_eq!(projected.status, DraftStatus::QueuedSend);
        assert_eq!(projected.queued_action, Some(DraftSyncIntent::Send));
    }

    #[test]
    fn started_and_failed_pushes_have_distinct_states() {
        let pushing = push_item(Some(PushStepStatus::Succeeded));
        assert_eq!(
            DraftStateProjection::from_queue(Some(DraftSyncIntent::SaveDraft), Some(&pushing))
                .status,
            DraftStatus::Pushing
        );

        let failed = push_item(Some(PushStepStatus::RemoteOutcomeUnknown));
        assert_eq!(
            DraftStateProjection::from_queue(Some(DraftSyncIntent::Send), Some(&failed)).status,
            DraftStatus::Failed
        );
    }

    #[test]
    fn intent_and_queue_must_exist_together() {
        let queued = push_item(None);
        assert_eq!(
            DraftStateProjection::from_queue(Some(DraftSyncIntent::Send), None).issue,
            Some(DraftProjectionIssue::IntentWithoutPush)
        );
        assert_eq!(
            DraftStateProjection::from_queue(None, Some(&queued)).issue,
            Some(DraftProjectionIssue::PushWithoutIntent)
        );
    }
}
