use super::*;
use crate::config::ResolvedIdentity;
use serde::Serialize;

const PUSH_PLAN_SCHEMA_NAME: &str = "push_plan";
const PUSH_PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub(super) struct ResolvedPushPlan {
    pub(super) plan_id: String,
    pub(super) workspace_uid: String,
    pub(super) workspace_path: String,
    pub(super) config_effect_id: String,
    pub(super) items: Vec<PushItem>,
    pub(super) skipped_items: Vec<PushItem>,
    item_views: Vec<PushPlanItemView>,
    skipped_views: Vec<PushPlanSkippedItem>,
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanDigest<'a> {
    schema_name: &'static str,
    schema_version: u32,
    config_effect_id: &'a str,
    queue_records: &'a [PushQueueRecord],
    items: &'a [PushPlanItemView],
    skipped_items: &'a [PushPlanSkippedItem],
}

#[derive(Clone, Debug, Serialize)]
struct PushQueueRecord {
    push_id: String,
    content_hash: String,
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanItemView {
    push_id: String,
    queue_item_hash: String,
    kind: String,
    display_kind: String,
    effect_scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    case_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    actions: Vec<String>,
    steps: Vec<ActionStep>,
    step_states: Vec<PushStepState>,
    attempt_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_stage: Option<PushLocalStage>,
    audit_pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    message_ids: Vec<String>,
    locations: Vec<PushLocation>,
    outbound: Option<PushPlanOutbound>,
    message_content_hashes: Vec<PushPlanMessageContent>,
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanOutbound {
    case_uid: String,
    draft_name: String,
    review_path: Option<String>,
    draft_hash: Option<String>,
    draft_size_bytes: Option<u64>,
    mime_hash: Option<String>,
    identity: Option<PushPlanIdentity>,
    identity_placeholder: bool,
    warnings: Vec<&'static str>,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    attachments: Vec<PushPlanAttachment>,
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanIdentity {
    identity: String,
    name: String,
    email: String,
}

impl From<ResolvedIdentity> for PushPlanIdentity {
    fn from(identity: ResolvedIdentity) -> Self {
        Self {
            identity: identity.identity,
            name: identity.name,
            email: identity.email,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanAttachment {
    path: String,
    filename: String,
    size_bytes: u64,
    content_hash: String,
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanMessageContent {
    message_id: String,
    content_hash: String,
    size_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
struct PushPlanSkippedItem {
    push_id: String,
    queue_item_hash: String,
    reason: &'static str,
}

#[derive(Serialize)]
struct PushPlanOutput<'a> {
    code: &'a str,
    schema_name: &'static str,
    schema_version: u32,
    confirmed: bool,
    workspace_uid: &'a str,
    workspace_path: &'a str,
    plan_id: &'a str,
    config_effect_id: &'a str,
    count: usize,
    items: &'a [PushPlanItemView],
    skipped_count: usize,
    skipped_items: &'a [PushPlanSkippedItem],
    hint: String,
}

pub(super) fn build_push_plan(root: &Path, config: &MailConfig) -> Result<ResolvedPushPlan> {
    let identity = crate::workspace_identity::load_workspace_identity(root)?;
    let workspace_path = canonical_workspace_path(root)?;
    let queue_records = queue_records(root)?;
    let queue_hashes = queue_records
        .iter()
        .map(|record| (record.push_id.as_str(), record.content_hash.as_str()))
        .collect::<BTreeMap<_, _>>();
    let config_effect_id = config_effect_id(config, &identity.workspace_uid)?;
    let mut items = Vec::new();
    let mut skipped_items = Vec::new();
    let mut skipped_views = Vec::new();
    for mut item in sorted_items(root)? {
        let queue_item_hash = queue_hashes
            .get(item.push_id.as_str())
            .copied()
            .ok_or_else(|| {
                AppError::new(
                    "push_plan_invalid",
                    format!("queue snapshot is missing push item {}", item.push_id),
                )
            })?;
        if plan_item(root, config, &mut item)? {
            items.push(item);
        } else {
            skipped_views.push(PushPlanSkippedItem {
                push_id: item.push_id.clone(),
                queue_item_hash: queue_item_hash.to_string(),
                reason: "no_longer_planned",
            });
            skipped_items.push(item);
        }
    }
    order_items_for_dependencies(&mut items);
    let mut item_views = Vec::with_capacity(items.len());
    for item in &items {
        let queue_item_hash = queue_hashes
            .get(item.push_id.as_str())
            .copied()
            .ok_or_else(|| {
                AppError::new(
                    "push_plan_invalid",
                    format!("queue snapshot is missing push item {}", item.push_id),
                )
            })?;
        item_views.push(push_plan_item_view(root, config, item, queue_item_hash)?);
    }
    let digest = PushPlanDigest {
        schema_name: PUSH_PLAN_SCHEMA_NAME,
        schema_version: PUSH_PLAN_SCHEMA_VERSION,
        config_effect_id: &config_effect_id,
        queue_records: &queue_records,
        items: &item_views,
        skipped_items: &skipped_views,
    };
    let plan_id = crate::plan_id::canonical_plan_id("push", &identity.workspace_uid, &digest)?;
    Ok(ResolvedPushPlan {
        plan_id,
        workspace_uid: identity.workspace_uid,
        workspace_path,
        config_effect_id,
        items,
        skipped_items,
        item_views,
        skipped_views,
    })
}

pub(super) fn push_plan_output(plan: &ResolvedPushPlan, code: &str) -> Result<Value> {
    serde_json::to_value(PushPlanOutput {
        code,
        schema_name: PUSH_PLAN_SCHEMA_NAME,
        schema_version: PUSH_PLAN_SCHEMA_VERSION,
        confirmed: false,
        workspace_uid: &plan.workspace_uid,
        workspace_path: &plan.workspace_path,
        plan_id: &plan.plan_id,
        config_effect_id: &plan.config_effect_id,
        count: plan.item_views.len(),
        items: &plan.item_views,
        skipped_count: plan.skipped_views.len(),
        skipped_items: &plan.skipped_views,
        hint: format!(
            "No remote changes were made. Apply exactly this reviewed plan with `afmail push --confirm {}`.",
            plan.plan_id
        ),
    })
    .map_err(|err| AppError::json("serialize push plan", &err))
}

pub(super) fn require_matching_push_plan(
    plan: &ResolvedPushPlan,
    confirmed_plan_id: &str,
) -> Result<()> {
    if confirmed_plan_id == plan.plan_id {
        return Ok(());
    }
    Err(AppError::new(
        "push_plan_changed",
        "the confirmed push plan does not match the current resolved queue",
    )
    .with_hint(
        "Run `afmail push` again, review the complete new plan, and confirm its new plan_id.",
    )
    .with_details(json!({
        "confirmed_plan_id": confirmed_plan_id,
        "current_plan_id": plan.plan_id,
        "workspace_uid": plan.workspace_uid,
        "workspace_path": plan.workspace_path,
        "remote_calls_started": false,
        "suggested_commands": ["afmail push"]
    })))
}

fn order_items_for_dependencies(items: &mut Vec<PushItem>) {
    // Queue order remains FIFO unless one planned effect would invalidate a local
    // reference needed by another. A case archive can reuse the earlier message
    // item's enqueue_seq from case.add, so lift its queued reply sends just ahead of
    // that move. Selecting the first dependency-free item keeps unrelated items stable.
    let mut remaining = std::mem::take(items);
    let mut ordered = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .position(|candidate| {
                !remaining
                    .iter()
                    .any(|item| item_must_run_before(item, candidate))
            })
            .unwrap_or(0);
        ordered.push(remaining.remove(next));
    }
    *items = ordered;
}

fn item_must_run_before(item: &PushItem, candidate: &PushItem) -> bool {
    let Some(outbound) = item.outbound() else {
        return false;
    };
    if outbound.action != OutboundAction::Send {
        return false;
    }
    let Some(reply_to_message_id) = outbound.reply_to_message_id.as_deref() else {
        return false;
    };
    let Some(message_action) = candidate.message_action() else {
        return false;
    };
    if candidate.has_started_effects()
        || !matches!(
            message_action.action,
            MessagePushAction::Notification | MessagePushAction::Spam | MessagePushAction::Trash
        )
    {
        return false;
    }
    message_action
        .message_ids
        .iter()
        .any(|message_id| message_id == reply_to_message_id)
}

pub(super) fn reconcile_push_plan(root: &Path, plan: &ResolvedPushPlan) -> Result<Vec<String>> {
    for item in &plan.items {
        write_item(root, item)?;
    }
    let mut removed = Vec::new();
    for item in &plan.skipped_items {
        delete_item(root, item)?;
        removed.push(item.push_id.clone());
    }
    Ok(removed)
}

fn queue_records(root: &Path) -> Result<Vec<PushQueueRecord>> {
    let mut records = Vec::new();
    for item in sorted_items(root)? {
        let path = push_path(root, &item.push_id);
        let bytes =
            fs::read(&path).map_err(|err| AppError::io("read push plan queue item", &err))?;
        records.push(PushQueueRecord {
            push_id: item.push_id,
            content_hash: crate::util::blake3_fingerprint(&bytes),
        });
    }
    records.sort_by(|left, right| left.push_id.cmp(&right.push_id));
    Ok(records)
}

fn config_effect_id(config: &MailConfig, workspace_uid: &str) -> Result<String> {
    let mut value = serde_json::to_value(config)
        .map_err(|err| AppError::json("serialize push effect config", &err))?;
    for section in ["imap", "smtp"] {
        if let Some(object) = value.get_mut(section).and_then(Value::as_object_mut) {
            object.remove("password_secret");
        }
    }
    crate::plan_id::canonical_plan_id("push_effect_config", workspace_uid, &value)
}

fn push_plan_item_view(
    root: &Path,
    config: &MailConfig,
    item: &PushItem,
    queue_item_hash: &str,
) -> Result<PushPlanItemView> {
    let actions = actions_for(root, config, item)?;
    let steps = item
        .message_action()
        .map(|action| action.steps.clone())
        .or_else(|| item.outbound().map(|outbound| outbound.steps.clone()))
        .unwrap_or_default();
    let completed_steps = item.succeeded_step_count();
    let effect_scope = if item.local_stage.is_some() || completed_steps >= steps.len() {
        "local_recovery"
    } else {
        "remote"
    };
    let outbound = match item.outbound() {
        Some(outbound) => Some(push_plan_outbound(root, config, item, outbound)?),
        None => None,
    };
    let message_content_hashes = item
        .planned_message_rfc822
        .iter()
        .map(|(message_id, raw)| PushPlanMessageContent {
            message_id: message_id.clone(),
            content_hash: crate::util::blake3_fingerprint(raw),
            size_bytes: raw.len() as u64,
        })
        .collect();
    Ok(PushPlanItemView {
        push_id: item.push_id.clone(),
        queue_item_hash: queue_item_hash.to_string(),
        kind: item.kind().to_string(),
        display_kind: item.display_kind(),
        effect_scope,
        action: item
            .outbound()
            .map(|outbound| outbound.action.as_str().to_string())
            .or_else(|| {
                item.message_action()
                    .map(|action| action.action.mode_label().to_string())
            }),
        case_uid: item.outbound().map(|outbound| outbound.case_uid.clone()),
        draft_name: item.outbound().map(|outbound| outbound.draft_name.clone()),
        message_id: item.message().map(|message| message.message_id.clone()),
        actions,
        steps,
        step_states: item.step_states.clone(),
        attempt_count: item.attempt_count,
        local_stage: item.local_stage,
        audit_pending: item.audit_pending.is_some(),
        last_error: item.last_error.clone(),
        message_ids: item.message_ids().to_vec(),
        locations: item.locations().to_vec(),
        outbound,
        message_content_hashes,
    })
}

fn push_plan_outbound(
    root: &Path,
    config: &MailConfig,
    item: &PushItem,
    outbound: &OutboundPush,
) -> Result<PushPlanOutbound> {
    let case_path = find_case_path(root, &outbound.case_uid)?;
    let draft_path = case_path.join("drafts").join(&outbound.draft_name);
    if !draft_path.is_file() || item.planned_outbound_message.is_none() {
        return Ok(PushPlanOutbound {
            case_uid: outbound.case_uid.clone(),
            draft_name: outbound.draft_name.clone(),
            review_path: draft_path
                .is_file()
                .then(|| relative_path(root, &draft_path)),
            draft_hash: None,
            draft_size_bytes: None,
            mime_hash: None,
            identity: None,
            identity_placeholder: false,
            warnings: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            attachments: Vec::new(),
        });
    }
    let draft_bytes =
        fs::read(&draft_path).map_err(|err| AppError::io("read planned draft", &err))?;
    let draft_text = std::str::from_utf8(&draft_bytes)
        .map_err(|err| AppError::new("draft_invalid", format!("draft is not UTF-8: {err}")))?;
    let (frontmatter, _) = crate::markdown::read_doc::<DraftFrontmatter>(draft_text)?;
    let identity = config.resolve_identity(root, frontmatter.identity.as_deref())?;
    let identity_placeholder = identity.email.eq_ignore_ascii_case("me@example.com")
        || (identity.identity == "me" && identity.name == "Me");
    let mut attachments = Vec::new();
    for attachment in &frontmatter.attachments {
        let open = crate::attachments::open_draft_attachment(&case_path, attachment)?;
        let bytes = crate::attachments::read_attachment(&open)?;
        attachments.push(PushPlanAttachment {
            path: relative_path(root, &open.path),
            filename: open.filename.clone(),
            size_bytes: bytes.len() as u64,
            content_hash: crate::util::blake3_fingerprint(&bytes),
        });
    }
    let prepared = item.planned_outbound_message.as_ref().ok_or_else(|| {
        AppError::new(
            "push_plan_invalid",
            format!("planned outbound content is missing for {}", item.push_id),
        )
    })?;
    Ok(PushPlanOutbound {
        case_uid: outbound.case_uid.clone(),
        draft_name: outbound.draft_name.clone(),
        review_path: Some(relative_path(root, &draft_path)),
        draft_hash: Some(crate::util::blake3_fingerprint(&draft_bytes)),
        draft_size_bytes: Some(draft_bytes.len() as u64),
        mime_hash: Some(crate::util::blake3_fingerprint(&prepared.raw)),
        identity: Some(identity.into()),
        identity_placeholder,
        warnings: if identity_placeholder {
            vec!["placeholder_identity_must_be_replaced_before_confirming_send"]
        } else {
            Vec::new()
        },
        to: frontmatter.to,
        cc: frontmatter.cc,
        bcc: frontmatter.bcc,
        subject: frontmatter.subject.unwrap_or_default(),
        attachments,
    })
}

fn canonical_workspace_path(root: &Path) -> Result<String> {
    let path =
        fs::canonicalize(root).map_err(|err| AppError::io("canonicalize workspace", &err))?;
    Ok(path.to_string_lossy().replace('\\', "/"))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_item(push_id: &str, enqueue_seq: u64, payload: PushPayload) -> PushItem {
        PushItem {
            schema_name: "push_item".to_string(),
            schema_version: PUSH_ITEM_SCHEMA_VERSION,
            push_id: push_id.to_string(),
            enqueue_seq,
            payload,
            planned_outbound: None,
            planned_outbound_message: None,
            planned_message_action: None,
            planned_message_rfc822: BTreeMap::new(),
            created_rfc3339: "2026-07-20T00:00:00Z".to_string(),
            updated_rfc3339: "2026-07-20T00:00:00Z".to_string(),
            attempt_count: 0,
            step_states: Vec::new(),
            local_stage: None,
            audit_pending: None,
            last_error: None,
        }
    }

    fn reply_send_item(push_id: &str, enqueue_seq: u64, message_id: &str) -> PushItem {
        let mut item = push_item(
            push_id,
            enqueue_seq,
            PushPayload::Draft(Box::new(DraftPush {
                case_uid: "c20260720001".to_string(),
                draft_name: "reply.md".to_string(),
                draft_uid_validity: None,
                draft_uid: None,
            })),
        );
        item.planned_outbound = Some(OutboundPush {
            action: OutboundAction::Send,
            case_uid: "c20260720001".to_string(),
            draft_name: "reply.md".to_string(),
            draft_uid_validity: None,
            draft_uid: None,
            steps: Vec::new(),
            reply_to_message_id: Some(message_id.to_string()),
        });
        item
    }

    fn archive_item(push_id: &str, enqueue_seq: u64, message_id: &str) -> PushItem {
        let mut item = push_item(
            push_id,
            enqueue_seq,
            PushPayload::Message(MessagePush {
                message_id: message_id.to_string(),
                action: MessagePushAction::Notification,
                message_action_snapshot: None,
            }),
        );
        item.planned_message_action = Some(crate::types::MessageActionPush {
            action: MessagePushAction::Notification,
            message_ids: vec![message_id.to_string()],
            locations: Vec::new(),
            steps: Vec::new(),
            reply_to_message_id: None,
        });
        item
    }

    #[test]
    fn orders_reply_send_before_related_archive_and_keeps_unrelated_fifo() {
        let related_message_id = "message_related";
        let mut items = vec![
            archive_item("push_archive_related", 1, related_message_id),
            archive_item("push_archive_unrelated", 2, "message_unrelated"),
            reply_send_item("push_reply", 3, related_message_id),
        ];

        order_items_for_dependencies(&mut items);

        assert_eq!(
            items
                .iter()
                .map(|item| item.push_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "push_archive_unrelated",
                "push_reply",
                "push_archive_related"
            ]
        );
    }
}
