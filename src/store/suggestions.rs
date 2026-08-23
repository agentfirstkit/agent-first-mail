use super::*;
use crate::types::{
    CaseAgentSuggestion, CaseSuggestedAction, CollectionAgentSuggestion, CollectionHumanSuggestion,
    HumanSuggestionOutcome, MessageAgentSuggestion, MessageHumanSuggestion, MessageSuggestedAction,
    NotificationAgentSuggestion, NotificationSuggestedAction,
};
#[cfg(any(feature = "review", test))]
use crate::types::{CaseHumanSuggestion, NotificationHumanSuggestion};

#[derive(Serialize)]
#[serde(tag = "surface_kind", rename_all = "snake_case")]
enum CollectionReviewBaseDocument<'a> {
    Case {
        collection: &'a CollectionRenderDocument,
        notes: &'a NotesRenderDocument,
        drafts: &'a [DraftRenderDocument],
    },
    Notification {
        collection: &'a CollectionRenderDocument,
        notes: &'a NotesRenderDocument,
    },
}

pub(crate) fn case_review_base_hash_from_documents(
    collection: &CollectionRenderDocument,
    notes: &NotesRenderDocument,
    drafts: &[DraftRenderDocument],
) -> Result<String> {
    let mut collection = collection.clone();
    collection.collection.agent_suggestion = None;
    collection.collection.human_suggestion = None;
    review_document_hash(
        "case",
        &CollectionReviewBaseDocument::Case {
            collection: &collection,
            notes,
            drafts,
        },
    )
}

pub(crate) fn notification_review_base_hash_from_documents(
    collection: &CollectionRenderDocument,
    notes: &NotesRenderDocument,
) -> Result<String> {
    let mut collection = collection.clone();
    collection.collection.agent_suggestion = None;
    collection.collection.human_suggestion = None;
    review_document_hash(
        "notification",
        &CollectionReviewBaseDocument::Notification {
            collection: &collection,
            notes,
        },
    )
}

/// One item's settled disposition, as a review page's settle route receives it.
///
/// `based_on_agent_suggestion_uid` is `None` for `outcome: Decided` — the
/// person originated this disposition rather than answering a proposal — and
/// `Some` for `Accepted` / `Changed` / an agent-answering `Rejected`.
/// `message_review_base_hash` is always the message's own current hash at
/// settle time, whether or not an Agent suggestion is involved: it is what
/// catches a message that changed under the person between page-open and
/// settle.
#[derive(Clone, Debug)]
#[cfg(any(feature = "review", test))]
pub(crate) struct MessageHumanSuggestionInput {
    pub(crate) message_id: String,
    pub(crate) based_on_agent_suggestion_uid: Option<String>,
    pub(crate) message_review_base_hash: String,
    pub(crate) outcome: HumanSuggestionOutcome,
    pub(crate) suggested_action: Option<MessageSuggestedAction>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug)]
#[cfg(any(feature = "review", test))]
pub(crate) struct CaseHumanSuggestionInput {
    pub(crate) case_ref: String,
    pub(crate) based_on_agent_suggestion_uid: Option<String>,
    pub(crate) case_review_base_hash: String,
    pub(crate) outcome: HumanSuggestionOutcome,
    pub(crate) suggested_action: Option<CaseSuggestedAction>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug)]
#[cfg(any(feature = "review", test))]
pub(crate) struct NotificationHumanSuggestionInput {
    pub(crate) notification_ref: String,
    pub(crate) based_on_agent_suggestion_uid: Option<String>,
    pub(crate) notification_review_base_hash: String,
    pub(crate) outcome: HumanSuggestionOutcome,
    pub(crate) suggested_action: Option<NotificationSuggestedAction>,
    pub(crate) reason: Option<String>,
}

fn checked_suggestion_text<'a>(field: &str, value: &'a str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::new(
            "invalid_suggestion",
            format!("{field} must not be empty"),
        ));
    }
    Ok(value)
}

fn checked_confidence_percent(value: Option<u8>) -> Result<Option<u8>> {
    if value.is_some_and(|percent| percent > 100) {
        return Err(AppError::new(
            "invalid_suggestion",
            "confidence_percent must be between 0 and 100",
        ));
    }
    Ok(value)
}

fn suggestion_uid(kind: &str) -> String {
    new_event_id().replacen("event_", &format!("{kind}_"), 1)
}

fn validate_message_action(action: &MessageSuggestedAction) -> Result<()> {
    match action {
        MessageSuggestedAction::CaseCreate { case_name, group } => {
            validate_name(
                "case_name",
                checked_suggestion_text("case_name", case_name)?,
            )?;
            validate_id("group", checked_suggestion_text("group", group)?)
        }
        MessageSuggestedAction::CaseAdd { case_ref } => {
            checked_suggestion_text("case_ref", case_ref)?;
            Ok(())
        }
        MessageSuggestedAction::NotificationCreate {
            notification_name,
            summary,
        } => {
            validate_name(
                "notification_name",
                checked_suggestion_text("notification_name", notification_name)?,
            )?;
            checked_suggestion_text("summary", summary)?;
            Ok(())
        }
        MessageSuggestedAction::NotificationAdd {
            notification_ref,
            summary,
        } => {
            checked_suggestion_text("notification_ref", notification_ref)?;
            checked_suggestion_text("summary", summary)?;
            Ok(())
        }
        MessageSuggestedAction::Spam | MessageSuggestedAction::Trash => Ok(()),
    }
}

fn validate_case_action(action: &CaseSuggestedAction) -> Result<()> {
    match action {
        CaseSuggestedAction::CaseUnarchive { group } | CaseSuggestedAction::CaseMove { group } => {
            validate_id("group", checked_suggestion_text("group", group)?)
        }
        CaseSuggestedAction::CaseRename { case_name } => validate_name(
            "case_name",
            checked_suggestion_text("case_name", case_name)?,
        ),
        CaseSuggestedAction::CaseTag { tag } | CaseSuggestedAction::CaseUntag { tag } => {
            validate_id("tag", checked_suggestion_text("tag", tag)?)
        }
        CaseSuggestedAction::CaseArchive
        | CaseSuggestedAction::CaseTrash
        | CaseSuggestedAction::CaseDelete { .. } => Ok(()),
    }
}

fn validate_notification_action(action: &NotificationSuggestedAction) -> Result<()> {
    match action {
        NotificationSuggestedAction::NotificationRename { notification_name } => validate_name(
            "notification_name",
            checked_suggestion_text("notification_name", notification_name)?,
        ),
        NotificationSuggestedAction::NotificationDelete { .. } => Ok(()),
    }
}

fn validate_agent_fields(
    uid: &str,
    review_base_hash: &str,
    reason: &str,
    confidence_percent: Option<u8>,
    created_rfc3339: &str,
) -> Result<()> {
    checked_suggestion_text("agent_suggestion_uid", uid)?;
    checked_suggestion_text("review_base_hash", review_base_hash)?;
    checked_suggestion_text("reason", reason)?;
    checked_confidence_percent(confidence_percent)?;
    checked_suggestion_text("created_rfc3339", created_rfc3339)?;
    Ok(())
}

/// Shape-only checks on a [`crate::types::MessageHumanSuggestion`] (and its
/// case/notification equivalents): no field is blank, and — when present —
/// `based_on_agent_suggestion_uid` is not blank either. Whether it is allowed
/// to be present at all is an outcome invariant, checked separately by
/// `validate_human_*_fields`, because that check needs to know which agent
/// suggestion (if any) is current.
fn validate_human_fields(
    uid: &str,
    based_on_agent_suggestion_uid: Option<&str>,
    review_base_hash: &str,
    created_rfc3339: &str,
) -> Result<()> {
    checked_suggestion_text("human_suggestion_uid", uid)?;
    if let Some(based_on_uid) = based_on_agent_suggestion_uid {
        checked_suggestion_text("based_on_agent_suggestion_uid", based_on_uid)?;
    }
    checked_suggestion_text("review_base_hash", review_base_hash)?;
    checked_suggestion_text("created_rfc3339", created_rfc3339)?;
    Ok(())
}

pub(super) fn validate_message_suggestion_pair(
    agent: Option<&MessageAgentSuggestion>,
    human: Option<&MessageHumanSuggestion>,
) -> Result<()> {
    if let Some(agent) = agent {
        validate_agent_fields(
            &agent.agent_suggestion_uid,
            &agent.message_review_base_hash,
            &agent.reason,
            agent.confidence_percent,
            &agent.created_rfc3339,
        )?;
        validate_message_action(&agent.suggested_action)?;
    }
    let Some(human) = human else {
        return Ok(());
    };
    validate_human_fields(
        &human.human_suggestion_uid,
        human.based_on_agent_suggestion_uid.as_deref(),
        &human.message_review_base_hash,
        &human.created_rfc3339,
    )?;
    let agent_action = bind_human_to_agent(
        human.based_on_agent_suggestion_uid.as_deref(),
        &human.message_review_base_hash,
        agent.map(|agent| {
            (
                agent.agent_suggestion_uid.as_str(),
                agent.message_review_base_hash.as_str(),
                &agent.suggested_action,
            )
        }),
    )?;
    validate_human_message_fields(
        human.outcome,
        human.based_on_agent_suggestion_uid.as_deref(),
        human.suggested_action.as_ref(),
        human.reason.as_deref(),
        agent_action,
    )
}

pub(super) fn validate_case_suggestion_pair(
    agent: Option<&CollectionAgentSuggestion>,
    human: Option<&CollectionHumanSuggestion>,
) -> Result<()> {
    let agent = match agent {
        Some(CollectionAgentSuggestion::Case(agent)) => Some(agent),
        Some(CollectionAgentSuggestion::Notification(_)) => {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "case metadata contains a notification agent_suggestion",
            ));
        }
        None => None,
    };
    let human = match human {
        Some(CollectionHumanSuggestion::Case(human)) => Some(human),
        Some(CollectionHumanSuggestion::Notification(_)) => {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "case metadata contains a notification human_suggestion",
            ));
        }
        None => None,
    };
    if let Some(agent) = agent {
        validate_agent_fields(
            &agent.agent_suggestion_uid,
            &agent.case_review_base_hash,
            &agent.reason,
            agent.confidence_percent,
            &agent.created_rfc3339,
        )?;
        validate_case_action(&agent.suggested_action)?;
    }
    let Some(human) = human else {
        return Ok(());
    };
    validate_human_fields(
        &human.human_suggestion_uid,
        human.based_on_agent_suggestion_uid.as_deref(),
        &human.case_review_base_hash,
        &human.created_rfc3339,
    )?;
    let agent_action = bind_human_to_agent(
        human.based_on_agent_suggestion_uid.as_deref(),
        &human.case_review_base_hash,
        agent.map(|agent| {
            (
                agent.agent_suggestion_uid.as_str(),
                agent.case_review_base_hash.as_str(),
                &agent.suggested_action,
            )
        }),
    )?;
    validate_human_case_fields(
        human.outcome,
        human.based_on_agent_suggestion_uid.as_deref(),
        human.suggested_action.as_ref(),
        human.reason.as_deref(),
        agent_action,
    )
}

pub(super) fn validate_notification_suggestion_pair(
    agent: Option<&CollectionAgentSuggestion>,
    human: Option<&CollectionHumanSuggestion>,
) -> Result<()> {
    let agent = match agent {
        Some(CollectionAgentSuggestion::Notification(agent)) => Some(agent),
        Some(CollectionAgentSuggestion::Case(_)) => {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "notification metadata contains a case agent_suggestion",
            ));
        }
        None => None,
    };
    let human = match human {
        Some(CollectionHumanSuggestion::Notification(human)) => Some(human),
        Some(CollectionHumanSuggestion::Case(_)) => {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "notification metadata contains a case human_suggestion",
            ));
        }
        None => None,
    };
    if let Some(agent) = agent {
        validate_agent_fields(
            &agent.agent_suggestion_uid,
            &agent.notification_review_base_hash,
            &agent.reason,
            agent.confidence_percent,
            &agent.created_rfc3339,
        )?;
        validate_notification_action(&agent.suggested_action)?;
    }
    let Some(human) = human else {
        return Ok(());
    };
    validate_human_fields(
        &human.human_suggestion_uid,
        human.based_on_agent_suggestion_uid.as_deref(),
        &human.notification_review_base_hash,
        &human.created_rfc3339,
    )?;
    let agent_action = bind_human_to_agent(
        human.based_on_agent_suggestion_uid.as_deref(),
        &human.notification_review_base_hash,
        agent.map(|agent| {
            (
                agent.agent_suggestion_uid.as_str(),
                agent.notification_review_base_hash.as_str(),
                &agent.suggested_action,
            )
        }),
    )?;
    validate_human_notification_fields(
        human.outcome,
        human.based_on_agent_suggestion_uid.as_deref(),
        human.suggested_action.as_ref(),
        human.reason.as_deref(),
        agent_action,
    )
}

/// Resolve which agent action (if any) a human_suggestion is answering.
///
/// `None` (no `based_on_agent_suggestion_uid`) always resolves to `None` —
/// that is `Decided`, or a `Rejected` that answers nothing in particular.
/// `Some` must resolve to the exact current agent suggestion; a `based_on`
/// that names an agent suggestion which is missing, replaced, or was computed
/// against a different review hash is a dangling reference, not a stale one —
/// afmail always clears `human_suggestion` when it replaces or clears
/// `agent_suggestion`, so this can only happen to a hand-edited or corrupted
/// file.
fn bind_human_to_agent<'a, T>(
    based_on_agent_suggestion_uid: Option<&str>,
    human_review_base_hash: &str,
    agent: Option<(&str, &str, &'a T)>,
) -> Result<Option<&'a T>> {
    match (based_on_agent_suggestion_uid, agent) {
        (None, _) => Ok(None),
        (Some(based_on_uid), Some((agent_uid, agent_review_base_hash, agent_action))) => {
            if based_on_uid != agent_uid || human_review_base_hash != agent_review_base_hash {
                return Err(AppError::new(
                    "invalid_suggestion_state",
                    "human_suggestion is not bound to the current agent_suggestion",
                ));
            }
            Ok(Some(agent_action))
        }
        (Some(_), None) => Err(AppError::new(
            "invalid_suggestion_state",
            "human_suggestion references an agent_suggestion that is no longer current",
        )),
    }
}

impl Workspace {
    pub fn set_message_agent_suggestion(
        &self,
        message_id: &str,
        message_review_base_hash: &str,
        action: MessageSuggestedAction,
        reason: &str,
        confidence_percent: Option<u8>,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        validate_message_action(&action)?;
        let reason = checked_suggestion_text("reason", reason)?;
        let confidence_percent = checked_confidence_percent(confidence_percent)?;
        let message = self.read_message_by_id(message_id)?;
        if message.workspace.status != MessageStatus::Triage.as_str() {
            return Err(AppError::new(
                "message_not_in_triage",
                format!("message {message_id} is not in triage"),
            ));
        }
        let current_hash = self.message_review_base_hash(message_id)?;
        if current_hash != message_review_base_hash {
            return Err(stale_suggestion_error(
                "message",
                message_id,
                message_review_base_hash,
                &current_hash,
            ));
        }
        let mut metadata =
            read_message_metadata_file(&self.root, message_id)?.ok_or_else(|| {
                AppError::new(
                    "message_metadata_missing",
                    format!("canonical metadata is missing for message {message_id}"),
                )
            })?;
        let suggestion = MessageAgentSuggestion {
            agent_suggestion_uid: suggestion_uid("agent_suggestion"),
            message_review_base_hash: current_hash,
            suggested_action: action,
            reason: reason.to_string(),
            confidence_percent,
            created_rfc3339: now_rfc3339(),
        };
        metadata.agent_suggestion = Some(suggestion.clone());
        metadata.human_suggestion = None;
        write_message_metadata_file(&self.root, &metadata)?;
        self.refresh_message_projection(message_id)?;
        self.append_audit_event(
            "agent_suggestion_set",
            vec![audit_target("message", message_id)],
            Some(reason),
            json!({
                "agent_suggestion_uid": suggestion.agent_suggestion_uid,
                "message_review_base_hash": suggestion.message_review_base_hash,
                "suggested_action": suggestion.suggested_action,
                "confidence_percent": suggestion.confidence_percent,
            }),
        )?;
        Ok(json!({
            "message_id": message_id,
            "agent_suggestion": suggestion,
            "human_suggestion_cleared": true,
        }))
    }

    pub fn clear_message_suggestion(
        &self,
        message_id: &str,
        agent_suggestion_uid: &str,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let mut metadata =
            read_message_metadata_file(&self.root, message_id)?.ok_or_else(|| {
                AppError::new(
                    "message_metadata_missing",
                    format!("canonical metadata is missing for message {message_id}"),
                )
            })?;
        let current_uid = metadata
            .agent_suggestion
            .as_ref()
            .map(|value| value.agent_suggestion_uid.as_str())
            .ok_or_else(|| {
                AppError::new("suggestion_not_found", "message has no agent_suggestion")
            })?;
        if current_uid != agent_suggestion_uid {
            return Err(stale_suggestion_uid_error(
                current_uid,
                agent_suggestion_uid,
            ));
        }
        metadata.agent_suggestion = None;
        metadata.human_suggestion = None;
        write_message_metadata_file(&self.root, &metadata)?;
        self.refresh_message_projection(message_id)?;
        self.append_audit_event(
            "suggestion_cleared",
            vec![audit_target("message", message_id)],
            None,
            json!({"agent_suggestion_uid": agent_suggestion_uid}),
        )?;
        Ok(json!({
            "message_id": message_id,
            "agent_suggestion_uid": agent_suggestion_uid,
            "suggestion_cleared": true,
        }))
    }

    pub fn message_agent_suggestion(&self, message_id: &str) -> Result<MessageAgentSuggestion> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let message = self.read_message_by_id(message_id)?;
        if message.workspace.status != MessageStatus::Triage.as_str() {
            return Err(AppError::new(
                "message_not_in_triage",
                format!("message {message_id} is not in triage"),
            ));
        }
        read_message_metadata_file(&self.root, message_id)?
            .and_then(|value| value.agent_suggestion)
            .ok_or_else(|| {
                AppError::new(
                    "suggestion_not_found",
                    format!("message {message_id} has no agent_suggestion"),
                )
            })
    }

    /// One item's settled disposition, written the moment a person settles it.
    ///
    /// This writes exactly the one message named by `input`; it never touches
    /// any other message, and a caller settling several messages in one
    /// review session calls this once per message. A stale
    /// `message_review_base_hash` — the message changed since the page was
    /// rendered, or since the referenced `agent_suggestion` was proposed —
    /// fails this one call without touching the file at all.
    #[cfg(any(feature = "review", test))]
    pub(crate) fn set_message_human_suggestion(
        &self,
        input: &MessageHumanSuggestionInput,
    ) -> Result<MessageHumanSuggestion> {
        self.require_workspace()?;
        validate_id("message_id", &input.message_id)?;
        let mut metadata =
            read_message_metadata_file(&self.root, &input.message_id)?.ok_or_else(|| {
                AppError::new(
                    "message_metadata_missing",
                    format!(
                        "canonical metadata is missing for message {}",
                        input.message_id
                    ),
                )
            })?;
        let current_hash = self.message_review_base_hash(&input.message_id)?;
        if current_hash != input.message_review_base_hash {
            return Err(stale_suggestion_error(
                "message",
                &input.message_id,
                &input.message_review_base_hash,
                &current_hash,
            ));
        }
        let agent_action = match input.based_on_agent_suggestion_uid.as_deref() {
            Some(based_on_uid) => {
                let agent = metadata.agent_suggestion.as_ref().ok_or_else(|| {
                    AppError::new(
                        "suggestion_not_found",
                        format!("message {} has no agent_suggestion", input.message_id),
                    )
                })?;
                if agent.agent_suggestion_uid != based_on_uid
                    || agent.message_review_base_hash != input.message_review_base_hash
                {
                    return Err(stale_suggestion_error(
                        "message",
                        &input.message_id,
                        &input.message_review_base_hash,
                        &current_hash,
                    ));
                }
                Some(&agent.suggested_action)
            }
            None => None,
        };
        validate_human_message_fields(
            input.outcome,
            input.based_on_agent_suggestion_uid.as_deref(),
            input.suggested_action.as_ref(),
            input.reason.as_deref(),
            agent_action,
        )?;
        let human = MessageHumanSuggestion {
            human_suggestion_uid: suggestion_uid("human_suggestion"),
            based_on_agent_suggestion_uid: input.based_on_agent_suggestion_uid.clone(),
            message_review_base_hash: input.message_review_base_hash.clone(),
            outcome: input.outcome,
            suggested_action: input.suggested_action.clone(),
            reason: clean_optional_reason(input.reason.as_deref()),
            created_rfc3339: now_rfc3339(),
        };
        metadata.human_suggestion = Some(human.clone());
        write_message_metadata_file(&self.root, &metadata)?;
        let projection = self.refresh_message_projection(&input.message_id);
        let audit = self.append_audit_event(
            "human_suggestion_set",
            vec![audit_target("message", &input.message_id)],
            human.reason.as_deref(),
            json!({
                "human_suggestion_uid": human.human_suggestion_uid,
                "based_on_agent_suggestion_uid": human.based_on_agent_suggestion_uid,
                "message_review_base_hash": human.message_review_base_hash,
                "outcome": human.outcome,
                "suggested_action": human.suggested_action,
            }),
        );
        report_persisted_settlement_followup_errors(
            &human.human_suggestion_uid,
            [projection, audit],
        )?;
        Ok(human)
    }

    pub fn case_review_base_hash(&self, case_ref: &str) -> Result<String> {
        self.require_workspace()?;
        let collection = self.case_render_document(case_ref)?;
        let case_uid = collection.collection.collection_uid.clone();
        let notes = self.case_notes_render_document(&case_uid)?;
        let drafts = if collection.collection.status.is_empty() {
            self.draft_render_documents(&case_uid)?
        } else {
            Vec::new()
        };
        case_review_base_hash_from_documents(&collection, &notes, &drafts)
    }

    pub fn set_case_agent_suggestion(
        &self,
        case_ref: &str,
        case_review_base_hash: &str,
        action: CaseSuggestedAction,
        reason: &str,
        confidence_percent: Option<u8>,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_case_action(&action)?;
        let reason = checked_suggestion_text("reason", reason)?;
        let confidence_percent = checked_confidence_percent(confidence_percent)?;
        let case = self.resolve_case_any(case_ref)?;
        let current_hash = self.case_review_base_hash(case_ref)?;
        if current_hash != case_review_base_hash {
            return Err(stale_suggestion_error(
                "case",
                &case.case_uid,
                case_review_base_hash,
                &current_hash,
            ));
        }
        let mut data = read_case_file(&case.path)?;
        let suggestion = CaseAgentSuggestion {
            agent_suggestion_uid: suggestion_uid("agent_suggestion"),
            case_review_base_hash: current_hash,
            suggested_action: action,
            reason: reason.to_string(),
            confidence_percent,
            created_rfc3339: now_rfc3339(),
        };
        data.agent_suggestion = Some(CollectionAgentSuggestion::Case(suggestion.clone()));
        data.human_suggestion = None;
        write_case_file(&case.path, &data)?;
        self.append_audit_event(
            "agent_suggestion_set",
            vec![audit_target("case", &case.case_uid)],
            Some(reason),
            json!({
                "agent_suggestion_uid": suggestion.agent_suggestion_uid,
                "case_review_base_hash": suggestion.case_review_base_hash,
                "suggested_action": suggestion.suggested_action,
                "confidence_percent": suggestion.confidence_percent,
            }),
        )?;
        Ok(
            json!({"case_uid": case.case_uid, "agent_suggestion": suggestion, "human_suggestion_cleared": true}),
        )
    }

    pub fn clear_case_suggestion(
        &self,
        case_ref: &str,
        agent_suggestion_uid: &str,
    ) -> Result<Value> {
        self.require_workspace()?;
        let case = self.resolve_case_any(case_ref)?;
        let mut data = read_case_file(&case.path)?;
        let current_uid = match data.agent_suggestion.as_ref() {
            Some(CollectionAgentSuggestion::Case(value)) => value.agent_suggestion_uid.as_str(),
            _ => {
                return Err(AppError::new(
                    "suggestion_not_found",
                    "case has no agent_suggestion",
                ));
            }
        };
        if current_uid != agent_suggestion_uid {
            return Err(stale_suggestion_uid_error(
                current_uid,
                agent_suggestion_uid,
            ));
        }
        data.agent_suggestion = None;
        data.human_suggestion = None;
        write_case_file(&case.path, &data)?;
        self.append_audit_event(
            "suggestion_cleared",
            vec![audit_target("case", &case.case_uid)],
            None,
            json!({"agent_suggestion_uid": agent_suggestion_uid}),
        )?;
        Ok(
            json!({"case_uid": case.case_uid, "agent_suggestion_uid": agent_suggestion_uid, "suggestion_cleared": true}),
        )
    }

    /// A case's settled disposition, written the moment a person settles it.
    ///
    /// A case review has exactly one settleable item (the case itself, apart
    /// from its notes and drafts, which autosave independently), so unlike
    /// [`Workspace::set_message_human_suggestion`] there is no batch to speak
    /// of — this is that one write.
    #[cfg(any(feature = "review", test))]
    pub(crate) fn set_case_human_suggestion(
        &self,
        input: &CaseHumanSuggestionInput,
    ) -> Result<CaseHumanSuggestion> {
        self.require_workspace()?;
        let case = self.resolve_case_any(&input.case_ref)?;
        let mut data = read_case_file(&case.path)?;
        let current_hash = self.case_review_base_hash(&input.case_ref)?;
        if current_hash != input.case_review_base_hash {
            return Err(stale_suggestion_error(
                "case",
                &case.case_uid,
                &input.case_review_base_hash,
                &current_hash,
            ));
        }
        let agent_action = match input.based_on_agent_suggestion_uid.as_deref() {
            Some(based_on_uid) => {
                let agent = match data.agent_suggestion.as_ref() {
                    Some(CollectionAgentSuggestion::Case(value)) => value,
                    _ => {
                        return Err(AppError::new(
                            "suggestion_not_found",
                            "case has no agent_suggestion",
                        ));
                    }
                };
                if agent.agent_suggestion_uid != based_on_uid
                    || agent.case_review_base_hash != input.case_review_base_hash
                {
                    return Err(stale_suggestion_error(
                        "case",
                        &case.case_uid,
                        &input.case_review_base_hash,
                        &current_hash,
                    ));
                }
                Some(&agent.suggested_action)
            }
            None => None,
        };
        validate_human_case_fields(
            input.outcome,
            input.based_on_agent_suggestion_uid.as_deref(),
            input.suggested_action.as_ref(),
            input.reason.as_deref(),
            agent_action,
        )?;
        let human = CaseHumanSuggestion {
            human_suggestion_uid: suggestion_uid("human_suggestion"),
            based_on_agent_suggestion_uid: input.based_on_agent_suggestion_uid.clone(),
            case_review_base_hash: input.case_review_base_hash.clone(),
            outcome: input.outcome,
            suggested_action: input.suggested_action.clone(),
            reason: clean_optional_reason(input.reason.as_deref()),
            created_rfc3339: now_rfc3339(),
        };
        data.human_suggestion = Some(CollectionHumanSuggestion::Case(human.clone()));
        write_case_file(&case.path, &data)?;
        let audit = self.append_audit_event(
            "human_suggestion_set",
            vec![audit_target("case", &case.case_uid)],
            human.reason.as_deref(),
            json!({
                "human_suggestion_uid": human.human_suggestion_uid,
                "based_on_agent_suggestion_uid": human.based_on_agent_suggestion_uid,
                "case_review_base_hash": human.case_review_base_hash,
                "outcome": human.outcome,
                "suggested_action": human.suggested_action,
            }),
        );
        report_persisted_settlement_followup_errors(
            &human.human_suggestion_uid,
            std::iter::once(audit),
        )?;
        Ok(human)
    }

    pub fn notification_review_base_hash(&self, notification_ref: &str) -> Result<String> {
        self.require_workspace()?;
        let collection = self.notification_render_document(notification_ref)?;
        let notification_uid = collection.collection.collection_uid.clone();
        let notes = self.notification_notes_render_document(&notification_uid)?;
        notification_review_base_hash_from_documents(&collection, &notes)
    }

    pub fn set_notification_agent_suggestion(
        &self,
        notification_ref: &str,
        notification_review_base_hash: &str,
        action: NotificationSuggestedAction,
        reason: &str,
        confidence_percent: Option<u8>,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_notification_action(&action)?;
        let reason = checked_suggestion_text("reason", reason)?;
        let confidence_percent = checked_confidence_percent(confidence_percent)?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let current_hash = self.notification_review_base_hash(&notification_uid)?;
        if current_hash != notification_review_base_hash {
            return Err(stale_suggestion_error(
                "notification",
                &notification_uid,
                notification_review_base_hash,
                &current_hash,
            ));
        }
        let mut data = self.read_notifications(&notification_uid)?;
        let suggestion = NotificationAgentSuggestion {
            agent_suggestion_uid: suggestion_uid("agent_suggestion"),
            notification_review_base_hash: current_hash,
            suggested_action: action,
            reason: reason.to_string(),
            confidence_percent,
            created_rfc3339: now_rfc3339(),
        };
        data.agent_suggestion = Some(CollectionAgentSuggestion::Notification(suggestion.clone()));
        data.human_suggestion = None;
        self.write_notifications(&notification_uid, &data)?;
        self.append_audit_event(
            "agent_suggestion_set",
            vec![audit_target("notification", &notification_uid)],
            Some(reason),
            json!({
                "agent_suggestion_uid": suggestion.agent_suggestion_uid,
                "notification_review_base_hash": suggestion.notification_review_base_hash,
                "suggested_action": suggestion.suggested_action,
                "confidence_percent": suggestion.confidence_percent,
            }),
        )?;
        Ok(
            json!({"notification_uid": notification_uid, "agent_suggestion": suggestion, "human_suggestion_cleared": true}),
        )
    }

    pub fn clear_notification_suggestion(
        &self,
        notification_ref: &str,
        agent_suggestion_uid: &str,
    ) -> Result<Value> {
        self.require_workspace()?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let mut data = self.read_notifications(&notification_uid)?;
        let current_uid = match data.agent_suggestion.as_ref() {
            Some(CollectionAgentSuggestion::Notification(value)) => {
                value.agent_suggestion_uid.as_str()
            }
            _ => {
                return Err(AppError::new(
                    "suggestion_not_found",
                    "notification has no agent_suggestion",
                ));
            }
        };
        if current_uid != agent_suggestion_uid {
            return Err(stale_suggestion_uid_error(
                current_uid,
                agent_suggestion_uid,
            ));
        }
        data.agent_suggestion = None;
        data.human_suggestion = None;
        self.write_notifications(&notification_uid, &data)?;
        self.append_audit_event(
            "suggestion_cleared",
            vec![audit_target("notification", &notification_uid)],
            None,
            json!({"agent_suggestion_uid": agent_suggestion_uid}),
        )?;
        Ok(
            json!({"notification_uid": notification_uid, "agent_suggestion_uid": agent_suggestion_uid, "suggestion_cleared": true}),
        )
    }

    /// A notification collection's settled disposition, written the moment a
    /// person settles it. See [`Workspace::set_case_human_suggestion`]: same
    /// shape, one item.
    #[cfg(any(feature = "review", test))]
    pub(crate) fn set_notification_human_suggestion(
        &self,
        input: &NotificationHumanSuggestionInput,
    ) -> Result<NotificationHumanSuggestion> {
        self.require_workspace()?;
        let (notification_uid, _) = self.resolve_notification(&input.notification_ref)?;
        let mut data = self.read_notifications(&notification_uid)?;
        let current_hash = self.notification_review_base_hash(&notification_uid)?;
        if current_hash != input.notification_review_base_hash {
            return Err(stale_suggestion_error(
                "notification",
                &notification_uid,
                &input.notification_review_base_hash,
                &current_hash,
            ));
        }
        let agent_action = match input.based_on_agent_suggestion_uid.as_deref() {
            Some(based_on_uid) => {
                let agent = match data.agent_suggestion.as_ref() {
                    Some(CollectionAgentSuggestion::Notification(value)) => value,
                    _ => {
                        return Err(AppError::new(
                            "suggestion_not_found",
                            "notification has no agent_suggestion",
                        ));
                    }
                };
                if agent.agent_suggestion_uid != based_on_uid
                    || agent.notification_review_base_hash != input.notification_review_base_hash
                {
                    return Err(stale_suggestion_error(
                        "notification",
                        &notification_uid,
                        &input.notification_review_base_hash,
                        &current_hash,
                    ));
                }
                Some(&agent.suggested_action)
            }
            None => None,
        };
        validate_human_notification_fields(
            input.outcome,
            input.based_on_agent_suggestion_uid.as_deref(),
            input.suggested_action.as_ref(),
            input.reason.as_deref(),
            agent_action,
        )?;
        let human = NotificationHumanSuggestion {
            human_suggestion_uid: suggestion_uid("human_suggestion"),
            based_on_agent_suggestion_uid: input.based_on_agent_suggestion_uid.clone(),
            notification_review_base_hash: input.notification_review_base_hash.clone(),
            outcome: input.outcome,
            suggested_action: input.suggested_action.clone(),
            reason: clean_optional_reason(input.reason.as_deref()),
            created_rfc3339: now_rfc3339(),
        };
        data.human_suggestion = Some(CollectionHumanSuggestion::Notification(human.clone()));
        self.write_notifications(&notification_uid, &data)?;
        let audit = self.append_audit_event(
            "human_suggestion_set",
            vec![audit_target("notification", &notification_uid)],
            human.reason.as_deref(),
            json!({
                "human_suggestion_uid": human.human_suggestion_uid,
                "based_on_agent_suggestion_uid": human.based_on_agent_suggestion_uid,
                "notification_review_base_hash": human.notification_review_base_hash,
                "outcome": human.outcome,
                "suggested_action": human.suggested_action,
            }),
        );
        report_persisted_settlement_followup_errors(
            &human.human_suggestion_uid,
            std::iter::once(audit),
        )?;
        Ok(human)
    }

    fn refresh_message_projection(&self, message_id: &str) -> Result<()> {
        let message = self.read_message_by_id(message_id)?;
        self.write_message_materialized_cache(&message)
    }
}

/// Canonical suggestion files are the authority.  Once one has been replaced,
/// a projection or audit failure must never come back shaped like "nothing was
/// settled" — retrying that lie would mint a second human suggestion.  Keep
/// the underlying failures and mark the committed boundary explicitly so HTTP
/// callers can report an accurate, repair-required result.
#[cfg(any(feature = "review", test))]
fn report_persisted_settlement_followup_errors(
    human_suggestion_uid: &str,
    followups: impl IntoIterator<Item = Result<()>>,
) -> Result<()> {
    let failures = followups
        .into_iter()
        .filter_map(std::result::Result::err)
        .map(|error| {
            json!({
                "code": error.error_code,
                "message": error.message,
            })
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return Ok(());
    }
    Err(AppError::new(
        "settlement_followup_failed",
        "the human suggestion was persisted, but a derived projection or audit write failed",
    )
    .with_hint(
        "Do not repeat the decision. Run `afmail doctor`, repair the reported local follow-up, and reread the item.",
    )
    .with_details(json!({
        "settlement_persisted": true,
        "human_suggestion_uid": human_suggestion_uid,
        "followup_failures": failures,
    })))
}

/// The three invariants every [`crate::types::MessageHumanSuggestion`] (and
/// its case/notification equivalents) must satisfy, checked identically
/// whether the value was just built from a settle request or read back off
/// disk:
///
/// 1. `Decided` requires no `based_on_agent_suggestion_uid` and a complete
///    `suggested_action` — the person originated this, nothing was proposed;
/// 2. `Accepted` / `Changed` require `based_on_agent_suggestion_uid` — they
///    answer a proposal, so one must exist; `Accepted` must copy the current
///    Agent action exactly, `Changed` must carry a different complete one;
/// 3. `Rejected` requires a `reason` and carries no action — an Agent
///    suggestion is not required (a person may reject the idea that
///    anything is needed here even when nothing was proposed), but if one is
///    referenced, `agent_action` confirms it is still current.
fn validate_human_message_fields(
    outcome: HumanSuggestionOutcome,
    based_on_agent_suggestion_uid: Option<&str>,
    suggested_action: Option<&MessageSuggestedAction>,
    reason: Option<&str>,
    agent_action: Option<&MessageSuggestedAction>,
) -> Result<()> {
    match outcome {
        HumanSuggestionOutcome::Accepted => {
            if based_on_agent_suggestion_uid.is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion requires based_on_agent_suggestion_uid",
                ));
            }
            if suggested_action.is_none() || suggested_action != agent_action {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion must copy the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Changed => {
            if based_on_agent_suggestion_uid.is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires based_on_agent_suggestion_uid",
                ));
            }
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_message_action(action)?;
            if agent_action.is_none() || Some(action) == agent_action {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion must differ from the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Rejected => {
            if suggested_action.is_some() || clean_optional_reason(reason).is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "rejected human_suggestion requires a reason and must not contain an action",
                ));
            }
        }
        HumanSuggestionOutcome::Decided => {
            if based_on_agent_suggestion_uid.is_some() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "decided human_suggestion must not reference an agent_suggestion",
                ));
            }
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "decided human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_message_action(action)?;
        }
    }
    Ok(())
}

/// See [`validate_human_message_fields`]: same three invariants, case shape.
fn validate_human_case_fields(
    outcome: HumanSuggestionOutcome,
    based_on_agent_suggestion_uid: Option<&str>,
    suggested_action: Option<&CaseSuggestedAction>,
    reason: Option<&str>,
    agent_action: Option<&CaseSuggestedAction>,
) -> Result<()> {
    match outcome {
        HumanSuggestionOutcome::Accepted => {
            if based_on_agent_suggestion_uid.is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion requires based_on_agent_suggestion_uid",
                ));
            }
            if suggested_action.is_none() || suggested_action != agent_action {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion must copy the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Changed => {
            if based_on_agent_suggestion_uid.is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires based_on_agent_suggestion_uid",
                ));
            }
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_case_action(action)?;
            if agent_action.is_none() || Some(action) == agent_action {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion must differ from the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Rejected => {
            if suggested_action.is_some() || clean_optional_reason(reason).is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "rejected human_suggestion requires a reason and must not contain an action",
                ));
            }
        }
        HumanSuggestionOutcome::Decided => {
            if based_on_agent_suggestion_uid.is_some() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "decided human_suggestion must not reference an agent_suggestion",
                ));
            }
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "decided human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_case_action(action)?;
        }
    }
    Ok(())
}

/// See [`validate_human_message_fields`]: same three invariants, notification
/// shape.
fn validate_human_notification_fields(
    outcome: HumanSuggestionOutcome,
    based_on_agent_suggestion_uid: Option<&str>,
    suggested_action: Option<&NotificationSuggestedAction>,
    reason: Option<&str>,
    agent_action: Option<&NotificationSuggestedAction>,
) -> Result<()> {
    match outcome {
        HumanSuggestionOutcome::Accepted => {
            if based_on_agent_suggestion_uid.is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion requires based_on_agent_suggestion_uid",
                ));
            }
            if suggested_action.is_none() || suggested_action != agent_action {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion must copy the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Changed => {
            if based_on_agent_suggestion_uid.is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires based_on_agent_suggestion_uid",
                ));
            }
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_notification_action(action)?;
            if agent_action.is_none() || Some(action) == agent_action {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion must differ from the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Rejected => {
            if suggested_action.is_some() || clean_optional_reason(reason).is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "rejected human_suggestion requires a reason and must not contain an action",
                ));
            }
        }
        HumanSuggestionOutcome::Decided => {
            if based_on_agent_suggestion_uid.is_some() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "decided human_suggestion must not reference an agent_suggestion",
                ));
            }
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "decided human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_notification_action(action)?;
        }
    }
    Ok(())
}

fn clean_optional_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn stale_suggestion_error(
    object_kind: &str,
    object_ref: &str,
    expected_hash: &str,
    current_hash: &str,
) -> AppError {
    AppError::new(
        "review_stale",
        format!("{object_kind} {object_ref} changed after the Agent analyzed it"),
    )
    .with_hint("Read the object again and set a new suggestion using its current review hash.")
    .with_details(json!({
        "object_kind": object_kind,
        "object_ref": object_ref,
        "expected_review_base_hash": expected_hash,
        "current_review_base_hash": current_hash,
    }))
}

fn stale_suggestion_uid_error(current_uid: &str, supplied_uid: &str) -> AppError {
    AppError::new(
        "suggestion_stale",
        "the supplied agent_suggestion_uid is no longer current",
    )
    .with_details(json!({
        "agent_suggestion_uid": supplied_uid,
        "current_agent_suggestion_uid": current_uid,
    }))
}

/// The three invariants documented on [`validate_human_message_fields`], each
/// pinned by a test that a violation is refused. These are pure checks on
/// the free functions — no workspace, no disk — so they exercise the same
/// rule whether it fires while writing a fresh settlement or while
/// re-validating one just read back off disk.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decided_with_a_based_on_agent_suggestion_uid_is_refused() {
        let error = validate_human_message_fields(
            HumanSuggestionOutcome::Decided,
            Some("agent_suggestion_1"),
            Some(&MessageSuggestedAction::Spam),
            None,
            None,
        )
        .expect_err("Decided must not reference an agent_suggestion");
        assert_eq!(error.error_code, "invalid_human_suggestion");
    }

    #[test]
    fn accepted_without_a_based_on_agent_suggestion_uid_is_refused() {
        let error = validate_human_message_fields(
            HumanSuggestionOutcome::Accepted,
            None,
            Some(&MessageSuggestedAction::Spam),
            None,
            Some(&MessageSuggestedAction::Spam),
        )
        .expect_err("Accepted must reference an agent_suggestion");
        assert_eq!(error.error_code, "invalid_human_suggestion");
    }

    #[test]
    fn rejected_without_a_reason_is_refused() {
        let error =
            validate_human_message_fields(HumanSuggestionOutcome::Rejected, None, None, None, None)
                .expect_err("Rejected must carry a reason");
        assert_eq!(error.error_code, "invalid_human_suggestion");
    }

    #[test]
    fn changed_without_a_based_on_agent_suggestion_uid_is_refused() {
        let error = validate_human_message_fields(
            HumanSuggestionOutcome::Changed,
            None,
            Some(&MessageSuggestedAction::Trash),
            None,
            None,
        )
        .expect_err("Changed must reference an agent_suggestion");
        assert_eq!(error.error_code, "invalid_human_suggestion");
    }

    #[test]
    fn changed_must_actually_differ_for_every_suggestion_shape() {
        let message = MessageSuggestedAction::Spam;
        let error = validate_human_message_fields(
            HumanSuggestionOutcome::Changed,
            Some("agent_suggestion_1"),
            Some(&message),
            Some("different reasoning"),
            Some(&message),
        )
        .expect_err("an unchanged message action is not Changed");
        assert_eq!(error.error_code, "invalid_human_suggestion");

        let case = CaseSuggestedAction::CaseArchive;
        let error = validate_human_case_fields(
            HumanSuggestionOutcome::Changed,
            Some("agent_suggestion_1"),
            Some(&case),
            Some("different reasoning"),
            Some(&case),
        )
        .expect_err("an unchanged case action is not Changed");
        assert_eq!(error.error_code, "invalid_human_suggestion");

        let notification = NotificationSuggestedAction::NotificationRename {
            notification_name: "Release".to_string(),
        };
        let error = validate_human_notification_fields(
            HumanSuggestionOutcome::Changed,
            Some("agent_suggestion_1"),
            Some(&notification),
            Some("different reasoning"),
            Some(&notification),
        )
        .expect_err("an unchanged notification action is not Changed");
        assert_eq!(error.error_code, "invalid_human_suggestion");
    }

    #[test]
    fn decided_without_a_suggested_action_is_refused() {
        let error =
            validate_human_message_fields(HumanSuggestionOutcome::Decided, None, None, None, None)
                .expect_err("Decided must carry a complete suggested_action");
        assert_eq!(error.error_code, "invalid_human_suggestion");
    }

    #[test]
    fn a_well_formed_value_of_each_outcome_is_accepted() {
        validate_human_message_fields(
            HumanSuggestionOutcome::Accepted,
            Some("agent_suggestion_1"),
            Some(&MessageSuggestedAction::Spam),
            None,
            Some(&MessageSuggestedAction::Spam),
        )
        .expect("accepted, bound, matching action");
        validate_human_message_fields(
            HumanSuggestionOutcome::Changed,
            Some("agent_suggestion_1"),
            Some(&MessageSuggestedAction::Trash),
            None,
            Some(&MessageSuggestedAction::Spam),
        )
        .expect("changed, bound, different complete action");
        validate_human_message_fields(
            HumanSuggestionOutcome::Rejected,
            Some("agent_suggestion_1"),
            None,
            Some("not needed"),
            Some(&MessageSuggestedAction::Spam),
        )
        .expect("rejected, bound, with a reason");
        validate_human_message_fields(
            HumanSuggestionOutcome::Rejected,
            None,
            None,
            Some("nothing to do here"),
            None,
        )
        .expect("rejected with no agent_suggestion at all, still with a reason");
        validate_human_message_fields(
            HumanSuggestionOutcome::Decided,
            None,
            Some(&MessageSuggestedAction::Trash),
            None,
            None,
        )
        .expect("decided, unbound, with a complete action");
    }
}
