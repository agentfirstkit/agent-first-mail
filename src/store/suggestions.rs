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

#[derive(Clone, Debug)]
#[cfg(any(feature = "review", test))]
pub(crate) struct MessageHumanSuggestionInput {
    pub(crate) message_id: String,
    pub(crate) agent_suggestion_uid: String,
    pub(crate) message_review_base_hash: String,
    pub(crate) outcome: HumanSuggestionOutcome,
    pub(crate) suggested_action: Option<MessageSuggestedAction>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug)]
#[cfg(any(feature = "review", test))]
pub(crate) struct CaseHumanSuggestionInput {
    pub(crate) case_ref: String,
    pub(crate) agent_suggestion_uid: String,
    pub(crate) case_review_base_hash: String,
    pub(crate) outcome: HumanSuggestionOutcome,
    pub(crate) suggested_action: Option<CaseSuggestedAction>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug)]
#[cfg(any(feature = "review", test))]
pub(crate) struct NotificationHumanSuggestionInput {
    pub(crate) notification_ref: String,
    pub(crate) agent_suggestion_uid: String,
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

fn validate_human_fields(
    uid: &str,
    based_on_uid: &str,
    review_base_hash: &str,
    created_rfc3339: &str,
) -> Result<()> {
    checked_suggestion_text("human_suggestion_uid", uid)?;
    checked_suggestion_text("based_on_agent_suggestion_uid", based_on_uid)?;
    checked_suggestion_text("review_base_hash", review_base_hash)?;
    checked_suggestion_text("created_rfc3339", created_rfc3339)?;
    Ok(())
}

pub(super) fn validate_message_suggestion_pair(
    agent: Option<&MessageAgentSuggestion>,
    human: Option<&MessageHumanSuggestion>,
) -> Result<()> {
    let Some(agent) = agent else {
        return if human.is_none() {
            Ok(())
        } else {
            Err(AppError::new(
                "invalid_suggestion_state",
                "human_suggestion requires a current agent_suggestion",
            ))
        };
    };
    validate_agent_fields(
        &agent.agent_suggestion_uid,
        &agent.message_review_base_hash,
        &agent.reason,
        agent.confidence_percent,
        &agent.created_rfc3339,
    )?;
    validate_message_action(&agent.suggested_action)?;
    if let Some(human) = human {
        validate_human_fields(
            &human.human_suggestion_uid,
            &human.based_on_agent_suggestion_uid,
            &human.message_review_base_hash,
            &human.created_rfc3339,
        )?;
        if human.based_on_agent_suggestion_uid != agent.agent_suggestion_uid
            || human.message_review_base_hash != agent.message_review_base_hash
        {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "human_suggestion is not bound to the current agent_suggestion",
            ));
        }
        validate_human_message_fields(
            human.outcome,
            human.suggested_action.as_ref(),
            human.reason.as_deref(),
            &agent.suggested_action,
        )?;
    }
    Ok(())
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
    let Some(agent) = agent else {
        return if human.is_none() {
            Ok(())
        } else {
            Err(AppError::new(
                "invalid_suggestion_state",
                "human_suggestion requires a current agent_suggestion",
            ))
        };
    };
    validate_agent_fields(
        &agent.agent_suggestion_uid,
        &agent.case_review_base_hash,
        &agent.reason,
        agent.confidence_percent,
        &agent.created_rfc3339,
    )?;
    validate_case_action(&agent.suggested_action)?;
    if let Some(human) = human {
        validate_human_fields(
            &human.human_suggestion_uid,
            &human.based_on_agent_suggestion_uid,
            &human.case_review_base_hash,
            &human.created_rfc3339,
        )?;
        if human.based_on_agent_suggestion_uid != agent.agent_suggestion_uid
            || human.case_review_base_hash != agent.case_review_base_hash
        {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "human_suggestion is not bound to the current agent_suggestion",
            ));
        }
        validate_human_case_fields(
            human.outcome,
            human.suggested_action.as_ref(),
            human.reason.as_deref(),
            &agent.suggested_action,
        )?;
    }
    Ok(())
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
    let Some(agent) = agent else {
        return if human.is_none() {
            Ok(())
        } else {
            Err(AppError::new(
                "invalid_suggestion_state",
                "human_suggestion requires a current agent_suggestion",
            ))
        };
    };
    validate_agent_fields(
        &agent.agent_suggestion_uid,
        &agent.notification_review_base_hash,
        &agent.reason,
        agent.confidence_percent,
        &agent.created_rfc3339,
    )?;
    validate_notification_action(&agent.suggested_action)?;
    if let Some(human) = human {
        validate_human_fields(
            &human.human_suggestion_uid,
            &human.based_on_agent_suggestion_uid,
            &human.notification_review_base_hash,
            &human.created_rfc3339,
        )?;
        if human.based_on_agent_suggestion_uid != agent.agent_suggestion_uid
            || human.notification_review_base_hash != agent.notification_review_base_hash
        {
            return Err(AppError::new(
                "invalid_suggestion_state",
                "human_suggestion is not bound to the current agent_suggestion",
            ));
        }
        validate_human_notification_fields(
            human.outcome,
            human.suggested_action.as_ref(),
            human.reason.as_deref(),
            &agent.suggested_action,
        )?;
    }
    Ok(())
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

    pub fn triage_suggested_message_ids(&self) -> Result<Vec<String>> {
        self.require_workspace()?;
        let mut message_ids = Vec::new();
        for path in message_json_paths(&self.root)? {
            let message = read_message(&path)?;
            if message.workspace.status == MessageStatus::Triage.as_str()
                && message.agent_suggestion.is_some()
            {
                message_ids.push(message.message_id);
            }
        }
        message_ids.sort();
        Ok(message_ids)
    }

    #[cfg(any(feature = "review", test))]
    pub(crate) fn set_message_human_suggestions(
        &self,
        inputs: &[MessageHumanSuggestionInput],
    ) -> Result<Vec<(String, MessageHumanSuggestion)>> {
        self.require_workspace()?;
        if inputs.is_empty() {
            return Err(AppError::new(
                "invalid_human_suggestion",
                "triage review must contain at least one message",
            ));
        }
        let mut prepared = Vec::with_capacity(inputs.len());
        let mut seen = BTreeSet::new();
        for input in inputs {
            validate_id("message_id", &input.message_id)?;
            if !seen.insert(input.message_id.as_str()) {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "triage review contains the same message more than once",
                ));
            }
            let mut metadata = read_message_metadata_file(&self.root, &input.message_id)?
                .ok_or_else(|| {
                    AppError::new(
                        "message_metadata_missing",
                        format!(
                            "canonical metadata is missing for message {}",
                            input.message_id
                        ),
                    )
                })?;
            let agent = metadata.agent_suggestion.as_ref().ok_or_else(|| {
                AppError::new(
                    "suggestion_not_found",
                    format!("message {} has no agent_suggestion", input.message_id),
                )
            })?;
            let current_hash = self.message_review_base_hash(&input.message_id)?;
            if agent.agent_suggestion_uid != input.agent_suggestion_uid
                || agent.message_review_base_hash != input.message_review_base_hash
                || current_hash != input.message_review_base_hash
            {
                return Err(stale_suggestion_error(
                    "message",
                    &input.message_id,
                    &input.message_review_base_hash,
                    &current_hash,
                ));
            }
            validate_human_message_choice(input, &agent.suggested_action)?;
            let human = MessageHumanSuggestion {
                human_suggestion_uid: suggestion_uid("human_suggestion"),
                based_on_agent_suggestion_uid: input.agent_suggestion_uid.clone(),
                message_review_base_hash: input.message_review_base_hash.clone(),
                outcome: input.outcome,
                suggested_action: input.suggested_action.clone(),
                reason: clean_optional_reason(input.reason.as_deref()),
                created_rfc3339: now_rfc3339(),
            };
            metadata.human_suggestion = Some(human.clone());
            prepared.push((input.message_id.clone(), metadata, human));
        }
        for (message_id, metadata, human) in &prepared {
            write_message_metadata_file(&self.root, metadata)?;
            self.refresh_message_projection(message_id)?;
            self.append_audit_event(
                "human_suggestion_set",
                vec![audit_target("message", message_id)],
                human.reason.as_deref(),
                json!({
                    "human_suggestion_uid": human.human_suggestion_uid,
                    "based_on_agent_suggestion_uid": human.based_on_agent_suggestion_uid,
                    "message_review_base_hash": human.message_review_base_hash,
                    "outcome": human.outcome,
                    "suggested_action": human.suggested_action,
                }),
            )?;
        }
        Ok(prepared
            .into_iter()
            .map(|(message_id, _, human)| (message_id, human))
            .collect())
    }

    pub fn case_review_base_hash(&self, case_ref: &str) -> Result<String> {
        self.require_workspace()?;
        let mut collection = self.case_render_document(case_ref)?;
        collection.collection.agent_suggestion = None;
        collection.collection.human_suggestion = None;
        let case_uid = collection.collection.collection_uid.clone();
        let notes = self.case_notes_render_document(&case_uid)?;
        let drafts = if collection.collection.status.is_empty() {
            self.draft_render_documents(&case_uid)?
        } else {
            Vec::new()
        };
        review_document_hash(
            "case",
            &CollectionReviewBaseDocument::Case {
                collection: &collection,
                notes: &notes,
                drafts: &drafts,
            },
        )
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

    #[cfg(any(feature = "review", test))]
    pub(crate) fn set_case_human_suggestion(
        &self,
        input: &CaseHumanSuggestionInput,
    ) -> Result<CaseHumanSuggestion> {
        self.require_workspace()?;
        let case = self.resolve_case_any(&input.case_ref)?;
        let mut data = read_case_file(&case.path)?;
        let agent = match data.agent_suggestion.as_ref() {
            Some(CollectionAgentSuggestion::Case(value)) => value,
            _ => {
                return Err(AppError::new(
                    "suggestion_not_found",
                    "case has no agent_suggestion",
                ));
            }
        };
        let current_hash = self.case_review_base_hash(&input.case_ref)?;
        if agent.agent_suggestion_uid != input.agent_suggestion_uid
            || agent.case_review_base_hash != input.case_review_base_hash
            || current_hash != input.case_review_base_hash
        {
            return Err(stale_suggestion_error(
                "case",
                &case.case_uid,
                &input.case_review_base_hash,
                &current_hash,
            ));
        }
        validate_human_case_choice(input, &agent.suggested_action)?;
        let human = CaseHumanSuggestion {
            human_suggestion_uid: suggestion_uid("human_suggestion"),
            based_on_agent_suggestion_uid: input.agent_suggestion_uid.clone(),
            case_review_base_hash: input.case_review_base_hash.clone(),
            outcome: input.outcome,
            suggested_action: input.suggested_action.clone(),
            reason: clean_optional_reason(input.reason.as_deref()),
            created_rfc3339: now_rfc3339(),
        };
        data.human_suggestion = Some(CollectionHumanSuggestion::Case(human.clone()));
        write_case_file(&case.path, &data)?;
        self.append_audit_event(
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
        )?;
        Ok(human)
    }

    pub fn notification_review_base_hash(&self, notification_ref: &str) -> Result<String> {
        self.require_workspace()?;
        let mut collection = self.notification_render_document(notification_ref)?;
        collection.collection.agent_suggestion = None;
        collection.collection.human_suggestion = None;
        let notification_uid = collection.collection.collection_uid.clone();
        let notes = self.notification_notes_render_document(&notification_uid)?;
        review_document_hash(
            "notification",
            &CollectionReviewBaseDocument::Notification {
                collection: &collection,
                notes: &notes,
            },
        )
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

    #[cfg(any(feature = "review", test))]
    pub(crate) fn set_notification_human_suggestion(
        &self,
        input: &NotificationHumanSuggestionInput,
    ) -> Result<NotificationHumanSuggestion> {
        self.require_workspace()?;
        let (notification_uid, _) = self.resolve_notification(&input.notification_ref)?;
        let mut data = self.read_notifications(&notification_uid)?;
        let agent = match data.agent_suggestion.as_ref() {
            Some(CollectionAgentSuggestion::Notification(value)) => value,
            _ => {
                return Err(AppError::new(
                    "suggestion_not_found",
                    "notification has no agent_suggestion",
                ));
            }
        };
        let current_hash = self.notification_review_base_hash(&notification_uid)?;
        if agent.agent_suggestion_uid != input.agent_suggestion_uid
            || agent.notification_review_base_hash != input.notification_review_base_hash
            || current_hash != input.notification_review_base_hash
        {
            return Err(stale_suggestion_error(
                "notification",
                &notification_uid,
                &input.notification_review_base_hash,
                &current_hash,
            ));
        }
        validate_human_notification_fields(
            input.outcome,
            input.suggested_action.as_ref(),
            input.reason.as_deref(),
            &agent.suggested_action,
        )?;
        let human = NotificationHumanSuggestion {
            human_suggestion_uid: suggestion_uid("human_suggestion"),
            based_on_agent_suggestion_uid: input.agent_suggestion_uid.clone(),
            notification_review_base_hash: input.notification_review_base_hash.clone(),
            outcome: input.outcome,
            suggested_action: input.suggested_action.clone(),
            reason: clean_optional_reason(input.reason.as_deref()),
            created_rfc3339: now_rfc3339(),
        };
        data.human_suggestion = Some(CollectionHumanSuggestion::Notification(human.clone()));
        self.write_notifications(&notification_uid, &data)?;
        self.append_audit_event(
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
        )?;
        Ok(human)
    }

    fn refresh_message_projection(&self, message_id: &str) -> Result<()> {
        let message = self.read_message_by_id(message_id)?;
        self.write_message_materialized_cache(&message)
    }
}

#[cfg(any(feature = "review", test))]
fn validate_human_message_choice(
    input: &MessageHumanSuggestionInput,
    agent_action: &MessageSuggestedAction,
) -> Result<()> {
    validate_human_message_fields(
        input.outcome,
        input.suggested_action.as_ref(),
        input.reason.as_deref(),
        agent_action,
    )
}

fn validate_human_message_fields(
    outcome: HumanSuggestionOutcome,
    suggested_action: Option<&MessageSuggestedAction>,
    reason: Option<&str>,
    agent_action: &MessageSuggestedAction,
) -> Result<()> {
    match outcome {
        HumanSuggestionOutcome::Accepted => {
            if suggested_action != Some(agent_action) {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion must copy the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Changed => {
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_message_action(action)?;
        }
        HumanSuggestionOutcome::Rejected => {
            if suggested_action.is_some() || clean_optional_reason(reason).is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "rejected human_suggestion requires a reason and must not contain an action",
                ));
            }
        }
        HumanSuggestionOutcome::Deferred => {
            if suggested_action.is_some() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "deferred human_suggestion must not contain an action",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(any(feature = "review", test))]
fn validate_human_case_choice(
    input: &CaseHumanSuggestionInput,
    agent_action: &CaseSuggestedAction,
) -> Result<()> {
    validate_human_case_fields(
        input.outcome,
        input.suggested_action.as_ref(),
        input.reason.as_deref(),
        agent_action,
    )
}

fn validate_human_case_fields(
    outcome: HumanSuggestionOutcome,
    suggested_action: Option<&CaseSuggestedAction>,
    reason: Option<&str>,
    agent_action: &CaseSuggestedAction,
) -> Result<()> {
    match outcome {
        HumanSuggestionOutcome::Accepted => {
            if suggested_action != Some(agent_action) {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion must copy the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Changed => {
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_case_action(action)?;
        }
        HumanSuggestionOutcome::Rejected => {
            if suggested_action.is_some() || clean_optional_reason(reason).is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "rejected human_suggestion requires a reason and must not contain an action",
                ));
            }
        }
        HumanSuggestionOutcome::Deferred => {
            if suggested_action.is_some() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "deferred human_suggestion must not contain an action",
                ));
            }
        }
    }
    Ok(())
}

fn validate_human_notification_fields(
    outcome: HumanSuggestionOutcome,
    suggested_action: Option<&NotificationSuggestedAction>,
    reason: Option<&str>,
    agent_action: &NotificationSuggestedAction,
) -> Result<()> {
    match outcome {
        HumanSuggestionOutcome::Accepted => {
            if suggested_action != Some(agent_action) {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "accepted human_suggestion must copy the current Agent action",
                ));
            }
        }
        HumanSuggestionOutcome::Changed => {
            let action = suggested_action.ok_or_else(|| {
                AppError::new(
                    "invalid_human_suggestion",
                    "changed human_suggestion requires a complete suggested_action",
                )
            })?;
            validate_notification_action(action)?;
        }
        HumanSuggestionOutcome::Rejected => {
            if suggested_action.is_some() || clean_optional_reason(reason).is_none() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "rejected human_suggestion requires a reason and must not contain an action",
                ));
            }
        }
        HumanSuggestionOutcome::Deferred => {
            if suggested_action.is_some() {
                return Err(AppError::new(
                    "invalid_human_suggestion",
                    "deferred human_suggestion must not contain an action",
                ));
            }
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
