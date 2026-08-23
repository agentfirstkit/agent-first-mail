mod execute;
mod io;
mod plan;
mod preview;

use execute::*;
use io::*;
use plan::*;
use preview::*;

use crate::config::{ActionStep, ActionStepOn, MailConfig, SpecialUseKind};
use crate::error::{AppError, Result};
use crate::frontmatter::DraftFrontmatter;
use crate::progress::ProgressCallback;
use crate::types::{
    CommandResult, DraftPush, MessagePush, MessagePushAction, OutboundAction, OutboundPush,
    PUSH_ITEM_SCHEMA_VERSION, PushAuditPending, PushItem, PushLocalStage, PushLocation,
    PushPayload, PushStepState, PushStepStatus,
};
use crate::util::write_string_atomic;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct RemovedOutbound {
    pub push_id: String,
    pub action: OutboundAction,
}

#[derive(Clone, Debug)]
pub struct RemovedMessagePush {
    pub push_id: String,
}

pub fn queue_outbound(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
    action: OutboundAction,
    previous_action: Option<OutboundAction>,
) -> Result<Value> {
    let existing = find_draft_item(root, case_uid, draft_name)?;
    let reset_progress = validate_outbound_requeue(
        existing.as_ref(),
        case_uid,
        draft_name,
        previous_action,
        action,
    )?;
    let preserved = existing.as_ref().filter(|_| !reset_progress);
    let push_id = match &existing {
        Some(item) => item.push_id.clone(),
        None => new_draft_push_id(root)?,
    };
    let push_dir = root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
    create_dir_all(&push_dir)?;
    let now = crate::store::now_rfc3339();
    let existing_draft = existing.as_ref().and_then(PushItem::draft);
    let enqueue_seq = match &existing {
        Some(item) => item.enqueue_seq,
        // Enqueueing requires the exclusive workspace lock (see workspace_lock.rs);
        // max+1 here is race-free only because the caller holds that lock.
        None => next_enqueue_seq(root)?,
    };
    let item = PushItem {
        schema_name: "push_item".to_string(),
        schema_version: PUSH_ITEM_SCHEMA_VERSION,
        push_id: push_id.clone(),
        enqueue_seq,
        payload: PushPayload::Draft(Box::new(DraftPush {
            case_uid: case_uid.to_string(),
            draft_name: draft_name.to_string(),
            draft_uid_validity: existing_draft.and_then(|draft| draft.draft_uid_validity),
            draft_uid: existing_draft.and_then(|draft| draft.draft_uid),
        })),
        planned_outbound: None,
        planned_outbound_message: None,
        planned_message_action: None,
        planned_message_rfc822: BTreeMap::new(),
        created_rfc3339: preserved
            .map(|item| item.created_rfc3339.clone())
            .unwrap_or_else(|| now.clone()),
        updated_rfc3339: now,
        attempt_count: preserved.map_or(0, |item| item.attempt_count),
        step_states: preserved
            .map(|item| item.step_states.clone())
            .unwrap_or_default(),
        local_stage: preserved.and_then(|item| item.local_stage),
        audit_pending: preserved.and_then(|item| item.audit_pending.clone()),
        last_error: None,
    };
    write_item(root, &item)?;
    Ok(json!({
        "push_id": push_id,
        "kind": "draft",
        "action": action.as_str(),
        "case_uid": case_uid,
        "draft_name": draft_name
    }))
}

pub(crate) fn ensure_outbound_requeue_allowed(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
    previous_action: Option<OutboundAction>,
    action: OutboundAction,
) -> Result<()> {
    let existing = find_draft_item(root, case_uid, draft_name)?;
    validate_outbound_requeue(
        existing.as_ref(),
        case_uid,
        draft_name,
        previous_action,
        action,
    )?;
    Ok(())
}

fn validate_outbound_requeue(
    existing: Option<&PushItem>,
    case_uid: &str,
    draft_name: &str,
    previous_action: Option<OutboundAction>,
    action: OutboundAction,
) -> Result<bool> {
    let Some(existing) = existing else {
        return Ok(false);
    };
    if existing.has_started_effects() {
        return Err(AppError::new(
            "push_already_started",
            format!(
                "outbound push item already started and cannot be re-queued: {}",
                existing.push_id
            ),
        )
        .with_hint("Run `afmail push`, review its current plan_id and recovery state, then confirm that exact plan before changing the queued draft action.")
        .with_details(json!({
            "push_id": existing.push_id,
            "case_uid": case_uid,
            "draft_name": draft_name,
            "previous_action": previous_action.map(OutboundAction::as_str),
            "requested_action": action.as_str(),
            "step_states": existing.step_states,
            "local_stage": existing.local_stage.map(local_stage_name),
            "audit_pending": existing.audit_pending.is_some(),
        })));
    }
    Ok(previous_action != Some(action))
}

pub(crate) fn find_outbound_for_draft(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Option<PushItem>> {
    find_draft_item(root, case_uid, draft_name)
}

pub fn queue_action_steps(
    root: &Path,
    kind: &str,
    message_ids: &[String],
    locations: &[PushLocation],
    steps: &[ActionStep],
    _reply_to_message_id: Option<String>,
) -> Result<Option<PushItem>> {
    if message_ids.is_empty() || steps.is_empty() {
        return Ok(None);
    }
    let action = MessagePushAction::from_kind(kind).ok_or_else(|| {
        AppError::new(
            "push_item_invalid",
            format!("unsupported message push action kind: {kind}"),
        )
    })?;
    let location_message_ids = locations
        .iter()
        .map(|location| location.message_id.as_str())
        .collect::<BTreeSet<_>>();
    if location_message_ids.is_empty() {
        return Ok(None);
    }
    let push_dir = root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
    create_dir_all(&push_dir)?;
    let now = crate::store::now_rfc3339();
    let mut first = None;
    for message_id in message_ids {
        if !location_message_ids.contains(message_id.as_str()) {
            continue;
        }
        let existing = find_message_item(root, message_id)?;
        let action_changed = existing
            .as_ref()
            .and_then(PushItem::message)
            .is_some_and(|message| message.action != action);
        if action_changed
            && let Some(existing) = existing.as_ref()
            && existing.has_started_effects()
        {
            return Err(AppError::new(
                "push_already_started",
                format!(
                    "message push item already started and cannot change action from {} to {}: {}",
                    existing
                        .message()
                        .map(|message| message.action.kind())
                        .unwrap_or("unknown"),
                    action.kind(),
                    existing.push_id
                ),
            )
            .with_details(json!({
                "push_id": existing.push_id,
                "message_id": message_id,
                "previous_action": existing.message().map(|message| message.action.kind()),
                "requested_action": action.kind(),
                "step_states": existing.step_states,
                "local_stage": existing.local_stage.map(local_stage_name),
                "audit_pending": existing.audit_pending.is_some(),
            })));
        }
        let preserved = existing.as_ref().filter(|_| !action_changed);
        let enqueue_seq = match &existing {
            Some(item) => item.enqueue_seq,
            // Enqueueing requires the exclusive workspace lock (see workspace_lock.rs);
            // max+1 here is race-free only because the caller holds that lock. Each
            // loop iteration writes its item before the next iteration re-scans the
            // queue, so each new item still gets its own increasing sequence.
            None => next_enqueue_seq(root)?,
        };
        let item = PushItem {
            schema_name: "push_item".to_string(),
            schema_version: PUSH_ITEM_SCHEMA_VERSION,
            push_id: message_push_id(message_id),
            enqueue_seq,
            payload: PushPayload::Message(MessagePush {
                message_id: message_id.clone(),
                action,
                message_action_snapshot: preserved.and_then(|item| {
                    item.message()
                        .and_then(|message| message.message_action_snapshot.clone())
                }),
            }),
            planned_outbound: None,
            planned_outbound_message: None,
            planned_message_action: Some(crate::types::MessageActionPush {
                action,
                message_ids: vec![message_id.clone()],
                locations: locations
                    .iter()
                    .filter(|location| location.message_id == *message_id)
                    .cloned()
                    .collect(),
                steps: steps.to_vec(),
                reply_to_message_id: _reply_to_message_id.clone(),
            }),
            planned_message_rfc822: BTreeMap::new(),
            created_rfc3339: preserved
                .map(|item| item.created_rfc3339.clone())
                .unwrap_or_else(|| now.clone()),
            updated_rfc3339: now.clone(),
            attempt_count: preserved.map_or(0, |item| item.attempt_count),
            step_states: preserved
                .map(|item| item.step_states.clone())
                .unwrap_or_default(),
            local_stage: preserved.and_then(|item| item.local_stage),
            audit_pending: preserved.and_then(|item| item.audit_pending.clone()),
            last_error: preserved.and_then(|item| item.last_error.clone()),
        };
        write_item(root, &item)?;
        first.get_or_insert(item);
    }
    Ok(first)
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PushStatus {
    /// Outbound drafts queued to save or send.
    pub drafts: usize,
    /// Case-membership flag operations queued.
    pub case: usize,
    /// Notification filing moves queued, not yet applied on the server.
    pub notification: usize,
    /// Spam (junk) moves queued, not yet applied on the server.
    pub spam: usize,
    /// Trash moves queued, not yet applied on the server.
    pub trash: usize,
}

pub fn push_status(root: &Path) -> Result<PushStatus> {
    let mut status = PushStatus::default();
    let config = MailConfig::load(root)?;
    for item in build_push_plan(root, &config)?.items {
        match item_summary_label(&item) {
            "drafts" => status.drafts += 1,
            "case" => status.case += 1,
            "notification" => status.notification += 1,
            "spam" => status.spam += 1,
            "trash" => status.trash += 1,
            _ => {}
        }
    }
    Ok(status)
}

pub fn list(root: &Path) -> Result<Value> {
    let config = MailConfig::load(root)?;
    let plan = build_push_plan(root, &config)?;
    push_plan_output(&plan, "push_list")
}

pub(crate) fn pending_items(root: &Path) -> Result<Vec<PushItem>> {
    sorted_items(root)
}

pub fn push_with_progress(
    root: &Path,
    confirm_plan_id: Option<&str>,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<CommandResult> {
    let mut progress = progress;
    let config = MailConfig::load(root)?;
    let plan = build_push_plan(root, &config)?;
    if confirm_plan_id.is_none() {
        crate::progress::emit(
            &mut progress,
            "push_preview",
            json!({
                "item_count": plan.items.len(),
                "plan_id": plan.plan_id.as_str(),
                "workspace_uid": plan.workspace_uid.as_str(),
            }),
        );
        return Ok(CommandResult::succeeded(push_plan_output(
            &plan,
            "push_plan",
        )?));
    }
    let confirmed_plan_id = confirm_plan_id.unwrap_or_default();
    require_matching_push_plan(&plan, confirmed_plan_id)?;
    let reconciled_removed_push_ids = reconcile_push_plan(root, &plan)?;
    let items = plan.items.clone();

    let remote = crate::remote::ImapSmtpRemote::new(&config);
    let mut pushed = 0usize;
    let mut failed = 0usize;
    let mut partially_applied = 0usize;
    let mut audit_pending_count = 0usize;
    let mut stopped_after_failure = false;
    let mut remaining_count = 0usize;
    let mut succeeded_items = Vec::new();
    let mut failures = Vec::new();
    let mut audit_warnings: Vec<Value> = Vec::new();
    let item_count = items.len();
    crate::progress::emit(
        &mut progress,
        "push_start",
        json!({
            "item_count": item_count,
            "plan_id": plan.plan_id.as_str(),
            "workspace_uid": plan.workspace_uid.as_str(),
        }),
    );
    for (index, mut item) in items.into_iter().enumerate() {
        let progress_context = PushProgressContext {
            item_index: index,
            item_count,
        };
        crate::progress::emit(
            &mut progress,
            "push_item_start",
            push_item_progress_fields(&item, index, item_count, None),
        );
        let result = if item.audit_pending.is_some() {
            // The remote effect and local finalization already succeeded. This durable
            // state may only finish the idempotent audit append and queue cleanup;
            // never resolve current action config or replay remote steps.
            Ok(())
        } else {
            let remote_result = match item.push_kind() {
                crate::types::PushKind::Draft => push_outbound(
                    root,
                    &config,
                    &remote,
                    &mut item,
                    progress_context,
                    progress.as_deref_mut(),
                ),
                crate::types::PushKind::Message => push_action_steps(
                    root,
                    &config,
                    &remote,
                    &mut item,
                    progress_context,
                    progress.as_deref_mut(),
                ),
            };
            remote_result.and_then(|()| {
                finalize_pushed_item(
                    root,
                    &config,
                    &mut item,
                    progress_context,
                    progress.as_deref_mut(),
                )
            })
        };
        match result {
            Ok(()) => {
                if let Err(audit_err) = complete_successful_push_audit(root, &mut item) {
                    let fields = audit_warning_fields(&item, "push_succeeded", &audit_err);
                    crate::progress::emit(&mut progress, "push_audit_failed", fields.clone());
                    audit_warnings.push(fields);
                    audit_pending_count += 1;
                    if let Some(pending) = &mut item.audit_pending {
                        pending.last_error = Some(audit_err.to_string());
                    }
                    item.last_error = Some(audit_err.to_string());
                    let _ = write_item(root, &item);
                }
                pushed += 1;
                succeeded_items.push(push_success_value(&item));
                crate::progress::emit(
                    &mut progress,
                    "push_item_done",
                    push_item_progress_fields(&item, index, item_count, None),
                );
                if item.audit_pending.is_some() {
                    stopped_after_failure = true;
                    remaining_count = item_count.saturating_sub(index + 1);
                    break;
                }
            }
            Err(err) => {
                let item_partially_applied = item.succeeded_step_count() > 0
                    || item.local_stage.is_some()
                    || item
                        .step_states
                        .iter()
                        .any(|state| state.status == PushStepStatus::RemoteOutcomeUnknown);
                if item_partially_applied {
                    partially_applied += 1;
                }
                if let Err(audit_err) = audit_push(root, "push_failed", &item, Some(&err)) {
                    let fields = audit_warning_fields(&item, "push_failed", &audit_err);
                    crate::progress::emit(&mut progress, "push_audit_failed", fields.clone());
                    audit_warnings.push(fields);
                }
                failed += 1;
                item.attempt_count += 1;
                item.updated_rfc3339 = crate::store::now_rfc3339();
                item.last_error = Some(err.to_string());
                let mut state_persistence_failures = Vec::new();
                if let Err(persist_err) = persist_failed_push_item(root, &item) {
                    state_persistence_failures.push(push_state_persistence_failure_value(
                        "push_item",
                        &persist_err,
                    ));
                }
                if let Err(persist_err) = persist_pending_push_error(root, &item, &err) {
                    state_persistence_failures.push(push_state_persistence_failure_value(
                        "pending_state",
                        &persist_err,
                    ));
                }
                failures.push(push_failure_value(
                    &item,
                    &err,
                    item_partially_applied,
                    state_persistence_failures,
                ));
                crate::progress::emit(
                    &mut progress,
                    "push_item_failed",
                    push_item_progress_fields(&item, index, item_count, Some(&err)),
                );
                stopped_after_failure = true;
                remaining_count = item_count.saturating_sub(index + 1);
                break;
            }
        }
    }
    crate::progress::emit(
        &mut progress,
        "push_done",
        json!({
            "item_count": item_count,
            "pushed_count": pushed,
            "failed_count": failed,
            "partially_applied_count": partially_applied,
            "audit_pending_count": audit_pending_count,
            "stopped_after_failure": stopped_after_failure,
            "remaining_count": remaining_count,
            "audit_warning_count": audit_warnings.len(),
        }),
    );
    Ok(CommandResult::aggregate(
        json!({
            "confirmed": true,
            "plan_id": plan.plan_id,
            "workspace_uid": plan.workspace_uid,
            "workspace_path": plan.workspace_path,
            "reconciled_removed_push_ids": reconciled_removed_push_ids,
            "pushed_count": pushed,
            "failed_count": failed,
            "partially_applied_count": partially_applied,
            "audit_pending_count": audit_pending_count,
            "stopped_after_failure": stopped_after_failure,
            "remaining_count": remaining_count,
            "succeeded_items": succeeded_items,
            "failures": failures,
            "audit_warnings": audit_warnings
        }),
        pushed + partially_applied,
        failed + audit_warnings.len(),
    ))
}

fn push_success_value(item: &PushItem) -> Value {
    let outbound = item.outbound();
    json!({
        "push_id": item.push_id.as_str(),
        "kind": item.kind(),
        "display_kind": item.display_kind(),
        "case_uid": outbound.map(|outbound| outbound.case_uid.as_str()),
        "draft_name": outbound.map(|outbound| outbound.draft_name.as_str()),
        "action": outbound.map(|outbound| outbound.action.as_str()),
        "message_ids": item.message_ids(),
        "succeeded_step_count": item.succeeded_step_count(),
        "audit_pending": item.audit_pending.is_some(),
    })
}

fn push_failure_value(
    item: &PushItem,
    err: &AppError,
    partially_applied: bool,
    state_persistence_failures: Vec<Value>,
) -> Value {
    let mut value = err.to_error_value();
    if let Value::Object(map) = &mut value {
        map.insert("push_id".to_string(), json!(item.push_id.as_str()));
        map.insert(
            "succeeded_step_count".to_string(),
            json!(item.succeeded_step_count()),
        );
        map.insert("partially_applied".to_string(), json!(partially_applied));
        map.insert(
            "state_persistence_failures".to_string(),
            json!(state_persistence_failures),
        );
    }
    value
}

fn persist_failed_push_item(root: &Path, item: &PushItem) -> Result<()> {
    fail_push_failure_persistence("push_item")?;
    write_item(root, item)
}

fn persist_pending_push_error(root: &Path, item: &PushItem, err: &AppError) -> Result<()> {
    fail_push_failure_persistence("pending_state")?;
    crate::store::Workspace::at(root).mark_pending_push_error(item, &err.to_string())
}

fn fail_push_failure_persistence(stage: &str) -> Result<()> {
    let Ok(value) = std::env::var("AFMAIL_TEST_FAIL_PUSH_FAILURE_PERSIST") else {
        return Ok(());
    };
    if value != stage {
        return Ok(());
    }
    Err(AppError::new(
        "push_failure_persistence_test_failure",
        format!("test failure while persisting push failure stage {stage}"),
    ))
}

fn push_state_persistence_failure_value(stage: &str, err: &AppError) -> Value {
    let mut value = err.to_error_value();
    if let Value::Object(map) = &mut value {
        map.insert("stage".to_string(), json!(stage));
    }
    value
}

fn finalize_pushed_item(
    root: &Path,
    config: &MailConfig,
    item: &mut PushItem,
    context: PushProgressContext,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<()> {
    let mut progress = progress;
    advance_local_stage(root, item, PushLocalStage::RemoteSucceeded)?;
    let workspace = crate::store::Workspace::at(root);
    if item.outbound().is_some() {
        if !local_stage_at_least(item, PushLocalStage::OutboundRecorded) {
            workspace.record_outbound_after_push(item, config)?;
            advance_local_stage(root, item, PushLocalStage::OutboundRecorded)?;
            emit_local_refresh_start(&mut progress, item, context, "outbound_recorded");
            workspace.refresh_outbound_push_views(item)?;
            emit_local_refresh_done(&mut progress, item, context, "outbound_recorded");
        }
        if !local_stage_at_least(item, PushLocalStage::DraftRemoved) {
            workspace.remove_outbound_draft_after_push(item)?;
            advance_local_stage(root, item, PushLocalStage::DraftRemoved)?;
            emit_local_refresh_start(&mut progress, item, context, "draft_removed");
            workspace.refresh_outbound_push_views(item)?;
            emit_local_refresh_done(&mut progress, item, context, "draft_removed");
        }
    }
    if !local_stage_at_least(item, PushLocalStage::PendingCleared) {
        workspace.clear_pending_push_item(item)?;
        advance_local_stage(root, item, PushLocalStage::PendingCleared)?;
        emit_local_refresh_start(&mut progress, item, context, "pending_cleared");
        workspace.refresh_message_push_views(item.message_ids())?;
        emit_local_refresh_done(&mut progress, item, context, "pending_cleared");
    }
    prepare_push_audit_pending(root, item)
}

fn prepare_push_audit_pending(root: &Path, item: &mut PushItem) -> Result<()> {
    if item.audit_pending.is_none() {
        let outbound = item.outbound().cloned();
        item.audit_pending = Some(PushAuditPending {
            event_id: crate::store::new_audit_event_id(),
            created_rfc3339: crate::store::now_rfc3339(),
            push_kind: item.display_kind(),
            case_uid: outbound.as_ref().map(|outbound| outbound.case_uid.clone()),
            draft_name: outbound
                .as_ref()
                .map(|outbound| outbound.draft_name.clone()),
            action: outbound
                .as_ref()
                .map(|outbound| outbound.action.as_str().to_string()),
            message_ids: item.message_ids().to_vec(),
            succeeded_step_count: item.succeeded_step_count(),
            attempt_count: item.attempt_count,
            last_error: None,
        });
    }
    if !local_stage_at_least(item, PushLocalStage::AuditPending) {
        item.local_stage = Some(PushLocalStage::AuditPending);
        item.updated_rfc3339 = crate::store::now_rfc3339();
        write_item(root, item)?;
        fail_after_local_stage(PushLocalStage::AuditPending)?;
    }
    Ok(())
}

fn complete_successful_push_audit(root: &Path, item: &mut PushItem) -> Result<()> {
    audit_push(root, "push_succeeded", item, None)?;
    delete_item(root, item)?;
    crate::store::Workspace::at(root).finalize_archiving_cases()?;
    item.audit_pending = None;
    item.last_error = None;
    Ok(())
}

fn advance_local_stage(root: &Path, item: &mut PushItem, stage: PushLocalStage) -> Result<()> {
    if local_stage_at_least(item, stage) {
        return Ok(());
    }
    item.local_stage = Some(stage);
    item.updated_rfc3339 = crate::store::now_rfc3339();
    write_item(root, item)?;
    fail_after_local_stage(stage)
}

fn local_stage_at_least(item: &PushItem, stage: PushLocalStage) -> bool {
    item.local_stage.is_some_and(|current| current >= stage)
}

fn fail_after_local_stage(stage: PushLocalStage) -> Result<()> {
    let Ok(value) = std::env::var("AFMAIL_TEST_FAIL_PUSH_LOCAL_STAGE_AFTER") else {
        return Ok(());
    };
    if value != local_stage_name(stage) {
        return Ok(());
    }
    Err(AppError::new(
        "push_local_stage_test_failure",
        format!(
            "test failure after push local stage {}",
            local_stage_name(stage)
        ),
    ))
}

fn local_stage_name(stage: PushLocalStage) -> &'static str {
    match stage {
        PushLocalStage::RemoteSucceeded => "remote_succeeded",
        PushLocalStage::OutboundRecorded => "outbound_recorded",
        PushLocalStage::DraftRemoved => "draft_removed",
        PushLocalStage::PendingCleared => "pending_cleared",
        PushLocalStage::AuditPending => "audit_pending",
    }
}

fn emit_local_refresh_start(
    progress: &mut Option<&mut ProgressCallback<'_>>,
    item: &PushItem,
    context: PushProgressContext,
    local_stage: &str,
) {
    emit_local_refresh(
        progress,
        "push_local_refresh_start",
        item,
        context,
        local_stage,
    );
}

fn emit_local_refresh_done(
    progress: &mut Option<&mut ProgressCallback<'_>>,
    item: &PushItem,
    context: PushProgressContext,
    local_stage: &str,
) {
    emit_local_refresh(
        progress,
        "push_local_refresh_done",
        item,
        context,
        local_stage,
    );
}

fn emit_local_refresh(
    progress: &mut Option<&mut ProgressCallback<'_>>,
    phase: &str,
    item: &PushItem,
    context: PushProgressContext,
    local_stage: &str,
) {
    crate::progress::emit(
        progress,
        phase,
        json!({
            "push_id": item.push_id.as_str(),
            "kind": item.kind(),
            "display_kind": item.display_kind(),
            "item_index": context.item_index + 1,
            "item_count": context.item_count,
            "local_stage": local_stage,
        }),
    );
}

fn push_item_progress_fields(
    item: &PushItem,
    index: usize,
    item_count: usize,
    err: Option<&AppError>,
) -> Value {
    let mut value = json!({
        "push_id": item.push_id.as_str(),
        "kind": item.kind(),
        "display_kind": item.display_kind(),
        "index": index + 1,
        "item_count": item_count,
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

/// Structured warning emitted when a confirmed effect's audit append could not be
/// completed. The durable audit_pending item remains recovery-only, so callers can
/// report the remote success without losing the audit identity or replaying it.
fn audit_warning_fields(item: &PushItem, audit_kind: &str, err: &AppError) -> Value {
    json!({
        "push_id": item.push_id.as_str(),
        "audit_kind": audit_kind,
        "error_code": err.error_code,
        "error": err.message.as_str(),
    })
}

fn audit_push(root: &Path, kind: &str, item: &PushItem, err: Option<&AppError>) -> Result<()> {
    if std::env::var("AFMAIL_TEST_FAIL_PUSH_AUDIT").is_ok_and(|value| value == kind) {
        return Err(AppError::new(
            "push_audit_test_failure",
            format!("test failure while writing {kind} audit event"),
        ));
    }
    let mut targets = vec![json!({"kind": "push", "id": item.push_id.as_str()})];
    if let Some(outbound) = item.outbound() {
        targets.push(json!({"kind": "case", "id": outbound.case_uid.as_str()}));
    } else {
        targets.extend(
            item.message_ids()
                .iter()
                .map(|message_id| json!({"kind": "message", "id": message_id})),
        );
    }
    let mut fields = json!({
        "push_id": item.push_id.as_str(),
        "push_kind": item.display_kind(),
        "succeeded_step_count": item.succeeded_step_count(),
        "attempt_count": item.attempt_count,
    });
    if let Some(outbound) = item.outbound()
        && let Value::Object(map) = &mut fields
    {
        map.insert("case_uid".to_string(), json!(outbound.case_uid.as_str()));
        map.insert(
            "draft_name".to_string(),
            json!(outbound.draft_name.as_str()),
        );
        map.insert("action".to_string(), json!(outbound.action.as_str()));
    }
    if let Some(err) = err
        && let Value::Object(map) = &mut fields
    {
        map.insert("error_code".to_string(), json!(err.error_code));
        map.insert("error".to_string(), json!(err.message.as_str()));
        map.insert("retryable".to_string(), json!(err.retryable));
    }
    let workspace = crate::store::Workspace::at(root);
    if kind == "push_succeeded" {
        let pending = item.audit_pending.as_ref().ok_or_else(|| {
            AppError::new(
                "push_audit_state_invalid",
                format!(
                    "push {} has no durable audit_pending identity",
                    item.push_id
                ),
            )
        })?;
        let mut durable_targets = vec![json!({"kind": "push", "id": item.push_id.as_str()})];
        if let Some(case_uid) = pending.case_uid.as_deref() {
            durable_targets.push(json!({"kind": "case", "id": case_uid}));
        } else {
            durable_targets.extend(
                pending
                    .message_ids
                    .iter()
                    .map(|message_id| json!({"kind": "message", "id": message_id})),
            );
        }
        let durable_fields = json!({
            "push_id": item.push_id.as_str(),
            "push_kind": pending.push_kind.as_str(),
            "succeeded_step_count": pending.succeeded_step_count,
            "attempt_count": pending.attempt_count,
            "case_uid": pending.case_uid.as_deref(),
            "draft_name": pending.draft_name.as_deref(),
            "action": pending.action.as_deref(),
        });
        return workspace
            .append_audit_event_with_identity(
                &pending.event_id,
                &pending.created_rfc3339,
                kind,
                durable_targets,
                None,
                durable_fields,
            )
            .map(|_| ());
    }
    workspace.append_audit_event(kind, targets, None, fields)
}

pub fn remove_outbound_for_draft(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Vec<RemovedOutbound>> {
    let items = read_items(root)?
        .into_iter()
        .filter(|item| {
            item.draft()
                .is_some_and(|draft| draft.case_uid == case_uid && draft.draft_name == draft_name)
        })
        .collect::<Vec<_>>();
    if let Some(item) = items.iter().find(|item| item.has_started_effects()) {
        let draft = item.draft();
        return Err(AppError::new(
            "push_already_started",
            format!(
                "outbound push item already started and cannot be removed locally: {}",
                item.push_id
            ),
        )
        .with_hint("Run `afmail push`, review its current plan_id and incomplete step state, then confirm that exact plan; do not remove a draft after remote processing has started.")
        .with_details(json!({
            "push_id": item.push_id.as_str(),
            "case_uid": draft.map(|draft| draft.case_uid.as_str()),
            "draft_name": draft.map(|draft| draft.draft_name.as_str()),
            "step_states": &item.step_states,
            "suggested_commands": [
                "afmail push",
                "afmail push list"
            ]
        })));
    }
    let mut removed = Vec::new();
    for item in items {
        let action = draft_frontmatter(root, case_uid, draft_name)?
            .and_then(|frontmatter| frontmatter.sync_intent)
            .map(OutboundAction::from)
            .unwrap_or(OutboundAction::Send);
        removed.push(RemovedOutbound {
            push_id: item.push_id.clone(),
            action,
        });
        delete_item(root, &item)?;
    }
    Ok(removed)
}

pub fn remove_pending_message_pushes(
    root: &Path,
    message_id: &str,
    kind: &str,
) -> Result<Vec<RemovedMessagePush>> {
    let action = MessagePushAction::from_kind(kind).ok_or_else(|| {
        AppError::new(
            "push_item_invalid",
            format!("unsupported message push action kind: {kind}"),
        )
    })?;
    let items = read_items(root)?
        .into_iter()
        .filter(|item| {
            item.message()
                .is_some_and(|payload| payload.message_id == message_id && payload.action == action)
        })
        .collect::<Vec<_>>();
    if let Some(item) = items.iter().find(|item| item.has_started_effects()) {
        return Err(AppError::new(
            "push_already_started",
            format!(
                "push item already started and cannot be undone locally: {}",
                item.push_id
            ),
        ));
    }

    let mut removed = Vec::new();
    for item in items {
        let push_id = item.push_id.clone();
        delete_item(root, &item)?;
        removed.push(RemovedMessagePush { push_id });
    }
    Ok(removed)
}

fn plan_item(root: &Path, config: &MailConfig, item: &mut PushItem) -> Result<bool> {
    if item.audit_pending.is_some() {
        hydrate_audit_pending_plan(item)?;
        return Ok(true);
    }
    match &item.payload {
        PushPayload::Message(message) => {
            let message = message.clone();
            let workspace = crate::store::Workspace::at(root);
            let current_push_path = push_path(root, &item.push_id);
            item.planned_message_action = if item.has_started_effects() {
                match message.message_action_snapshot.clone() {
                    Some(snapshot) => {
                        validate_message_action_snapshot(item, &message, &snapshot)?;
                        Some(snapshot)
                    }
                    None => workspace.plan_message_push(
                        &message.message_id,
                        message.action,
                        Some(&current_push_path),
                        config,
                    )?,
                }
            } else {
                workspace.plan_message_push(
                    &message.message_id,
                    message.action,
                    Some(&current_push_path),
                    config,
                )?
            };
            if item.planned_message_action.is_some() {
                reset_progress_if_plan_changed(root, config, item)?;
                if item.message_action().is_some_and(|action| {
                    action
                        .steps
                        .iter()
                        .any(|step| step.append_to_mailbox_id.is_some())
                }) {
                    for message_id in item.message_ids().to_vec() {
                        let raw_path = root
                            .join(".afmail/messages")
                            .join(format!("{message_id}.eml"));
                        let raw = fs::read(&raw_path)
                            .map_err(|err| AppError::io("read planned message eml", &err))?;
                        item.planned_message_rfc822.insert(message_id, raw);
                    }
                }
                Ok(true)
            } else if item.has_started_effects() {
                Err(AppError::new(
                    "push_remote_outcome_unknown",
                    format!(
                        "message push {} has started but its original action plan is unavailable",
                        item.push_id
                    ),
                )
                .with_hint(
                    "Inspect the push item and remote mailbox; this item cannot be reconciled from current mailbox state alone.",
                )
                .with_details(json!({
                    "push_id": item.push_id.as_str(),
                    "message_id": message.message_id.as_str(),
                    "action": message.action.kind(),
                    "remote_calls_started": true,
                })))
            } else {
                Ok(false)
            }
        }
        PushPayload::Draft(draft) => {
            let draft = draft.as_ref().clone();
            let Some(frontmatter) = draft_frontmatter(root, &draft.case_uid, &draft.draft_name)?
            else {
                if local_stage_at_least(item, PushLocalStage::DraftRemoved) {
                    item.planned_outbound = Some(OutboundPush {
                        action: OutboundAction::Send,
                        case_uid: draft.case_uid.clone(),
                        draft_name: draft.draft_name.clone(),
                        draft_uid_validity: draft.draft_uid_validity,
                        draft_uid: draft.draft_uid,
                        steps: Vec::new(),
                        reply_to_message_id: None,
                    });
                    return Ok(true);
                }
                if !item.has_started_effects() {
                    return Ok(false);
                }
                return Err(AppError::new(
                    "push_remote_outcome_unknown",
                    format!(
                        "draft push {} has started but the draft has no sync_intent",
                        item.push_id
                    ),
                )
                .with_hint("Inspect the draft and push item before retrying."));
            };
            let Some(action) = frontmatter.sync_intent.map(OutboundAction::from) else {
                if !item.has_started_effects() {
                    return Ok(false);
                }
                return Err(AppError::new(
                    "push_remote_outcome_unknown",
                    format!(
                        "draft push {} has started but the draft has no sync_intent",
                        item.push_id
                    ),
                )
                .with_hint("Inspect the draft and push item before retrying."));
            };
            let configured_steps = configured_outbound_steps(config, action);
            item.planned_outbound = Some(OutboundPush {
                action,
                case_uid: draft.case_uid.clone(),
                draft_name: draft.draft_name.clone(),
                draft_uid_validity: draft.draft_uid_validity,
                draft_uid: draft.draft_uid,
                steps: Vec::new(),
                reply_to_message_id: frontmatter.reply_to_message_id,
            });
            let resolved_steps = filter_reply_to_steps(root, item, configured_steps)?;
            if let Some(outbound) = &mut item.planned_outbound {
                outbound.steps = resolved_steps;
            }
            reset_progress_if_plan_changed(root, config, item)?;
            item.planned_outbound_message = Some(prepare_current_outbound(root, config, item)?);
            Ok(true)
        }
    }
}

fn validate_message_action_snapshot(
    item: &PushItem,
    message: &MessagePush,
    snapshot: &crate::types::MessageActionPush,
) -> Result<()> {
    let message_ids_match = snapshot.message_ids.as_slice() == [message.message_id.as_str()];
    let locations_match = snapshot
        .locations
        .iter()
        .all(|location| location.message_id == message.message_id);
    if snapshot.action == message.action
        && message_ids_match
        && locations_match
        && !snapshot.locations.is_empty()
        && !snapshot.steps.is_empty()
    {
        return Ok(());
    }
    Err(AppError::new(
        "push_item_invalid",
        format!(
            "message action snapshot does not match queued intent for {}",
            item.push_id
        ),
    )
    .with_details(json!({
        "push_id": item.push_id.as_str(),
        "message_id": message.message_id.as_str(),
        "action": message.action.kind(),
        "snapshot_action": snapshot.action.kind(),
        "snapshot_message_ids": snapshot.message_ids,
        "snapshot_location_count": snapshot.locations.len(),
        "snapshot_step_count": snapshot.steps.len(),
        "remote_calls_started": item.has_started_effects(),
    })))
}

fn hydrate_audit_pending_plan(item: &mut PushItem) -> Result<()> {
    let pending = item.audit_pending.clone().ok_or_else(|| {
        AppError::new(
            "push_audit_state_invalid",
            format!("push {} has no audit_pending payload", item.push_id),
        )
    })?;
    if let (Some(case_uid), Some(draft_name), Some(action)) = (
        pending.case_uid,
        pending.draft_name,
        pending.action.as_deref(),
    ) {
        let action = match action {
            "save_draft" => OutboundAction::SaveDraft,
            "send" => OutboundAction::Send,
            other => {
                return Err(AppError::new(
                    "push_audit_state_invalid",
                    format!("unsupported audit_pending outbound action: {other}"),
                ));
            }
        };
        let draft = item.draft();
        item.planned_outbound = Some(OutboundPush {
            action,
            case_uid,
            draft_name,
            draft_uid_validity: draft.and_then(|draft| draft.draft_uid_validity),
            draft_uid: draft.and_then(|draft| draft.draft_uid),
            steps: Vec::new(),
            reply_to_message_id: None,
        });
        return Ok(());
    }
    let action = MessagePushAction::from_kind(&pending.push_kind).ok_or_else(|| {
        AppError::new(
            "push_audit_state_invalid",
            format!(
                "unsupported audit_pending message action: {}",
                pending.push_kind
            ),
        )
    })?;
    item.planned_message_action = Some(crate::types::MessageActionPush {
        action,
        message_ids: pending.message_ids,
        locations: Vec::new(),
        steps: Vec::new(),
        reply_to_message_id: None,
    });
    Ok(())
}

fn configured_outbound_steps(config: &MailConfig, action: OutboundAction) -> Vec<ActionStep> {
    match action {
        OutboundAction::SaveDraft if config.actions.draft.save.steps.is_empty() => {
            vec![ActionStep::append_to_mailbox_id("drafts")]
        }
        OutboundAction::SaveDraft => config.actions.draft.save.steps.clone(),
        OutboundAction::Send if config.actions.draft.send.steps.is_empty() => {
            crate::config::default_draft_send_steps()
        }
        OutboundAction::Send => config.actions.draft.send.steps.clone(),
    }
}

fn reset_progress_if_plan_changed(
    root: &Path,
    config: &MailConfig,
    item: &mut PushItem,
) -> Result<()> {
    if item.step_states.is_empty() {
        return Ok(());
    }
    let planned_labels = actions_for(root, config, item)?;
    let current_labels = item
        .step_states
        .iter()
        .map(|state| state.label.as_str())
        .collect::<Vec<_>>();
    let planned_label_refs = planned_labels
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if current_labels == planned_label_refs
        || (current_labels.len() < planned_label_refs.len()
            && planned_label_refs.starts_with(&current_labels))
    {
        return Ok(());
    }
    if item.has_started_effects() {
        return Err(AppError::new(
            "push_plan_changed_after_start",
            format!(
                "push item {} has recorded remote progress but now resolves to a different plan",
                item.push_id
            ),
        )
        .with_hint("Inspect the push item and remote mailbox before changing this dirty ref.")
        .with_details(json!({
            "push_id": item.push_id.as_str(),
            "current_step_labels": current_labels,
            "planned_step_labels": planned_labels,
            "local_stage": item.local_stage.map(local_stage_name),
            "audit_pending": item.audit_pending.is_some(),
        })));
    }
    item.step_states.clear();
    item.local_stage = None;
    item.last_error = None;
    Ok(())
}

fn draft_frontmatter(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Option<DraftFrontmatter>> {
    let case_path = find_case_path(root, case_uid)?;
    let draft_path = case_path.join("drafts").join(draft_name);
    if !draft_path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
    let (frontmatter, _) = crate::markdown::read_doc::<DraftFrontmatter>(&text)?;
    Ok(Some(frontmatter))
}
