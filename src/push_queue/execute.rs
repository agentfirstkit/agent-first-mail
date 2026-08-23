use super::*;
use crate::remote::MailRemote;
use crate::types::{MessageFile, RemoteLocation};

#[derive(Clone, Copy)]
pub(super) struct PushProgressContext {
    pub(super) item_index: usize,
    pub(super) item_count: usize,
}

#[derive(Clone, Copy)]
pub(super) struct PushExecutionContext<'a> {
    pub(super) root: &'a Path,
    pub(super) config: &'a MailConfig,
    pub(super) remote: &'a dyn MailRemote,
    pub(super) progress: PushProgressContext,
}

pub(super) fn push_outbound(
    root: &Path,
    config: &MailConfig,
    remote: &dyn MailRemote,
    item: &mut PushItem,
    context: PushProgressContext,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<()> {
    let outbound = item
        .outbound()
        .ok_or_else(|| AppError::new("push_item_invalid", "push item is not outbound"))?;
    let steps = if outbound.steps.is_empty() {
        let configured = match outbound.action {
            OutboundAction::SaveDraft => {
                if config.actions.draft.save.steps.is_empty() {
                    vec![ActionStep::append_to_mailbox_id("drafts")]
                } else {
                    config.actions.draft.save.steps.clone()
                }
            }
            OutboundAction::Send => {
                if config.actions.draft.send.steps.is_empty() {
                    crate::config::default_draft_send_steps()
                } else {
                    config.actions.draft.send.steps.clone()
                }
            }
        };
        filter_reply_to_steps(root, item, configured)?
    } else {
        outbound.steps.clone()
    };
    execute_tracked_steps(
        PushExecutionContext {
            root,
            config,
            remote,
            progress: context,
        },
        item,
        &steps,
        progress,
        execute_outbound_step,
    )
}

pub(super) fn filter_reply_to_steps(
    root: &Path,
    item: &PushItem,
    steps: Vec<ActionStep>,
) -> Result<Vec<ActionStep>> {
    if !steps
        .iter()
        .any(|step| step.on == Some(ActionStepOn::ReplyToMessage))
    {
        return Ok(steps);
    }
    if should_skip_reply_to_steps(root, item)? {
        return Ok(steps
            .into_iter()
            .filter(|step| step.on != Some(ActionStepOn::ReplyToMessage))
            .collect());
    }
    Ok(steps)
}

pub(super) fn should_skip_reply_to_steps(root: &Path, item: &PushItem) -> Result<bool> {
    let Some(outbound) = item.outbound() else {
        return Ok(true);
    };
    let case_uid = outbound.case_uid.as_str();
    let draft_name = outbound.draft_name.as_str();
    let case_path = find_case_path(root, case_uid)?;
    let draft_path = case_path.join("drafts").join(draft_name);
    ensure_draft_file_exists(root, case_uid, draft_name, &draft_path)?;
    let draft_text = fs::read_to_string(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
    let (frontmatter, _) = crate::markdown::read_doc::<DraftFrontmatter>(&draft_text)?;
    Ok(frontmatter.send_intent.as_deref() == Some("new")
        || frontmatter.reply_to_message_id.is_none())
}

fn ensure_draft_file_exists(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
    draft_path: &Path,
) -> Result<()> {
    if draft_path.is_file() {
        return Ok(());
    }
    Err(AppError::new(
        "draft_not_found",
        format!("queued draft file is missing: {draft_name}"),
    )
    .with_hint(format!(
        "Remove the stale push item with `afmail case draft remove {case_uid} {draft_name} --reason TEXT`, or recreate the draft before retrying."
    ))
    .with_details(json!({
        "case_uid": case_uid,
        "draft_name": draft_name,
        "draft_path": rel_path(root, draft_path),
        "suggested_commands": [
            format!("afmail case draft remove {case_uid} {draft_name} --reason TEXT"),
            format!("afmail case draft new {case_uid} --to ADDR --subject TEXT")
        ]
    })))
}

pub(super) fn prepare_current_outbound(
    root: &Path,
    config: &MailConfig,
    item: &PushItem,
) -> Result<crate::types::PreparedOutbound> {
    if let Some(prepared) = &item.planned_outbound_message {
        return Ok(prepared.clone());
    }
    let outbound = item
        .outbound()
        .ok_or_else(|| AppError::new("push_item_invalid", "push item is not outbound"))?;
    let case_uid = outbound.case_uid.as_str();
    let draft_name = outbound.draft_name.as_str();
    let case_path = find_case_path(root, case_uid)?;
    let draft_path = case_path.join("drafts").join(draft_name);
    ensure_draft_file_exists(root, case_uid, draft_name, &draft_path)?;
    let workspace = crate::store::Workspace::at(root);
    workspace.validate_draft_for_push(case_uid, draft_name, &case_path, outbound.action)?;
    crate::smtp_send::prepare_outbound(
        root,
        &case_path,
        case_uid,
        draft_name,
        config,
        Some(&crate::smtp_send::message_id_for_push(&item.push_id)),
        Some(&item.created_rfc3339),
    )
}

pub(super) fn push_action_steps(
    root: &Path,
    config: &MailConfig,
    remote: &dyn MailRemote,
    item: &mut PushItem,
    context: PushProgressContext,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<()> {
    let workspace = crate::store::Workspace::at(root);
    let current_push_path = push_path(root, &item.push_id);
    let payload = item
        .message_action()
        .ok_or_else(|| AppError::new("push_item_invalid", "push item is not a message action"))?;
    match payload.action {
        MessagePushAction::Notification => {
            workspace.ensure_archive_eligible(&payload.message_ids, Some(&current_push_path))?;
        }
        MessagePushAction::Spam | MessagePushAction::Trash => {
            workspace
                .ensure_message_ids_unreferenced(&payload.message_ids, Some(&current_push_path))?;
        }
        MessagePushAction::CaseAdd | MessagePushAction::Unspam | MessagePushAction::Untrash => {}
    }
    let steps = payload.steps.clone();
    execute_tracked_steps(
        PushExecutionContext {
            root,
            config,
            remote,
            progress: context,
        },
        item,
        &steps,
        progress,
        execute_message_step,
    )
}

pub(super) fn execute_tracked_steps(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    steps: &[ActionStep],
    progress: Option<&mut ProgressCallback<'_>>,
    mut execute: impl FnMut(PushExecutionContext<'_>, &mut PushItem, usize, &ActionStep) -> Result<()>,
) -> Result<()> {
    let mut progress = progress;
    ensure_step_states(item, steps);
    for (index, step) in steps.iter().enumerate() {
        let label = step_label(step);
        if step_succeeded(item, index) {
            if item.local_stage.is_none() {
                refresh_after_successful_step(
                    context,
                    item,
                    step,
                    index,
                    steps.len(),
                    &label,
                    &mut progress,
                )?;
            }
            continue;
        }
        if step_remote_outcome_unknown(item, index) {
            if recover_remote_outcome_unknown_step(context, item, index, step)? {
                crate::progress::emit(
                    &mut progress,
                    "push_step_done",
                    push_step_progress_fields(
                        item,
                        context.progress,
                        index,
                        steps.len(),
                        &label,
                        None,
                    ),
                );
                refresh_after_successful_step(
                    context,
                    item,
                    step,
                    index,
                    steps.len(),
                    &label,
                    &mut progress,
                )?;
                continue;
            }
            return Err(remote_outcome_unknown_error(
                context.root,
                item,
                index,
                step,
                None,
            ));
        }
        crate::progress::emit(
            &mut progress,
            "push_step_start",
            push_step_progress_fields(item, context.progress, index, steps.len(), &label, None),
        );
        mark_step_pending(item, index, step);
        write_item(context.root, item)?;
        match execute(context, item, index, step) {
            Ok(()) => {
                mark_step_succeeded(item, index, step);
                item.updated_rfc3339 = crate::store::now_rfc3339();
                item.last_error = None;
                write_item(context.root, item)?;
                crate::progress::emit(
                    &mut progress,
                    "push_step_done",
                    push_step_progress_fields(
                        item,
                        context.progress,
                        index,
                        steps.len(),
                        &label,
                        None,
                    ),
                );
                refresh_after_successful_step(
                    context,
                    item,
                    step,
                    index,
                    steps.len(),
                    &label,
                    &mut progress,
                )?;
            }
            Err(err) => {
                let err = if step_remote_outcome_unknown(item, index)
                    && !remote_calls_not_started(&err)
                {
                    let unknown =
                        remote_outcome_unknown_error(context.root, item, index, step, Some(&err));
                    mark_step_remote_outcome_unknown(item, index, step, Some(&err));
                    item.updated_rfc3339 = crate::store::now_rfc3339();
                    item.last_error = Some(unknown.to_string());
                    write_item(context.root, item)?;
                    unknown
                } else {
                    mark_step_failed(item, index, step, &err);
                    item.updated_rfc3339 = crate::store::now_rfc3339();
                    item.last_error = Some(err.to_string());
                    write_item(context.root, item)?;
                    err
                };
                crate::progress::emit(
                    &mut progress,
                    "push_step_failed",
                    push_step_progress_fields(
                        item,
                        context.progress,
                        index,
                        steps.len(),
                        &label,
                        Some(&err),
                    ),
                );
                return Err(err);
            }
        }
    }
    Ok(())
}

fn remote_calls_not_started(error: &AppError) -> bool {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("remote_calls_started"))
        .and_then(Value::as_bool)
        == Some(false)
}

fn refresh_after_successful_step(
    context: PushExecutionContext<'_>,
    item: &PushItem,
    step: &ActionStep,
    index: usize,
    step_count: usize,
    label: &str,
    progress: &mut Option<&mut ProgressCallback<'_>>,
) -> Result<()> {
    let message_ids = message_ids_for_step_refresh(context.root, item, step)?;
    if message_ids.is_empty() {
        return Ok(());
    }
    crate::progress::emit(
        progress,
        "push_step_refresh_start",
        push_step_progress_fields(item, context.progress, index, step_count, label, None),
    );
    crate::store::Workspace::at(context.root).refresh_message_push_views(&message_ids)?;
    crate::progress::emit(
        progress,
        "push_step_refresh_done",
        push_step_progress_fields(item, context.progress, index, step_count, label, None),
    );
    Ok(())
}

fn message_ids_for_step_refresh(
    root: &Path,
    item: &PushItem,
    step: &ActionStep,
) -> Result<Vec<String>> {
    if step.add_flags.is_empty()
        && step.move_to_mailbox_id.is_none()
        && step.append_to_mailbox_id.is_none()
    {
        return Ok(Vec::new());
    }
    let mut ids = std::collections::BTreeSet::new();
    if !step.add_flags.is_empty() || step.move_to_mailbox_id.is_some() {
        for location in action_locations(root, item, step)? {
            ids.insert(location.message_id);
        }
    }
    if step.append_to_mailbox_id.is_some() {
        ids.extend(item.message_ids().iter().cloned());
    }
    Ok(ids.into_iter().collect())
}

fn push_step_progress_fields(
    item: &PushItem,
    context: PushProgressContext,
    index: usize,
    step_count: usize,
    label: &str,
    err: Option<&AppError>,
) -> Value {
    let mut value = json!({
        "push_id": item.push_id.as_str(),
        "kind": item.kind(),
        "display_kind": item.display_kind(),
        "item_index": context.item_index + 1,
        "item_count": context.item_count,
        "step_index": index + 1,
        "step_count": step_count,
        "label": label,
    });
    if let Some(err) = err
        && let Value::Object(map) = &mut value
    {
        map.insert("error_code".to_string(), json!(err.error_code));
        map.insert("error".to_string(), json!(err.message.as_str()));
        map.insert("retryable".to_string(), json!(err.retryable));
    }
    value
}

fn ensure_step_states(item: &mut PushItem, steps: &[ActionStep]) {
    for (index, step) in steps.iter().enumerate() {
        if !item.step_states.iter().any(|state| state.index == index) {
            item.step_states.push(PushStepState {
                index,
                label: step_label(step),
                status: PushStepStatus::Pending,
                started_rfc3339: None,
                completed_rfc3339: None,
                result_summary: None,
                error_code: None,
                error: None,
                retryable: None,
            });
        }
    }
    item.step_states.retain(|state| state.index < steps.len());
    item.step_states.sort_by_key(|state| state.index);
}

fn step_succeeded(item: &PushItem, index: usize) -> bool {
    item.step_states
        .iter()
        .any(|state| state.index == index && state.status == PushStepStatus::Succeeded)
}

fn step_remote_outcome_unknown(item: &PushItem, index: usize) -> bool {
    item.step_states
        .iter()
        .any(|state| state.index == index && state.status == PushStepStatus::RemoteOutcomeUnknown)
}

fn step_state_mut(item: &mut PushItem, index: usize) -> Option<&mut PushStepState> {
    item.step_states
        .iter_mut()
        .find(|state| state.index == index)
}

fn mark_step_pending(item: &mut PushItem, index: usize, step: &ActionStep) {
    freeze_message_action_snapshot(item);
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        state.label = step_label(step);
        state.status = PushStepStatus::Pending;
        state.started_rfc3339 = Some(now);
        state.completed_rfc3339 = None;
        state.result_summary = None;
        state.error_code = None;
        state.error = None;
        state.retryable = None;
    }
    item.updated_rfc3339 = crate::store::now_rfc3339();
}

fn freeze_message_action_snapshot(item: &mut PushItem) {
    let snapshot = item.planned_message_action.clone();
    let PushPayload::Message(message) = &mut item.payload else {
        return;
    };
    if message.message_action_snapshot.is_none() {
        message.message_action_snapshot = snapshot;
    }
}

fn mark_step_succeeded(item: &mut PushItem, index: usize, step: &ActionStep) {
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        state.label = step_label(step);
        state.status = PushStepStatus::Succeeded;
        if state.started_rfc3339.is_none() {
            state.started_rfc3339 = Some(now.clone());
        }
        state.completed_rfc3339 = Some(now);
        state.result_summary = Some(step_result_summary(step).to_string());
        state.error_code = None;
        state.error = None;
        state.retryable = None;
    }
}

fn mark_step_failed(item: &mut PushItem, index: usize, step: &ActionStep, err: &AppError) {
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        state.label = step_label(step);
        state.status = PushStepStatus::Failed;
        if state.started_rfc3339.is_none() {
            state.started_rfc3339 = Some(now.clone());
        }
        state.completed_rfc3339 = Some(now);
        state.result_summary = None;
        state.error_code = Some(err.error_code.to_string());
        state.error = Some(err.message.clone());
        state.retryable = Some(err.retryable);
    }
}

fn persist_remote_outcome_unknown(
    root: &Path,
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
) -> Result<()> {
    mark_step_remote_outcome_unknown(item, index, step, None);
    item.updated_rfc3339 = crate::store::now_rfc3339();
    item.last_error = None;
    write_item(root, item)
}

fn mark_step_remote_outcome_unknown(
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
    err: Option<&AppError>,
) {
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        state.label = step_label(step);
        state.status = PushStepStatus::RemoteOutcomeUnknown;
        if state.started_rfc3339.is_none() {
            state.started_rfc3339 = Some(now);
        }
        state.completed_rfc3339 = None;
        state.result_summary = Some("remote outcome unknown".to_string());
        state.error_code = err.map(|err| err.error_code.to_string());
        state.error = err.map(|err| err.message.clone());
        state.retryable = Some(false);
    }
}

fn recover_remote_outcome_unknown_step(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
) -> Result<bool> {
    if let Some(mailbox_id) = &step.append_to_mailbox_id {
        return recover_remote_outcome_unknown_append(context, item, index, step, mailbox_id);
    }
    if let Some(mailbox_id) = &step.move_to_mailbox_id
        && move_completed_before_target_uid_lookup_failed(item, index)
    {
        return recover_remote_outcome_unknown_move(context, item, index, step, mailbox_id);
    }
    Ok(false)
}

fn recover_remote_outcome_unknown_append(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
    mailbox_id: &str,
) -> Result<bool> {
    let folder = context.remote.action_mailbox_folder(mailbox_id)?;
    let message_ids = append_rfc822_message_ids(context.root, item)?;
    if message_ids.is_empty() {
        return Ok(false);
    }
    for message_id in &message_ids {
        match context.remote.find_by_message_id(&folder, message_id) {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(false),
            Err(err) if find_absence_is_nonfatal(&err) => return Ok(false),
            Err(err) => return Err(err),
        }
    }
    mark_step_succeeded(item, index, step);
    item.updated_rfc3339 = crate::store::now_rfc3339();
    item.last_error = None;
    write_item(context.root, item)?;
    Ok(true)
}

fn move_completed_before_target_uid_lookup_failed(item: &PushItem, index: usize) -> bool {
    item.step_states
        .iter()
        .find(|state| state.index == index)
        .is_some_and(|state| match state.error_code.as_deref() {
            Some("imap_move_target_uid_missing") => true,
            // Compatibility with 0.10.0 records. That version used the generic
            // code and this exact message only after UID MOVE had returned OK.
            Some("imap_uid_missing") => {
                state.error.as_deref() == Some("moved message uid was not found")
            }
            // A provider may report a completed move without a target UID. The
            // old execute path turned that known post-effect condition into the
            // generic push error below.
            Some("push_remote_outcome_unknown") => state.error.as_deref().is_some_and(|error| {
                error.starts_with("remote move completed without a target UID for message ")
            }),
            _ => false,
        })
}

fn recover_remote_outcome_unknown_move(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
    mailbox_id: &str,
) -> Result<bool> {
    let action_locations = action_locations(context.root, item, step)?;
    let mut message_ids = item.message_ids().iter().cloned().collect::<BTreeSet<_>>();
    message_ids.extend(
        action_locations
            .iter()
            .map(|location| location.message_id.clone()),
    );
    if message_ids.is_empty() {
        return Ok(false);
    }

    let workspace = crate::store::Workspace::at(context.root);
    let mut reconciled_sources = Vec::new();
    for message_id in message_ids {
        let message = workspace.read_message_by_id(&message_id)?;
        let source_locations = action_locations
            .iter()
            .filter(|location| location.message_id == message_id)
            .cloned()
            .collect::<Vec<_>>();
        // Finding the message in the target mailbox proves only that a target
        // copy exists. Recovery is safe after pull has also reconciled every
        // planned source location as missing; otherwise an earlier source may
        // have moved while a later one is still active.
        if has_active_planned_source(&message, &source_locations) {
            return Ok(false);
        }
        reconciled_sources.push((message_id, message, source_locations));
    }

    // Resolving a configured target may require an IMAP mailbox listing. Do it
    // only after local pull evidence proves that every original source is gone.
    let target_folder = context.remote.action_mailbox_folder(mailbox_id)?;
    let mut recovered = Vec::new();
    for (message_id, message, source_locations) in reconciled_sources {
        let rfc822_message_id = message.rfc822_message_id.clone().ok_or_else(|| {
            AppError::new(
                "rfc822_message_id_missing",
                format!("message cannot be verified after remote move: {message_id}"),
            )
        })?;
        let local_target = message.remote.as_ref().and_then(|remote| {
            remote
                .locations
                .iter()
                .find(|location| {
                    location.mailbox_name == target_folder && location.missing_rfc3339.is_none()
                })
                .cloned()
        });
        let target_location = match local_target {
            Some(location) => location,
            None => match context
                .remote
                .find_by_message_id(&target_folder, &rfc822_message_id)
            {
                Ok(Some(location)) => location,
                Ok(None) => return Ok(false),
                Err(err) if find_absence_is_nonfatal(&err) => return Ok(false),
                Err(err) => return Err(err),
            },
        };
        recovered.push((message_id, source_locations, target_location));
    }

    for (message_id, source_locations, target_location) in recovered {
        let message = workspace.read_message_by_id(&message_id)?;
        let mut locations = message
            .remote
            .map(|remote| {
                remote
                    .locations
                    .into_iter()
                    .filter(|location| location.missing_rfc3339.is_none())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        locations.retain(|existing| {
            !source_locations
                .iter()
                .any(|source| remote_location_matches_push(existing, source))
        });
        if !locations.iter().any(|existing| {
            existing.mailbox_name == target_location.mailbox_name
                && existing.uid_validity == target_location.uid_validity
                && existing.uid == target_location.uid
        }) {
            locations.push(target_location);
        }
        workspace.relocate_message(&message_id, &locations)?;
    }

    mark_step_succeeded(item, index, step);
    item.updated_rfc3339 = crate::store::now_rfc3339();
    item.last_error = None;
    write_item(context.root, item)?;
    Ok(true)
}

fn has_active_planned_source(message: &MessageFile, source_locations: &[PushLocation]) -> bool {
    message.remote.as_ref().is_some_and(|remote| {
        remote.locations.iter().any(|existing| {
            existing.missing_rfc3339.is_none()
                && source_locations
                    .iter()
                    .any(|source| remote_location_matches_push(existing, source))
        })
    })
}

fn remote_location_matches_push(existing: &RemoteLocation, planned: &PushLocation) -> bool {
    existing.mailbox_name == planned.mailbox_name
        && existing.uid_validity == Some(planned.uid_validity)
        && existing.uid == Some(planned.uid)
}

fn find_absence_is_nonfatal(err: &AppError) -> bool {
    matches!(
        err.error_code,
        "imap_uid_missing" | "remote_operation_unsupported"
    )
}

fn append_rfc822_message_ids(root: &Path, item: &PushItem) -> Result<Vec<String>> {
    if item.outbound().is_some() {
        return Ok(vec![outbound_rfc822_message_id(item)]);
    }
    let workspace = crate::store::Workspace::at(root);
    item.message_ids()
        .iter()
        .map(|message_id| {
            workspace
                .read_message_by_id(message_id)?
                .rfc822_message_id
                .ok_or_else(|| {
                    AppError::new(
                        "rfc822_message_id_missing",
                        format!("message cannot be verified after append: {message_id}"),
                    )
                })
        })
        .collect()
}

fn outbound_rfc822_message_id(item: &PushItem) -> String {
    format!(
        "<{}@afmail.local>",
        crate::smtp_send::message_id_for_push(&item.push_id)
    )
}

fn remote_outcome_unknown_error(
    root: &Path,
    item: &PushItem,
    index: usize,
    step: &ActionStep,
    cause: Option<&AppError>,
) -> AppError {
    let label = step_label(step);
    let mut details = json!({
        "push_id": item.push_id.as_str(),
        "step_index": index + 1,
        "label": label.as_str(),
        "step_status": "remote_outcome_unknown",
        "suggested_commands": [
            "afmail push list",
            "afmail doctor"
        ]
    });
    if let Some(mailbox_id) = step.append_to_mailbox_id.as_deref()
        && let Value::Object(map) = &mut details
    {
        map.insert("append_to_mailbox_id".to_string(), json!(mailbox_id));
        if let Ok(message_ids) = append_rfc822_message_ids(root, item) {
            map.insert("rfc822_message_ids".to_string(), json!(message_ids));
        }
    }
    if let Some(cause) = cause
        && let Value::Object(map) = &mut details
    {
        map.insert("cause_error_code".to_string(), json!(cause.error_code));
        map.insert("cause_error".to_string(), json!(cause.message.as_str()));
    }
    AppError::new(
        "push_remote_outcome_unknown",
        format!(
            "remote outcome is unknown for push step {label}; refusing to retry automatically"
        ),
    )
    .with_hint(
        "Inspect the remote mailbox or SMTP outcome before retrying. Append steps and moves whose target-UID lookup failed after a completed move can resume only after the target Message-ID is found.",
    )
    .with_details(details)
}

fn step_result_summary(step: &ActionStep) -> &'static str {
    if step.smtp_send.is_some() {
        "smtp_send succeeded"
    } else if step.append_to_mailbox_id.is_some() {
        "append succeeded"
    } else if step.move_to_mailbox_id.is_some() {
        "move succeeded"
    } else if !step.add_flags.is_empty() {
        "flags updated"
    } else {
        "noop succeeded"
    }
}

pub(super) fn execute_outbound_step(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
) -> Result<()> {
    item.outbound()
        .ok_or_else(|| AppError::new("push_item_invalid", "push item is not outbound"))?;
    let prepared = prepare_current_outbound(context.root, context.config, item)?;
    if step.smtp_send.is_some() {
        validate_smtp_send_attempt(context.config, &prepared)?;
        // Local construction first, and it fails the step outright. Only what
        // happens after this line could have delivered mail, so only what
        // happens after it deserves an unknown outcome.
        context
            .remote
            .prepare_send(&prepared.envelope_from, &prepared.envelope_to)?;
        persist_remote_outcome_unknown(context.root, item, index, step)?;
        context.remote.send_raw_message(
            &prepared.envelope_from,
            &prepared.envelope_to,
            &prepared.delivery_raw,
        )?;
    }
    if let Some(mailbox_id) = &step.append_to_mailbox_id {
        let folder = context.remote.action_mailbox_folder(mailbox_id)?;
        let draft = mailbox_is_kind(context.config, mailbox_id, SpecialUseKind::Drafts);
        persist_remote_outcome_unknown(context.root, item, index, step)?;
        context
            .remote
            .append_message(&folder, &prepared.raw, draft)?;
    }
    if !step.add_flags.is_empty() {
        let locations = action_locations(context.root, item, step)?;
        execute_add_flags(context.root, context.remote, &locations, &step.add_flags)?;
    }
    if step.move_to_mailbox_id.is_some() {
        execute_message_step(context, item, index, step)?;
    }
    Ok(())
}

fn validate_smtp_send_attempt(
    config: &MailConfig,
    prepared: &crate::types::PreparedOutbound,
) -> Result<()> {
    config.require_smtp()?;
    prepared
        .envelope_from
        .parse::<lettre::address::Address>()
        .map_err(|e| AppError::new("smtp_send_failed", format!("invalid envelope from: {e}")))?;
    for address in &prepared.envelope_to {
        address.parse::<lettre::address::Address>().map_err(|e| {
            AppError::new(
                "smtp_send_failed",
                format!("invalid envelope recipient: {e}"),
            )
        })?;
    }
    Ok(())
}

pub(super) fn execute_message_step(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
) -> Result<()> {
    if !step.add_flags.is_empty() {
        let locations = action_locations(context.root, item, step)?;
        execute_add_flags(context.root, context.remote, &locations, &step.add_flags)?;
    }
    if let Some(mailbox_id) = &step.move_to_mailbox_id {
        let locations = action_locations(context.root, item, step)?;
        let target = context.remote.action_mailbox_folder(mailbox_id)?;
        if locations
            .iter()
            .any(|location| location.mailbox_name != target)
        {
            persist_remote_outcome_unknown(context.root, item, index, step)?;
        }
        execute_move(context.root, context.remote, item, &locations, mailbox_id)?;
    }
    if let Some(mailbox_id) = &step.append_to_mailbox_id {
        let folder = context.remote.action_mailbox_folder(mailbox_id)?;
        let mut marked_unknown = false;
        let message_ids = item.message_ids().to_vec();
        for message_id in message_ids {
            let raw = match item.planned_message_rfc822.get(&message_id) {
                Some(raw) => raw.clone(),
                None => fs::read(
                    context
                        .root
                        .join(format!(".afmail/messages/{message_id}.eml")),
                )
                .map_err(|e| AppError::io("read message eml", &e))?,
            };
            if !marked_unknown {
                persist_remote_outcome_unknown(context.root, item, index, step)?;
                marked_unknown = true;
            }
            context.remote.append_message(&folder, &raw, false)?;
        }
    }
    if step.smtp_send.is_some() {
        return Err(AppError::new(
            "push_item_invalid",
            "smtp_send is only valid for outbound push items",
        ));
    }
    Ok(())
}

pub(super) fn action_locations(
    root: &Path,
    item: &PushItem,
    step: &ActionStep,
) -> Result<Vec<PushLocation>> {
    if step.on == Some(ActionStepOn::ReplyToMessage) {
        let reply_to = reply_to_message_id_for_step(root, item)?;
        let workspace = crate::store::Workspace::at(root);
        return workspace.message_remote_locations_any(&[reply_to]);
    }
    Ok(item.locations().to_vec())
}

fn reply_to_message_id_for_step(root: &Path, item: &PushItem) -> Result<String> {
    if let Some(reply_to) = item.reply_to_message_id() {
        return Ok(reply_to.to_string());
    }
    let outbound = item.outbound().ok_or_else(|| {
        AppError::new(
            "push_item_invalid",
            "reply_to_message action has no reply_to_message_id",
        )
    })?;
    let case_path = find_case_path(root, &outbound.case_uid)?;
    let draft_path = case_path.join("drafts").join(&outbound.draft_name);
    ensure_draft_file_exists(root, &outbound.case_uid, &outbound.draft_name, &draft_path)?;
    let draft_text = fs::read_to_string(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
    let (frontmatter, _) = crate::markdown::read_doc::<DraftFrontmatter>(&draft_text)?;
    frontmatter.reply_to_message_id.ok_or_else(|| {
        AppError::new(
            "push_item_invalid",
            "reply_to_message action has no reply_to_message_id",
        )
    })
}

pub(super) fn execute_add_flags(
    root: &Path,
    remote: &dyn MailRemote,
    locations: &[PushLocation],
    flags: &[String],
) -> Result<()> {
    if locations.is_empty() {
        return Ok(());
    }
    let workspace = crate::store::Workspace::at(root);
    for location in locations {
        remote.add_flags(
            &location.mailbox_name,
            location.uid_validity,
            location.uid,
            flags,
        )?;
        workspace.add_remote_flags(std::slice::from_ref(location), flags)?;
    }
    Ok(())
}

pub(super) fn execute_move(
    root: &Path,
    remote: &dyn MailRemote,
    item: &PushItem,
    locations: &[PushLocation],
    mailbox_id: &str,
) -> Result<()> {
    if locations.is_empty() {
        return Ok(());
    }
    let workspace = crate::store::Workspace::at(root);
    let mut rfc822_ids = BTreeMap::new();
    let mut active_locations: BTreeMap<String, Vec<crate::types::RemoteLocation>> = BTreeMap::new();
    for message_id in item.message_ids() {
        if active_locations.contains_key(message_id) {
            continue;
        }
        let message = workspace.read_message_by_id(message_id)?;
        let Some(rfc822_message_id) = message.rfc822_message_id else {
            return Err(AppError::new(
                "rfc822_message_id_missing",
                format!(
                    "message cannot be relocated after remote move without Message-ID: {}",
                    message_id
                ),
            ));
        };
        rfc822_ids.insert(message_id.clone(), rfc822_message_id);
        active_locations.insert(
            message_id.clone(),
            message
                .remote
                .map(|remote| {
                    remote
                        .locations
                        .into_iter()
                        .filter(|location| location.missing_rfc3339.is_none())
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
    let target = remote.action_mailbox_folder(mailbox_id)?;
    for location in locations {
        if location.mailbox_name == target {
            continue;
        }
        let rfc822_message_id = rfc822_ids
            .get(&location.message_id)
            .ok_or_else(|| AppError::new("rfc822_message_id_missing", "Message-ID missing"))?;
        let outcome = remote.move_message(
            &location.mailbox_name,
            location.uid_validity,
            location.uid,
            &target,
            Some(rfc822_message_id.as_str()),
        )?;
        let target_location = outcome.target_location.ok_or_else(|| {
            AppError::new(
                "imap_move_target_uid_missing",
                format!(
                    "remote move succeeded but the target UID was not returned for message {}",
                    location.message_id
                ),
            )
            .with_hint(
                "Run `afmail pull`, then preview and confirm the push again. Recovery verifies the target message without replaying the move.",
            )
            .with_details(json!({
                "message_id": location.message_id.as_str(),
                "target_mailbox_name": target.as_str(),
                "remote_effect_completed": true,
            }))
        })?;
        let message_locations =
            active_locations
                .get_mut(&location.message_id)
                .ok_or_else(|| {
                    AppError::new(
                        "message_not_found",
                        format!(
                            "message not found during remote relocation: {}",
                            location.message_id
                        ),
                    )
                })?;
        message_locations.retain(|existing| {
            !(existing.mailbox_name == location.mailbox_name
                && existing.uid_validity == Some(location.uid_validity)
                && existing.uid == Some(location.uid))
        });
        if !message_locations.iter().any(|existing| {
            existing.mailbox_name == target_location.mailbox_name
                && existing.uid_validity == target_location.uid_validity
                && existing.uid == target_location.uid
        }) {
            message_locations.push(target_location);
        }
        workspace.relocate_message(&location.message_id, message_locations)?;
    }
    Ok(())
}

pub(super) fn mailbox_is_kind(config: &MailConfig, mailbox_id: &str, kind: SpecialUseKind) -> bool {
    config
        .mailbox(mailbox_id)
        .ok()
        .is_some_and(|mailbox| mailbox.contains_special_use(kind))
        || mailbox_id == kind.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        DraftPush, MessageActionPush, MessagePush, OutboundPush, PushKind, PushPayload,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "afmail-push-execute-{name}-{}-{suffix}",
            std::process::id()
        ))
    }

    fn outbound_item(push_id: &str, step_states: Vec<PushStepState>) -> PushItem {
        PushItem {
            schema_name: "push_item".to_string(),
            schema_version: PUSH_ITEM_SCHEMA_VERSION,
            push_id: push_id.to_string(),
            enqueue_seq: 1,
            payload: PushPayload::Draft(Box::new(DraftPush {
                case_uid: "c20260609001".to_string(),
                draft_name: "reply.md".to_string(),
                draft_uid_validity: None,
                draft_uid: None,
            })),
            planned_outbound: Some(OutboundPush {
                action: OutboundAction::Send,
                case_uid: "c20260609001".to_string(),
                draft_name: "reply.md".to_string(),
                draft_uid_validity: None,
                draft_uid: None,
                steps: Vec::new(),
                reply_to_message_id: None,
            }),
            planned_outbound_message: None,
            planned_message_action: None,
            planned_message_rfc822: BTreeMap::new(),
            created_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            updated_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            attempt_count: 0,
            step_states,
            local_stage: None,
            audit_pending: None,
            last_error: None,
        }
    }

    fn message_action_item(
        push_id: &str,
        action: MessagePushAction,
        message_ids: Vec<String>,
        locations: Vec<PushLocation>,
    ) -> PushItem {
        PushItem {
            schema_name: "push_item".to_string(),
            schema_version: PUSH_ITEM_SCHEMA_VERSION,
            push_id: push_id.to_string(),
            enqueue_seq: 1,
            payload: PushPayload::Message(MessagePush {
                message_id: message_ids.first().cloned().unwrap_or_default(),
                action,
                message_action_snapshot: None,
            }),
            planned_outbound: None,
            planned_outbound_message: None,
            planned_message_action: Some(MessageActionPush {
                action,
                message_ids,
                locations,
                steps: Vec::new(),
                reply_to_message_id: None,
            }),
            planned_message_rfc822: BTreeMap::new(),
            created_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            updated_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            attempt_count: 0,
            step_states: Vec::new(),
            local_stage: None,
            audit_pending: None,
            last_error: None,
        }
    }

    fn step_state(index: usize, step: &ActionStep, status: PushStepStatus) -> PushStepState {
        PushStepState {
            index,
            label: step_label(step),
            status,
            started_rfc3339: Some("2026-06-09T00:00:00Z".to_string()),
            completed_rfc3339: None,
            result_summary: None,
            error_code: None,
            error: None,
            retryable: None,
        }
    }

    fn write_message_json(root: &Path, message_id: &str, rfc822_message_id: &str, uid: u64) {
        assert!(fs::create_dir_all(root.join("messages")).is_ok());
        assert!(fs::create_dir_all(root.join(".afmail")).is_ok());
        assert!(
            crate::util::write_json_pretty(
                &root.join(".afmail/config.json"),
                &MailConfig::default(),
            )
            .is_ok()
        );
        let path = root.join(format!("messages/{message_id}.json"));
        assert!(
            crate::util::write_json_pretty(
                &path,
                &json!({
                    "schema_name": "message",
                    "schema_version": crate::types::MESSAGE_SCHEMA_VERSION,
                    "message_id": message_id,
                    "rfc822_message_id": rfc822_message_id,
                    "remote": {
                        "locations": [{
                            "mailbox_name": "INBOX",
                            "uid_validity": 44,
                            "uid": uid,
                            "flags": [],
                            "observed_rfc3339": "2026-06-09T00:00:00Z"
                        }]
                    },
                    "workspace": {
                        "status": "triage"
                    }
                }),
            )
            .is_ok()
        );
    }

    fn context<'a>(
        root: &'a Path,
        config: &'a MailConfig,
        remote: &'a dyn MailRemote,
    ) -> PushExecutionContext<'a> {
        PushExecutionContext {
            root,
            config,
            remote,
            progress: PushProgressContext {
                item_index: 0,
                item_count: 1,
            },
        }
    }

    #[test]
    fn pending_message_step_persists_the_exact_action_snapshot() {
        let step = ActionStep::move_to_mailbox_id("archive");
        let mut item = message_action_item(
            "push_snapshot",
            MessagePushAction::Notification,
            vec!["message_snapshot".to_string()],
            vec![PushLocation {
                message_id: "message_snapshot".to_string(),
                mailbox_name: "INBOX".to_string(),
                uid_validity: 44,
                uid: 42,
            }],
        );
        if let Some(action) = item.planned_message_action.as_mut() {
            action.steps = vec![step.clone()];
        }
        ensure_step_states(&mut item, std::slice::from_ref(&step));

        mark_step_pending(&mut item, 0, &step);

        let encoded = serde_json::to_string(&item);
        assert!(encoded.is_ok());
        let encoded = encoded.unwrap_or_default();
        let persisted = PushItem::parse_json(&encoded);
        assert!(persisted.is_ok());
        let Some(persisted) = persisted.ok() else {
            return;
        };
        assert_eq!(
            persisted
                .message()
                .and_then(|message| message.message_action_snapshot.as_ref()),
            item.planned_message_action.as_ref()
        );
    }

    #[test]
    fn remote_outcome_unknown_smtp_step_refuses_retry() {
        let root = temp_root("smtp-unknown");
        let config = MailConfig::default();
        let remote = crate::remote::FakeMailRemote::default();
        let step = ActionStep::smtp_send();
        let steps = vec![step.clone()];
        let mut item = outbound_item(
            "push_smtp_unknown",
            vec![step_state(0, &step, PushStepStatus::RemoteOutcomeUnknown)],
        );
        let mut called = false;

        let result = execute_tracked_steps(
            context(&root, &config, &remote),
            &mut item,
            &steps,
            None,
            |_context, _item, _index, _step| {
                called = true;
                Ok(())
            },
        );

        assert_eq!(
            result.err().map(|err| err.error_code),
            Some("push_remote_outcome_unknown")
        );
        assert!(!called);
        assert_eq!(
            item.step_states[0].status,
            PushStepStatus::RemoteOutcomeUnknown
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_outcome_unknown_append_recovers_when_message_id_is_found() {
        let root = temp_root("append-found");
        let config = MailConfig::default();
        let remote = crate::remote::FakeMailRemote::default();
        let step = ActionStep::append_to_mailbox_id("sent");
        let steps = vec![step.clone()];
        let mut item = outbound_item(
            "push_append_found",
            vec![step_state(0, &step, PushStepStatus::RemoteOutcomeUnknown)],
        );
        let mut called = false;

        let result = execute_tracked_steps(
            context(&root, &config, &remote),
            &mut item,
            &steps,
            None,
            |_context, _item, _index, _step| {
                called = true;
                Ok(())
            },
        );

        assert!(result.is_ok());
        assert!(!called);
        assert_eq!(item.step_states[0].status, PushStepStatus::Succeeded);
        assert_eq!(
            item.step_states[0].result_summary.as_deref(),
            Some("append succeeded")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_outcome_unknown_move_recovers_target_without_replaying_move() {
        let root = temp_root("move-target-found");
        write_message_json(
            &root,
            "message_move_recovery",
            "<move-recovery@example.com>",
            7,
        );
        let message_path = root.join("messages/message_move_recovery.json");
        let mut message = fs::read_to_string(&message_path)
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations[0]["missing_rfc3339"] = json!("2026-06-09T00:00:01Z");
            locations.push(json!({
                "mailbox_name": "archive",
                "mailbox_id": "archive",
                "uid_validity": 55,
                "uid": 91,
                "flags": [],
                "observed_rfc3339": "2026-06-09T00:00:01Z"
            }));
        }
        assert!(crate::util::write_json_pretty(&message_path, &message).is_ok());
        let config = MailConfig::default();
        let remote = crate::remote::FakeMailRemote::default();
        let step = ActionStep::move_to_mailbox_id("archive");
        let locations = vec![PushLocation {
            message_id: "message_move_recovery".to_string(),
            mailbox_name: "INBOX".to_string(),
            uid_validity: 44,
            uid: 7,
        }];
        let mut item = message_action_item(
            "push_move_recovery",
            MessagePushAction::Notification,
            vec!["message_move_recovery".to_string()],
            locations,
        );
        if let Some(action) = item.planned_message_action.as_mut() {
            action.steps = vec![step.clone()];
        }
        let mut state = step_state(0, &step, PushStepStatus::RemoteOutcomeUnknown);
        state.error_code = Some("imap_move_target_uid_missing".to_string());
        state.error =
            Some("remote move succeeded but the target message UID was not found".to_string());
        state.retryable = Some(false);
        item.step_states = vec![state];

        let recovered = recover_remote_outcome_unknown_step(
            context(&root, &config, &remote),
            &mut item,
            0,
            &step,
        );

        assert_eq!(recovered, Ok(true));
        assert_eq!(item.step_states[0].status, PushStepStatus::Succeeded);
        assert!(remote.move_requests.borrow().is_empty());
        let message = fs::read_to_string(root.join("messages/message_move_recovery.json"))
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        assert_eq!(
            message["remote"]["locations"].as_array().map(Vec::len),
            Some(1),
            "{message}"
        );
        assert_eq!(message["remote"]["locations"][0]["mailbox_name"], "archive");
        assert_eq!(message["remote"]["locations"][0]["uid"], 91);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_outcome_unknown_move_waits_while_a_planned_source_is_still_active() {
        let root = temp_root("move-source-still-active");
        write_message_json(
            &root,
            "message_move_partial",
            "<move-partial@example.com>",
            7,
        );
        let message_path = root.join("messages/message_move_partial.json");
        let mut message = fs::read_to_string(&message_path)
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations[0]["missing_rfc3339"] = json!("2026-06-09T00:00:01Z");
            locations.push(json!({
                "mailbox_name": "Secondary",
                "mailbox_id": "secondary",
                "uid_validity": 45,
                "uid": 8,
                "flags": [],
                "observed_rfc3339": "2026-06-09T00:00:00Z"
            }));
            locations.push(json!({
                "mailbox_name": "archive",
                "mailbox_id": "archive",
                "uid_validity": 55,
                "uid": 92,
                "flags": [],
                "observed_rfc3339": "2026-06-09T00:00:01Z"
            }));
        }
        assert!(crate::util::write_json_pretty(&message_path, &message).is_ok());

        let config = MailConfig::default();
        let remote = crate::remote::FakeMailRemote::default();
        let step = ActionStep::move_to_mailbox_id("archive");
        let mut item = message_action_item(
            "push_move_partial",
            MessagePushAction::Notification,
            vec!["message_move_partial".to_string()],
            vec![
                PushLocation {
                    message_id: "message_move_partial".to_string(),
                    mailbox_name: "INBOX".to_string(),
                    uid_validity: 44,
                    uid: 7,
                },
                PushLocation {
                    message_id: "message_move_partial".to_string(),
                    mailbox_name: "Secondary".to_string(),
                    uid_validity: 45,
                    uid: 8,
                },
            ],
        );
        if let Some(action) = item.planned_message_action.as_mut() {
            action.steps = vec![step.clone()];
        }
        let mut state = step_state(0, &step, PushStepStatus::RemoteOutcomeUnknown);
        state.error_code = Some("imap_move_target_uid_missing".to_string());
        state.error =
            Some("remote move succeeded but the target message UID was not found".to_string());
        state.retryable = Some(false);
        item.step_states = vec![state];

        let recovered = recover_remote_outcome_unknown_step(
            context(&root, &config, &remote),
            &mut item,
            0,
            &step,
        );

        assert_eq!(recovered, Ok(false));
        assert_eq!(
            item.step_states[0].status,
            PushStepStatus::RemoteOutcomeUnknown
        );
        assert!(remote.move_requests.borrow().is_empty());
        let unchanged = fs::read_to_string(&message_path)
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        assert_eq!(
            unchanged["remote"]["locations"].as_array().map(Vec::len),
            Some(3),
            "{unchanged}"
        );
        assert!(
            unchanged["remote"]["locations"]
                .as_array()
                .is_some_and(|locations| locations.iter().any(|location| {
                    location["mailbox_name"] == "Secondary"
                        && location.get("missing_rfc3339").is_none()
                }))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_outcome_unknown_move_does_not_recover_an_ambiguous_failure() {
        let root = temp_root("move-still-ambiguous");
        write_message_json(
            &root,
            "message_move_ambiguous",
            "<move-ambiguous@example.com>",
            8,
        );
        let path = root.join("messages/message_move_ambiguous.json");
        let mut message = fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations.push(json!({
                "mailbox_name": "archive",
                "mailbox_id": "archive",
                "uid_validity": 55,
                "uid": 92,
                "flags": [],
                "observed_rfc3339": "2026-06-09T00:00:01Z"
            }));
        }
        assert!(crate::util::write_json_pretty(&path, &message).is_ok());
        let config = MailConfig::default();
        let remote = crate::remote::FakeMailRemote::default();
        let step = ActionStep::move_to_mailbox_id("archive");
        let mut item = message_action_item(
            "push_move_ambiguous",
            MessagePushAction::Notification,
            vec!["message_move_ambiguous".to_string()],
            vec![PushLocation {
                message_id: "message_move_ambiguous".to_string(),
                mailbox_name: "INBOX".to_string(),
                uid_validity: 44,
                uid: 8,
            }],
        );
        if let Some(action) = item.planned_message_action.as_mut() {
            action.steps = vec![step.clone()];
        }
        let mut state = step_state(0, &step, PushStepStatus::RemoteOutcomeUnknown);
        state.error_code = Some("imap_move_failed".to_string());
        state.error = Some("connection closed before MOVE completed".to_string());
        state.retryable = Some(false);
        item.step_states = vec![state];

        let recovered = recover_remote_outcome_unknown_step(
            context(&root, &config, &remote),
            &mut item,
            0,
            &step,
        );

        assert_eq!(recovered, Ok(false));
        assert_eq!(
            item.step_states[0].status,
            PushStepStatus::RemoteOutcomeUnknown
        );
        assert!(remote.move_requests.borrow().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn irreversible_remote_error_preserves_unknown_status() {
        let root = temp_root("remote-error");
        let config = MailConfig::default();
        let remote = crate::remote::FakeMailRemote::default();
        let step = ActionStep::smtp_send();
        let steps = vec![step.clone()];
        let mut item = outbound_item("push_remote_error", Vec::new());

        let result = execute_tracked_steps(
            context(&root, &config, &remote),
            &mut item,
            &steps,
            None,
            |context, item, index, step| {
                persist_remote_outcome_unknown(context.root, item, index, step)?;
                let data = fs::read_to_string(push_path(context.root, &item.push_id))
                    .map_err(|e| AppError::io("read push item", &e))?;
                let persisted = PushItem::parse_json(&data)?;
                assert_eq!(
                    persisted.step_states[0].status,
                    PushStepStatus::RemoteOutcomeUnknown
                );
                Err(AppError::new("smtp_send_failed", "simulated send error"))
            },
        );

        assert_eq!(
            result.err().map(|err| err.error_code),
            Some("push_remote_outcome_unknown")
        );
        assert_eq!(
            item.step_states[0].status,
            PushStepStatus::RemoteOutcomeUnknown
        );
        assert_eq!(
            item.step_states[0].error_code.as_deref(),
            Some("smtp_send_failed")
        );
        assert_eq!(item.push_kind(), PushKind::Draft);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn move_relocates_each_message_immediately_after_remote_success() {
        let root = temp_root("move-partial");
        write_message_json(&root, "message_one", "<one@example.com>", 1);
        write_message_json(&root, "message_two", "<two@example.com>", 2);
        let remote = crate::remote::FakeMailRemote::default();
        remote
            .move_results
            .borrow_mut()
            .push_back(Ok(crate::imap_client::MoveOutcome {
                keyword_set: false,
                keyword_error: None,
                seen_set: false,
                seen_error: None,
                moved: true,
                target_location: Some(crate::types::RemoteLocation {
                    mailbox_name: "junk".to_string(),
                    mailbox_id: None,
                    uid_validity: Some(44),
                    uid: Some(101),
                    flags: Vec::new(),
                    observed_rfc3339: "2026-06-09T00:00:01Z".to_string(),
                    missing_rfc3339: None,
                }),
            }));
        remote
            .move_results
            .borrow_mut()
            .push_back(Err(AppError::new(
                "imap_move_failed",
                "simulated second move failure",
            )));
        let item = PushItem {
            schema_name: "push_item".to_string(),
            schema_version: PUSH_ITEM_SCHEMA_VERSION,
            push_id: "push_move_partial".to_string(),
            enqueue_seq: 1,
            payload: PushPayload::Message(MessagePush {
                message_id: "message_one".to_string(),
                action: MessagePushAction::Spam,
                message_action_snapshot: None,
            }),
            planned_outbound: None,
            planned_outbound_message: None,
            planned_message_action: Some(MessageActionPush {
                action: MessagePushAction::Spam,
                message_ids: vec!["message_one".to_string(), "message_two".to_string()],
                locations: vec![
                    PushLocation {
                        message_id: "message_one".to_string(),
                        mailbox_name: "INBOX".to_string(),
                        uid_validity: 44,
                        uid: 1,
                    },
                    PushLocation {
                        message_id: "message_two".to_string(),
                        mailbox_name: "INBOX".to_string(),
                        uid_validity: 44,
                        uid: 2,
                    },
                ],
                steps: vec![ActionStep::move_to_mailbox_id("junk")],
                reply_to_message_id: None,
            }),
            planned_message_rfc822: BTreeMap::new(),
            created_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            updated_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            attempt_count: 0,
            step_states: Vec::new(),
            local_stage: None,
            audit_pending: None,
            last_error: None,
        };
        let locations = item.locations().to_vec();

        let result = execute_move(&root, &remote, &item, &locations, "junk");

        assert!(result.is_err(), "move unexpectedly succeeded");
        let error = result.err();
        assert_eq!(
            error.as_ref().map(|err| err.error_code),
            Some("imap_move_failed"),
            "{error:?}"
        );
        let first = fs::read_to_string(root.join("messages/message_one.json"))
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        assert_eq!(
            first["remote"]["locations"][0]["mailbox_name"],
            json!("junk")
        );
        assert_eq!(first["remote"]["locations"][0]["uid"], json!(101));
        let second = fs::read_to_string(root.join("messages/message_two.json"))
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        assert_eq!(
            second["remote"]["locations"][0]["mailbox_name"],
            json!("INBOX")
        );
        assert_eq!(second["remote"]["locations"][0]["uid"], json!(2));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn move_skips_only_the_location_already_in_target_mailbox() {
        let root = temp_root("move-multiple-locations");
        write_message_json(&root, "message_multiple", "<multiple@example.com>", 1);
        let path = root.join("messages/message_multiple.json");
        let mut message = fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations.push(json!({
                "mailbox_name": "Archive",
                "uid_validity": 44,
                "uid": 2,
                "flags": [],
                "observed_rfc3339": "2026-06-09T00:00:00Z"
            }));
        }
        assert!(crate::util::write_json_pretty(&path, &message).is_ok());
        let locations = vec![
            PushLocation {
                message_id: "message_multiple".to_string(),
                mailbox_name: "INBOX".to_string(),
                uid_validity: 44,
                uid: 1,
            },
            PushLocation {
                message_id: "message_multiple".to_string(),
                mailbox_name: "Archive".to_string(),
                uid_validity: 44,
                uid: 2,
            },
        ];
        let item = message_action_item(
            "push_move_multiple",
            MessagePushAction::CaseAdd,
            vec!["message_multiple".to_string()],
            locations.clone(),
        );
        let remote = crate::remote::FakeMailRemote::default();

        let result = execute_move(&root, &remote, &item, &locations, "Archive");

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            remote.move_requests.borrow().as_slice(),
            &[("INBOX".to_string(), 44, 1, "Archive".to_string())]
        );
        let message = fs::read_to_string(&path)
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        let locations = message["remote"]["locations"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(locations.len(), 2, "{message}");
        assert!(
            locations
                .iter()
                .all(|location| location["mailbox_name"] == "Archive"),
            "{message}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn add_flags_persists_each_success_before_a_later_failure() {
        let root = temp_root("flags-partial");
        write_message_json(&root, "message_one", "<one@example.com>", 1);
        write_message_json(&root, "message_two", "<two@example.com>", 2);
        let remote = crate::remote::FakeMailRemote::default();
        remote.add_flags_results.borrow_mut().extend([
            Ok(()),
            Err(AppError::new(
                "imap_store_failed",
                "simulated second flag failure",
            )),
        ]);
        let locations = vec![
            PushLocation {
                message_id: "message_one".to_string(),
                mailbox_name: "INBOX".to_string(),
                uid_validity: 44,
                uid: 1,
            },
            PushLocation {
                message_id: "message_two".to_string(),
                mailbox_name: "INBOX".to_string(),
                uid_validity: 44,
                uid: 2,
            },
        ];

        let result = execute_add_flags(&root, &remote, &locations, &["\\Seen".to_string()]);

        assert_eq!(
            result.err().map(|error| error.error_code),
            Some("imap_store_failed")
        );
        let first = fs::read_to_string(root.join("messages/message_one.json"))
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        let second = fs::read_to_string(root.join("messages/message_two.json"))
            .ok()
            .and_then(|data| serde_json::from_str::<Value>(&data).ok())
            .unwrap_or(Value::Null);
        assert_eq!(first["remote"]["locations"][0]["flags"], json!(["\\Seen"]));
        assert_eq!(second["remote"]["locations"][0]["flags"], json!([]));
        assert_eq!(
            remote.add_flags_requests.borrow()[0],
            ("INBOX".to_string(), 44, 1, vec!["\\Seen".to_string()])
        );
        let _ = fs::remove_dir_all(root);
    }
}
