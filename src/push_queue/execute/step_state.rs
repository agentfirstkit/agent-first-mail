//! Durable transitions for one queued action step. Remote I/O remains in the executor.
use super::step_label;
use crate::config::ActionStep;
use crate::error::AppError;
use crate::types::{PushItem, PushPayload, PushStepState, PushStepStatus};

pub(super) fn ensure_step_states(item: &mut PushItem, steps: &[ActionStep]) {
    for (index, step) in steps.iter().enumerate() {
        if !item.step_states.iter().any(|state| state.index == index) {
            item.step_states.push(PushStepState {
                index,
                label: step_label(step),
                status: PushStepStatus::Pending,
                started_rfc3339: None,
                completed_rfc3339: None,
                result_summary: None,
                send_receipt: None,
                error_code: None,
                error: None,
                retryable: None,
            });
        }
    }
    item.step_states.retain(|state| state.index < steps.len());
    item.step_states.sort_by_key(|state| state.index);
}

pub(super) fn step_succeeded(item: &PushItem, index: usize) -> bool {
    item.step_states
        .iter()
        .any(|state| state.index == index && state.status == PushStepStatus::Succeeded)
}

pub(super) fn step_remote_outcome_unknown(item: &PushItem, index: usize) -> bool {
    item.step_states
        .iter()
        .any(|state| state.index == index && state.status == PushStepStatus::RemoteOutcomeUnknown)
}

pub(super) fn legacy_move_message_id_preflight_failed(
    item: &PushItem,
    index: usize,
    step: &ActionStep,
) -> bool {
    if item.message_action().is_none()
        || step.move_to_mailbox_id.is_none()
        || !step.add_flags.is_empty()
        || step.append_to_mailbox_id.is_some()
        || step.is_send()
    {
        return false;
    }
    item.step_states.iter().any(|state| {
        state.index == index
            && state.error_code.as_deref() == Some("rfc822_message_id_missing")
            && item.message_ids().iter().any(|message_id| {
                state.error.as_deref()
                    == Some(
                        format!(
                            "message cannot be relocated after remote move without Message-ID: {message_id}"
                        )
                        .as_str(),
                    )
            })
    })
}

fn step_state_mut(item: &mut PushItem, index: usize) -> Option<&mut PushStepState> {
    item.step_states
        .iter_mut()
        .find(|state| state.index == index)
}

fn record_attempt(
    state: &mut PushStepState,
    step: &ActionStep,
    status: PushStepStatus,
    now: String,
) {
    state.label = step_label(step);
    state.status = status;
    state.started_rfc3339.get_or_insert(now);
}

fn clear_step_error(state: &mut PushStepState) {
    state.error_code = None;
    state.error = None;
    state.retryable = None;
}

pub(super) fn mark_step_pending(item: &mut PushItem, index: usize, step: &ActionStep) {
    freeze_message_action_snapshot(item);
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        state.label = step_label(step);
        state.status = PushStepStatus::Pending;
        state.started_rfc3339 = Some(now);
        state.completed_rfc3339 = None;
        state.result_summary = None;
        state.send_receipt = None;
        clear_step_error(state);
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

pub(in crate::push_queue) fn mark_step_succeeded(
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
) {
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        record_attempt(state, step, PushStepStatus::Succeeded, now.clone());
        state.completed_rfc3339 = Some(now);
        state.result_summary = Some(step_result_summary(step).to_string());
        clear_step_error(state);
    }
}

pub(super) fn mark_step_failed(
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
    err: &AppError,
) {
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        record_attempt(state, step, PushStepStatus::Failed, now.clone());
        state.completed_rfc3339 = Some(now);
        state.result_summary = None;
        state.error_code = Some(err.error_code.to_string());
        state.error = Some(err.message.clone());
        state.retryable = Some(err.retryable);
    }
}

pub(super) fn mark_step_remote_outcome_unknown(
    item: &mut PushItem,
    index: usize,
    step: &ActionStep,
    err: Option<&AppError>,
) {
    let now = crate::store::now_rfc3339();
    if let Some(state) = step_state_mut(item, index) {
        record_attempt(state, step, PushStepStatus::RemoteOutcomeUnknown, now);
        state.completed_rfc3339 = None;
        state.result_summary = Some("remote outcome unknown".to_string());
        state.error_code = err.map(|err| err.error_code.to_string());
        state.error = err.map(|err| err.message.clone());
        state.retryable = Some(false);
    }
}

fn step_result_summary(step: &ActionStep) -> &'static str {
    if step.file_send.is_some() {
        "file_send published"
    } else if step.cloudflare_send.is_some() {
        "cloudflare_send submitted; inspect send_receipt for recipient outcomes"
    } else if step.smtp_send.is_some() {
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

/// Release an uncertain attempt after explicit recovery, retaining its diagnostic
/// and receipt until the next execution starts. The audit keeps the prior state.
pub(in crate::push_queue) fn mark_step_recovered_pending(state: &mut PushStepState, summary: &str) {
    state.status = PushStepStatus::Pending;
    state.started_rfc3339 = None;
    state.completed_rfc3339 = None;
    state.result_summary = Some(summary.to_string());
    state.retryable = Some(true);
}
