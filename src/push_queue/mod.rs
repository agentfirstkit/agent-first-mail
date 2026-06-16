mod execute;
mod io;
mod preview;

use execute::*;
use io::*;
use preview::*;

use crate::config::{ActionStep, ActionStepOn, MailConfig, SpecialUseKind};
use crate::error::{AppError, Result};
use crate::frontmatter::DraftFrontmatter;
use crate::progress::ProgressCallback;
use crate::types::{
    MessageActionPush, MessagePushAction, OutboundAction, OutboundPush, PushItem, PushLocation,
    PushPayload, PushStepState, PushStepStatus,
};
use crate::util::write_string_atomic;
use serde_json::{json, Value};
use std::collections::BTreeMap;
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
) -> Result<Value> {
    let existing = find_outbound_item(root, case_uid, draft_name)?;
    let push_id = existing
        .as_ref()
        .map(|item| item.push_id.clone())
        .unwrap_or_else(|| unique_push_id(root));
    let push_dir = root.join(".afmail/push");
    create_dir_all(&push_dir)?;
    let now = crate::store::now_rfc3339();
    let existing_outbound = existing.as_ref().and_then(|item| item.outbound());
    let same_action = existing_outbound.is_some_and(|outbound| outbound.action == action);
    let step_states = if same_action {
        existing
            .as_ref()
            .map(|item| item.step_states.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let item = PushItem {
        schema_name: "push_item".to_string(),
        schema_version: 1,
        push_id: push_id.clone(),
        payload: PushPayload::Outbound(Box::new(OutboundPush {
            action,
            case_uid: case_uid.to_string(),
            draft_name: draft_name.to_string(),
            draft_uid_validity: existing_outbound.and_then(|outbound| outbound.draft_uid_validity),
            draft_uid: existing_outbound.and_then(|outbound| outbound.draft_uid),
        })),
        created_rfc3339: existing
            .as_ref()
            .map(|item| item.created_rfc3339.clone())
            .unwrap_or_else(|| now.clone()),
        updated_rfc3339: now,
        attempt_count: if same_action {
            existing.as_ref().map_or(0, |item| item.attempt_count)
        } else {
            0
        },
        step_states,
        last_error: None,
    };
    write_item(root, &item)?;
    Ok(json!({
        "code": "push_queued",
        "push_id": push_id,
        "kind": "outbound",
        "action": action.as_str(),
        "case_uid": case_uid,
        "draft_name": draft_name
    }))
}

pub(crate) fn find_outbound_for_draft(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Option<PushItem>> {
    find_outbound_item(root, case_uid, draft_name)
}

pub fn queue_action_steps(
    root: &Path,
    kind: &str,
    message_ids: &[String],
    locations: &[PushLocation],
    steps: &[ActionStep],
    reply_to_message_id: Option<String>,
) -> Result<Option<PushItem>> {
    if locations.is_empty() || steps.is_empty() {
        return Ok(None);
    }
    let action = MessagePushAction::from_kind(kind).ok_or_else(|| {
        AppError::new(
            "push_item_invalid",
            format!("unsupported message push action kind: {kind}"),
        )
    })?;
    let push_dir = root.join(".afmail/push");
    create_dir_all(&push_dir)?;
    let push_id = unique_push_id(root);
    let now = crate::store::now_rfc3339();
    let item = PushItem {
        schema_name: "push_item".to_string(),
        schema_version: 1,
        push_id,
        payload: PushPayload::MessageAction(MessageActionPush {
            action,
            message_ids: message_ids.to_vec(),
            locations: locations.to_vec(),
            steps: steps.to_vec(),
            reply_to_message_id,
        }),
        created_rfc3339: now.clone(),
        updated_rfc3339: now,
        attempt_count: 0,
        step_states: Vec::new(),
        last_error: None,
    };
    write_item(root, &item)?;
    Ok(Some(item))
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PushStatus {
    /// Outbound drafts queued to save or send.
    pub drafts: usize,
    /// Case-membership flag operations queued.
    pub case: usize,
    /// Archive moves queued, not yet applied on the server.
    pub archive: usize,
    /// Spam (junk) moves queued, not yet applied on the server.
    pub spam: usize,
    /// Trash moves queued, not yet applied on the server.
    pub trash: usize,
}

pub fn push_status(root: &Path) -> Result<PushStatus> {
    let mut status = PushStatus::default();
    for item in sorted_items(root)? {
        match item_summary_label(&item) {
            "drafts" => status.drafts += 1,
            "case" => status.case += 1,
            "archive" => status.archive += 1,
            "spam" => status.spam += 1,
            "trash" => status.trash += 1,
            _ => {}
        }
    }
    Ok(status)
}

pub fn list(root: &Path) -> Result<Value> {
    let items = sorted_items(root)?;
    Ok(json!({
        "code": "push_list",
        "count": items.len(),
        "items": items
    }))
}

pub(crate) fn pending_items(root: &Path) -> Result<Vec<PushItem>> {
    sorted_items(root)
}

pub fn push_with_progress(
    root: &Path,
    confirmed: bool,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    let mut progress = progress;
    let items = sorted_items(root)?;
    let config = MailConfig::load(root)?;
    if !confirmed {
        crate::progress::emit(
            &mut progress,
            "push_preview",
            json!({
                "item_count": items.len(),
            }),
        );
        let rendered = items
            .iter()
            .map(|item| {
                let outbound = item.outbound();
                Ok(json!({
                    "push_id": item.push_id,
                    "kind": item.kind(),
                    "display_kind": item.display_kind(),
                    "actions": actions_for(root, &config, item)?,
                    "case_uid": outbound.map(|outbound| outbound.case_uid.as_str()),
                    "draft_name": outbound.map(|outbound| outbound.draft_name.as_str()),
                    "action": outbound.map(|outbound| outbound.action.as_str())
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(json!({
            "code": "push_dry_run",
            "confirmed": false,
            "hint": preview_hint(),
            "items": rendered,
            "count": rendered.len()
        }));
    }

    let remote = crate::remote::ImapSmtpRemote::new(&config);
    let mut pushed = 0usize;
    let mut failed = 0usize;
    let mut failures = Vec::new();
    let item_count = items.len();
    crate::progress::emit(
        &mut progress,
        "push_start",
        json!({
            "item_count": item_count,
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
        let result = match &item.payload {
            PushPayload::Outbound(_) => push_outbound(
                root,
                &config,
                &remote,
                &mut item,
                progress_context,
                progress.as_deref_mut(),
            ),
            PushPayload::MessageAction(_) => push_action_steps(
                root,
                &config,
                &remote,
                &mut item,
                progress_context,
                progress.as_deref_mut(),
            ),
        };
        match result {
            Ok(()) => {
                let workspace = crate::store::Workspace::at(root);
                let transaction = workspace.begin_transaction(
                    "push_commit",
                    vec![
                        format!(".afmail/push/{}.json", item.push_id),
                        "cases".to_string(),
                        "messages".to_string(),
                    ],
                )?;
                workspace.consume_outbound_draft_after_push(&item, &config)?;
                delete_item(root, &item)?;
                workspace.clear_pending_push_item(&item)?;
                transaction.commit()?;
                let _ = audit_push(root, "push_succeeded", &item, None);
                pushed += 1;
                crate::progress::emit(
                    &mut progress,
                    "push_item_done",
                    push_item_progress_fields(&item, index, item_count, None),
                );
            }
            Err(err) => {
                let _ = audit_push(root, "push_failed", &item, Some(&err));
                failed += 1;
                failures.push(json!({
                    "push_id": item.push_id,
                    "error_code": err.error_code,
                    "error": err.message
                }));
                item.attempt_count += 1;
                item.updated_rfc3339 = crate::store::now_rfc3339();
                item.last_error = Some(err.to_string());
                write_item(root, &item)?;
                crate::store::Workspace::at(root)
                    .mark_pending_push_error(&item, &err.to_string())?;
                crate::progress::emit(
                    &mut progress,
                    "push_item_failed",
                    push_item_progress_fields(&item, index, item_count, Some(&err)),
                );
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
        }),
    );
    Ok(json!({
        "code": "push_result",
        "confirmed": true,
        "pushed_count": pushed,
        "failed_count": failed,
        "failures": failures
    }))
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
    if let Some(err) = err {
        if let Value::Object(map) = &mut value {
            map.insert("error_code".to_string(), json!(err.error_code));
            map.insert("error".to_string(), json!(err.message.as_str()));
            map.insert("retryable".to_string(), json!(err.retryable));
        }
    }
    value
}

fn audit_push(root: &Path, kind: &str, item: &PushItem, err: Option<&AppError>) -> Result<()> {
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
    if let Some(outbound) = item.outbound() {
        if let Value::Object(map) = &mut fields {
            map.insert("case_uid".to_string(), json!(outbound.case_uid.as_str()));
            map.insert(
                "draft_name".to_string(),
                json!(outbound.draft_name.as_str()),
            );
            map.insert("action".to_string(), json!(outbound.action.as_str()));
        }
    }
    if let Some(err) = err {
        if let Value::Object(map) = &mut fields {
            map.insert("error_code".to_string(), json!(err.error_code));
            map.insert("error".to_string(), json!(err.message.as_str()));
            map.insert("retryable".to_string(), json!(err.retryable));
        }
    }
    crate::store::Workspace::at(root).append_audit_event(kind, targets, None, fields)
}

pub fn remove_outbound_for_draft(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Vec<RemovedOutbound>> {
    let items = read_items(root)?
        .into_iter()
        .filter(|item| {
            item.outbound().is_some_and(|outbound| {
                outbound.case_uid == case_uid && outbound.draft_name == draft_name
            })
        })
        .collect::<Vec<_>>();
    let mut removed = Vec::new();
    for item in items {
        removed.push(RemovedOutbound {
            push_id: item.push_id.clone(),
            action: item
                .outbound()
                .map(|outbound| outbound.action)
                .unwrap_or(OutboundAction::Send),
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
            item.message_action().is_some_and(|payload| {
                payload.action == action
                    && (payload.message_ids.iter().any(|id| id == message_id)
                        || payload
                            .locations
                            .iter()
                            .any(|loc| loc.message_id == message_id))
            })
        })
        .collect::<Vec<_>>();
    if let Some(item) = items.iter().find(|item| item.has_started_steps()) {
        return Err(AppError::new(
            "push_already_started",
            format!(
                "push item already started and cannot be undone locally: {}",
                item.push_id
            ),
        ));
    }

    let mut removed = Vec::new();
    for mut item in items {
        let push_id = item.push_id.clone();
        if let Some(payload) = item.message_action_mut() {
            payload.message_ids.retain(|id| id != message_id);
            payload.locations.retain(|loc| loc.message_id != message_id);
        }
        let empty = item
            .message_action()
            .is_some_and(|payload| payload.message_ids.is_empty() && payload.locations.is_empty());
        if empty {
            delete_item(root, &item)?;
        } else {
            item.updated_rfc3339 = crate::store::now_rfc3339();
            write_item(root, &item)?;
        }
        removed.push(RemovedMessagePush { push_id });
    }
    Ok(removed)
}

fn preview_hint() -> &'static str {
    "No remote changes were made. Re-run with --confirm to apply queued effects."
}
