use super::*;
use crate::remote::MailRemote;

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

pub(super) fn push_outbound_drafts(
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
    let steps = if outbound.draft_save_steps.is_empty() {
        vec![ActionStep::append_to_mailbox_id("drafts")]
    } else {
        outbound.draft_save_steps.clone()
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

pub(super) fn push_outbound_send(
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
    let steps = if outbound.draft_send_steps.is_empty() {
        vec![
            ActionStep::smtp_send(),
            ActionStep::append_to_mailbox_id("sent"),
            ActionStep::add_flags_on(
                vec!["\\Seen".to_string(), "\\Answered".to_string()],
                ActionStepOn::ReplyToMessage,
            ),
        ]
    } else {
        outbound.draft_send_steps.clone()
    };
    let steps = filter_reply_to_steps(root, item, steps)?;
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
    if outbound.reply_to_message_id.is_none() {
        return Ok(true);
    }
    let case_uid = outbound.case_uid.as_str();
    let draft_name = outbound.draft_name.as_str();
    let Some(case_path) = find_case_path_any(root, case_uid)? else {
        return Ok(false);
    };
    let draft_path = case_path.join("drafts").join(draft_name);
    if !draft_path.is_file() {
        return Ok(false);
    }
    let draft_text = fs::read_to_string(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
    let (frontmatter, _) = crate::markdown::read_doc::<DraftFrontmatter>(&draft_text)?;
    Ok(frontmatter.send_intent.as_deref() == Some("new")
        || frontmatter.reply_to_message_id.is_none())
}

pub(super) fn ensure_outbound_draft_fresh(root: &Path, item: &PushItem) -> Result<()> {
    let outbound = item
        .outbound()
        .ok_or_else(|| AppError::new("invalid_request", "push item is not outbound"))?;
    let case_uid = outbound.case_uid.as_str();
    let draft_name = outbound.draft_name.as_str();
    let Some(case_path) = find_case_path_any(root, case_uid)? else {
        return Ok(());
    };
    let draft_path = case_path.join("drafts").join(draft_name);
    if !draft_path.is_file() {
        return Ok(());
    }
    let expected = outbound.draft_hash.as_str();
    let current = crate::util::file_sha256_fingerprint(&draft_path, "read draft")?;
    if current != expected {
        return Err(AppError::new(
            "draft_changed_since_compose",
            format!("draft changed since compose: {draft_name}"),
        )
        .with_hint(format!(
            "Re-run `afmail case draft validate {case_uid} {draft_name}`, then `afmail case compose {case_uid} {draft_name}`."
        ))
        .with_details(serde_json::json!({
            "case_uid": case_uid,
            "draft_name": draft_name,
            "suggested_commands": [
                format!("afmail case draft validate {case_uid} {draft_name}"),
                format!("afmail case compose {case_uid} {draft_name}")
            ]
        })));
    }
    Ok(())
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
        MessagePushAction::Archive => {
            workspace.ensure_archive_eligible(&payload.message_ids, Some(&current_push_path))?;
        }
        MessagePushAction::Spam | MessagePushAction::Trash => {
            workspace
                .ensure_message_ids_unreferenced(&payload.message_ids, Some(&current_push_path))?;
        }
        MessagePushAction::CaseAdd => {}
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
    mut execute: impl FnMut(PushExecutionContext<'_>, &mut PushItem, &ActionStep) -> Result<()>,
) -> Result<()> {
    let mut progress = progress;
    ensure_step_states(item, steps);
    for (index, step) in steps.iter().enumerate() {
        if step_succeeded(item, index) {
            continue;
        }
        let label = step_label(step);
        crate::progress::emit(
            &mut progress,
            "push_step_start",
            push_step_progress_fields(item, context.progress, index, steps.len(), &label, None),
        );
        mark_step_pending(item, index, step);
        write_item(context.root, item)?;
        match execute(context, item, step) {
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
            }
            Err(err) => {
                mark_step_failed(item, index, step, &err);
                item.updated_rfc3339 = crate::store::now_rfc3339();
                item.last_error = Some(err.to_string());
                write_item(context.root, item)?;
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
    if let Some(err) = err {
        if let Value::Object(map) = &mut value {
            map.insert("error_code".to_string(), json!(err.error_code));
            map.insert("error".to_string(), json!(err.message.as_str()));
            map.insert("retryable".to_string(), json!(err.retryable));
        }
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

fn step_state_mut(item: &mut PushItem, index: usize) -> Option<&mut PushStepState> {
    item.step_states
        .iter_mut()
        .find(|state| state.index == index)
}

fn mark_step_pending(item: &mut PushItem, index: usize, step: &ActionStep) {
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
    step: &ActionStep,
) -> Result<()> {
    let outbound = item
        .outbound()
        .ok_or_else(|| AppError::new("push_item_invalid", "push item is not outbound"))?
        .clone();
    if step.smtp_send.is_some() {
        let raw = read_item_eml(context.root, item)?;
        context
            .remote
            .send_raw_message(&outbound.envelope_from, &outbound.envelope_to, &raw)?;
    }
    if let Some(mailbox_id) = &step.append_to_mailbox_id {
        let raw = read_item_eml(context.root, item)?;
        let folder = context.remote.action_mailbox_folder(mailbox_id)?;
        let draft = mailbox_is_kind(context.config, mailbox_id, SpecialUseKind::Drafts);
        context.remote.append_message(&folder, &raw, draft)?;
        if draft {
            crate::smtp_send::mark_staged(
                context.root,
                &outbound.message_id,
                &raw,
                &outbound.case_uid,
            )?;
        } else if mailbox_is_kind(context.config, mailbox_id, SpecialUseKind::Sent) {
            let case_path = find_case_path(context.root, &outbound.case_uid)?;
            crate::smtp_send::mark_sent_and_append_case(
                context.root,
                &case_path,
                &outbound.case_uid,
                &outbound.message_id,
                &raw,
                context.config,
            )?;
        }
    }
    if !step.add_flags.is_empty() {
        let locations = action_locations(context.root, item, step)?;
        execute_add_flags(context.root, context.remote, &locations, &step.add_flags)?;
    }
    if step.move_to_mailbox_id.is_some() {
        execute_message_step(context, item, step)?;
    }
    Ok(())
}

pub(super) fn execute_message_step(
    context: PushExecutionContext<'_>,
    item: &mut PushItem,
    step: &ActionStep,
) -> Result<()> {
    if !step.add_flags.is_empty() {
        let locations = action_locations(context.root, item, step)?;
        execute_add_flags(context.root, context.remote, &locations, &step.add_flags)?;
    }
    if let Some(mailbox_id) = &step.move_to_mailbox_id {
        let locations = action_locations(context.root, item, step)?;
        execute_move(context.root, context.remote, item, &locations, mailbox_id)?;
    }
    if let Some(mailbox_id) = &step.append_to_mailbox_id {
        let folder = context.remote.action_mailbox_folder(mailbox_id)?;
        for message_id in item.message_ids() {
            let raw = fs::read(
                context
                    .root
                    .join(format!(".afmail/messages/{message_id}.eml")),
            )
            .map_err(|e| AppError::io("read message eml", &e))?;
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
        let reply_to = item.reply_to_message_id().ok_or_else(|| {
            AppError::new(
                "push_item_invalid",
                "reply_to_message action has no reply_to_message_id",
            )
        })?;
        let workspace = crate::store::Workspace::at(root);
        return workspace.message_remote_locations_any(&[reply_to.to_string()]);
    }
    Ok(item.locations().to_vec())
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
    for location in locations {
        remote.add_flags(&location.mailbox_name, location.uid, flags)?;
    }
    let workspace = crate::store::Workspace::at(root);
    workspace.add_remote_flags(locations, flags)
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
    for location in locations {
        if rfc822_ids.contains_key(&location.message_id) {
            continue;
        }
        let message = workspace.read_message_by_id(&location.message_id)?;
        let Some(rfc822_message_id) = message.rfc822_message_id else {
            return Err(AppError::new(
                "rfc822_message_id_missing",
                format!(
                    "message cannot be relocated after remote move without Message-ID: {}",
                    location.message_id
                ),
            ));
        };
        rfc822_ids.insert(location.message_id.clone(), rfc822_message_id);
    }
    let target = remote.action_mailbox_folder(mailbox_id)?;
    let mut target_locations: BTreeMap<String, Vec<crate::types::RemoteLocation>> = BTreeMap::new();
    for message_id in item.message_ids() {
        let message = workspace.read_message_by_id(message_id)?;
        if let Some(remote) = message.remote {
            for location in remote.locations {
                if location.missing_rfc3339.is_none()
                    && location.mailbox_name == target
                    && location.uid_validity.is_some()
                    && location.uid.is_some()
                {
                    target_locations
                        .entry(message_id.clone())
                        .or_default()
                        .push(location);
                }
            }
        }
    }
    for location in locations {
        if target_locations.contains_key(&location.message_id) {
            continue;
        }
        let rfc822_message_id = rfc822_ids
            .get(&location.message_id)
            .ok_or_else(|| AppError::new("rfc822_message_id_missing", "Message-ID missing"))?;
        let outcome = remote.move_message(
            &location.mailbox_name,
            location.uid,
            &target,
            Some(rfc822_message_id.as_str()),
        )?;
        if let Some(target_location) = outcome.target_location {
            target_locations
                .entry(location.message_id.clone())
                .or_default()
                .push(target_location);
        }
    }
    for message_id in item.message_ids() {
        if let Some(target_locations) = target_locations.get(message_id) {
            workspace.relocate_message(message_id, target_locations)?;
        }
    }
    Ok(())
}

pub(super) fn mailbox_is_kind(config: &MailConfig, mailbox_id: &str, kind: SpecialUseKind) -> bool {
    config
        .mailbox(mailbox_id)
        .ok()
        .and_then(|mailbox| {
            mailbox
                .special_use
                .as_deref()
                .and_then(SpecialUseKind::from_attribute)
        })
        .is_some_and(|candidate| candidate == kind)
        || mailbox_id == kind.as_str()
}
