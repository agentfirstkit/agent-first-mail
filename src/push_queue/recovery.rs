use super::*;
use crate::cli::SendSettlement;
use crate::remote::MailRemote;
use crate::types::{OutboundDelivery, RemoteLocation};

/// Confirmation rechecks the evidence while the caller holds the exclusive
/// workspace lock. Recovery never appends, sends, changes flags, or moves mail.
pub(crate) fn recover(
    root: &Path,
    push_id: &str,
    confirm_plan_id: Option<&str>,
    settle: Option<SendSettlement>,
) -> Result<Value> {
    let config = MailConfig::load(root)?;
    let remote = crate::remote::ImapSmtpRemote::new(&config);
    recover_with_remote(root, &config, &remote, push_id, confirm_plan_id, settle)
}

fn unsupported(push_id: &str) -> AppError {
    AppError::new(
        "push_recovery_unsupported",
        "recovery requires an uncertain first Drafts APPEND with every later step unstarted, or an uncertain send step with every earlier step succeeded and every later step unstarted",
    )
    .with_details(json!({"push_id": push_id}))
}

fn is_send_label(label: &str) -> bool {
    matches!(label, "smtp_send" | "cloudflare_send" | "file_send")
}

fn recover_with_remote(
    root: &Path,
    config: &MailConfig,
    remote: &dyn MailRemote,
    push_id: &str,
    confirm_plan_id: Option<&str>,
    settle: Option<SendSettlement>,
) -> Result<Value> {
    let item = read_items(root)?
        .into_iter()
        .find(|item| item.push_id == push_id)
        .ok_or_else(|| AppError::new("push_not_found", "queued push item was not found"))?;
    let uncertain_send = item
        .step_states
        .iter()
        .find(|state| state.status == PushStepStatus::RemoteOutcomeUnknown)
        .is_some_and(|state| is_send_label(&state.label));
    if uncertain_send {
        return recover_send(root, config, item, confirm_plan_id, settle);
    }
    if settle.is_some() {
        return Err(AppError::new(
            "push_recovery_settlement_unsupported",
            "--sent and --not-sent apply only to an uncertain send step",
        )
        .with_details(json!({"push_id": push_id})));
    }
    recover_drafts_append(root, config, remote, item, confirm_plan_id)
}

/// The recovery plan binds a confirmation to the exact queue record and the
/// evidence it was shown; any change in between makes the plan stale.
fn recovery_plan_id(root: &Path, original: &PushItem, evidence: &Value) -> Result<String> {
    let identity = crate::workspace_identity::load_workspace_identity(root)?;
    let item = serde_json::to_value(original)
        .map_err(|err| AppError::json("serialize push recovery item", &err))?;
    crate::plan_id::canonical_plan_id(
        "push_recover",
        &identity.workspace_uid,
        &json!({"item": item, "evidence": evidence}),
    )
}

fn require_matching_recovery_plan(
    push_id: &str,
    confirmed_plan_id: &str,
    plan_id: &str,
) -> Result<()> {
    if confirmed_plan_id == plan_id {
        return Ok(());
    }
    Err(AppError::new(
        "push_recovery_plan_changed",
        "the confirmed recovery plan does not match the current push item and evidence",
    )
    .with_hint(format!(
        "Run `afmail push recover {push_id}` again, review the new evidence, and confirm its new plan_id."
    ))
    .with_details(json!({
        "push_id": push_id,
        "confirmed_plan_id": confirmed_plan_id,
        "current_plan_id": plan_id,
        "remote_calls_started": false,
        "suggested_commands": [format!("afmail push recover {push_id}")]
    })))
}

fn audit_and_write(root: &Path, item: &PushItem, kind: &str, result: &Value) -> Result<()> {
    let case_uid = item
        .draft()
        .ok_or_else(|| unsupported(&item.push_id))?
        .case_uid
        .clone();
    // Audit the checked evidence and intended transition first. If it cannot
    // be retained, leave the uncertain item untouched.
    crate::store::Workspace::at(root).append_audit_event(
        kind,
        vec![
            json!({"kind": "push", "id": item.push_id}),
            json!({"kind": "case", "id": case_uid}),
        ],
        None,
        result.clone(),
    )?;
    write_item(root, item)?;
    crate::store::Workspace::at(root).refresh_outbound_push_views(item)
}

/// A person settles an uncertain send after checking reality. afmail has no
/// evidence that can prove either outcome, so it shows what it has, binds the
/// assertion to that evidence, and never retries the send by itself.
fn recover_send(
    root: &Path,
    config: &MailConfig,
    original: PushItem,
    confirm_plan_id: Option<&str>,
    settle: Option<SendSettlement>,
) -> Result<Value> {
    let push_id = original.push_id.clone();
    let mut item = original.clone();
    let unknown = item
        .step_states
        .iter()
        .position(|state| state.status == PushStepStatus::RemoteOutcomeUnknown)
        .ok_or_else(|| unsupported(&push_id))?;
    if item.draft().is_none()
        || item.local_stage.is_some()
        || item.audit_pending.is_some()
        || item.step_states.iter().enumerate().any(|(i, s)| {
            s.index != i
                || (i < unknown && s.status != PushStepStatus::Succeeded)
                || (i > unknown
                    && (s.status != PushStepStatus::Pending
                        || s.started_rfc3339.is_some()
                        || s.completed_rfc3339.is_some()))
        })
    {
        return Err(unsupported(&push_id));
    }
    if !plan_item(&crate::store::Workspace::at(root), config, &mut item)? {
        return Err(unsupported(&push_id));
    }
    let outbound = item.outbound().ok_or_else(|| unsupported(&push_id))?;
    let step = outbound
        .steps
        .get(unknown)
        .cloned()
        .ok_or_else(|| unsupported(&push_id))?;
    let state = item.step_states[unknown].clone();
    if item.step_states.len() != outbound.steps.len()
        || outbound.action != OutboundAction::Send
        || !step.is_send()
        || step_label(&step) != state.label
    {
        return Err(unsupported(&push_id));
    }
    let prepared = prepare_current_outbound(root, config, &item)?;
    let mut evidence = json!({
        "step_index": unknown + 1,
        "label": state.label,
        "started_rfc3339": state.started_rfc3339,
        "error_code": state.error_code,
        "error": state.error,
        "provider_message_id": state.send_receipt.as_ref().map(|receipt| receipt.message_id.as_str()),
        "send_receipt": state.send_receipt,
    });
    let mut output_confirmed = false;
    if let Value::Object(map) = &mut evidence {
        match &prepared.delivery {
            OutboundDelivery::Email(email) => {
                map.insert(
                    "rfc822_message_id".into(),
                    json!(outbound_rfc822_message_id(&item)),
                );
                map.insert("envelope_from".into(), json!(email.envelope_from));
                map.insert("envelope_to".into(), json!(email.envelope_to));
            }
            OutboundDelivery::File(route) => {
                output_confirmed = crate::channels::output_confirmed(root, config, &prepared)?;
                map.insert("reply_route".into(), json!(route));
                map.insert("output_confirmed".into(), json!(output_confirmed));
            }
        }
    }
    let plan_id = recovery_plan_id(root, &original, &evidence)?;
    let mut result = json!({
        "code": "push_recovery", "recovery_kind": "send_outcome", "push_id": push_id,
        "plan_id": plan_id, "confirmed": confirm_plan_id.is_some(),
        "settlement": settle.map(SendSettlement::as_str),
        "effect_scope": "local_recovery", "evidence": evidence,
    });
    let Some(confirmed_plan_id) = confirm_plan_id else {
        result["hint"] = json!(format!(
            "Nothing was sent or changed. afmail never retries an uncertain send by itself. Check the provider or the recipient, then settle exactly this plan: `afmail push recover {push_id} --sent --confirm {plan_id}` records the send as done so later steps run on the next confirmed push; `--not-sent --confirm {plan_id}` makes the send pending again, to be sent only by a newly previewed and confirmed push."
        ));
        return Ok(result);
    };
    let settle = settle.ok_or_else(|| {
        AppError::new(
            "push_recovery_settlement_required",
            "settling an uncertain send requires --sent or --not-sent",
        )
        .with_details(json!({"push_id": push_id, "plan_id": plan_id}))
    })?;
    require_matching_recovery_plan(&push_id, confirmed_plan_id, &plan_id)?;
    if settle == SendSettlement::NotSent && output_confirmed {
        return Err(AppError::new(
            "push_recovery_sent_evidence_found",
            "the file output for this send exists with the exact content; it was published",
        )
        .with_hint(format!(
            "Settle it with `afmail push recover {push_id} --sent --confirm {plan_id}`."
        ))
        .with_details(json!({"push_id": push_id, "evidence": result["evidence"]})));
    }
    let previous_step_states = item.step_states.clone();
    match settle {
        SendSettlement::Sent => {
            mark_step_succeeded(&mut item, unknown, &step);
            item.step_states[unknown].result_summary =
                Some("settled as sent by a person through push recover".to_string());
        }
        SendSettlement::NotSent => {
            // Keep the diagnostic visible until the next execution starts; the
            // audit retains the exact previous step either way.
            mark_step_recovered_pending(
                &mut item.step_states[unknown],
                "settled as not sent by a person through push recover; sending requires a newly confirmed push plan",
            );
        }
    }
    item.updated_rfc3339 = crate::store::now_rfc3339();
    item.last_error = None;
    result["asserted_by"] = json!("person");
    result["previous_step_states"] = json!(previous_step_states);
    result["step_states"] = json!(item.step_states);
    result["hint"] = json!(
        "Recovery changed only local queue state. Preview `afmail push` and confirm its plan to run the remaining steps."
    );
    audit_and_write(root, &item, "push_recovery_settled", &result)?;
    Ok(result)
}

fn recover_drafts_append(
    root: &Path,
    config: &MailConfig,
    remote: &dyn MailRemote,
    original: PushItem,
    confirm_plan_id: Option<&str>,
) -> Result<Value> {
    let push_id = original.push_id.clone();
    let push_id = push_id.as_str();
    let mut item = original.clone();
    // Validate the durable evidence before resolving today's config or draft.
    if item.draft().is_none()
        || item.local_stage.is_some()
        || item.audit_pending.is_some()
        || item
            .step_states
            .first()
            .is_none_or(|s| s.index != 0 || s.status != PushStepStatus::RemoteOutcomeUnknown)
        || item.step_states.iter().skip(1).enumerate().any(|(i, s)| {
            s.index != i + 1
                || s.status != PushStepStatus::Pending
                || s.started_rfc3339.is_some()
                || s.completed_rfc3339.is_some()
        })
    {
        return Err(unsupported(push_id));
    }
    if !plan_item(&crate::store::Workspace::at(root), config, &mut item)? {
        return Err(unsupported(push_id));
    }
    let outbound = item.outbound().ok_or_else(|| unsupported(push_id))?;
    let first = outbound.steps.first().ok_or_else(|| unsupported(push_id))?;
    let mailbox_id = first
        .append_to_mailbox_id
        .as_deref()
        .ok_or_else(|| unsupported(push_id))?;
    if item.step_states.len() != outbound.steps.len()
        || outbound.action != OutboundAction::Send
        || outbound.draft_uid.is_some()
        || outbound.draft_uid_validity.is_some()
        || !mailbox_is_kind(config, mailbox_id, SpecialUseKind::Drafts)
        || first.is_send()
        || first.move_to_mailbox_id.is_some()
        || !first.add_flags.is_empty()
        || !outbound.steps.iter().skip(1).any(|step| step.is_send())
    {
        return Err(unsupported(push_id));
    }
    let rfc822_message_id = outbound_rfc822_message_id(&item);
    let drafts_folder = remote.action_mailbox_folder(mailbox_id)?;
    let mut sent_folders = BTreeSet::new();
    for mailbox_id in config.mailboxes.keys() {
        if mailbox_is_kind(config, mailbox_id, SpecialUseKind::Sent) {
            sent_folders.insert(remote.action_mailbox_folder(mailbox_id)?);
        }
    }
    if sent_folders.is_empty() || sent_folders.contains(&drafts_folder) {
        return Err(AppError::new(
            "push_recovery_mailboxes_invalid",
            "recovery requires distinct resolved Drafts and Sent mailboxes",
        ));
    }
    let draft_location = lookup(remote, &drafts_folder, &rfc822_message_id)?;
    let mut evidence = vec![json!({"mailbox_name": drafts_folder, "location": draft_location})];
    let mut found_sent = false;
    for folder in sent_folders {
        let location = lookup(remote, &folder, &rfc822_message_id)?;
        found_sent |= location.is_some();
        evidence.push(json!({"mailbox_name": folder, "location": location}));
    }
    if found_sent {
        return Err(AppError::new("push_recovery_sent_copy_found", "a Sent copy exists; inspect delivery before recovering this push")
            .with_details(json!({"push_id": push_id, "rfc822_message_id": rfc822_message_id, "evidence": evidence})));
    }
    let resolution = if draft_location.is_some() {
        "draft_found"
    } else {
        "draft_absent"
    };
    let evidence = json!(evidence);
    let plan_id = recovery_plan_id(
        root,
        &original,
        &json!({"resolution": resolution, "mailboxes": evidence}),
    )?;
    let mut result = json!({
        "code": "push_recovery", "recovery_kind": "drafts_append", "push_id": push_id,
        "plan_id": plan_id, "confirmed": confirm_plan_id.is_some(),
        "effect_scope": "local_recovery", "rfc822_message_id": rfc822_message_id,
        "resolution": resolution, "evidence": evidence,
        "hint": format!("Recovery changes only local queue state; apply exactly this check with `afmail push recover {push_id} --confirm {plan_id}`, then preview afmail push and confirm its plan separately. Absence is a point-in-time check; an external client can change the mailbox later.")
    });
    if let Some(confirmed_plan_id) = confirm_plan_id {
        require_matching_recovery_plan(push_id, confirmed_plan_id, &plan_id)?;
        let previous_step_states = item.step_states.clone();
        let first = item.outbound().ok_or_else(|| unsupported(push_id))?.steps[0].clone();
        if draft_location.is_some() {
            mark_step_succeeded(&mut item, 0, &first);
        } else {
            // Release the started marker so normal local cancellation/requeueing
            // works too. The audit retains the exact previous step and diagnostic;
            // keep the diagnostic visible here until the next execution starts.
            mark_step_recovered_pending(
                &mut item.step_states[0],
                "explicit recovery found no Drafts or Sent copy; APPEND may be retried",
            );
        }
        item.updated_rfc3339 = crate::store::now_rfc3339();
        item.last_error = None;
        result["previous_step_states"] = json!(previous_step_states);
        result["step_states"] = json!(item.step_states);
        audit_and_write(root, &item, "push_recovery_checked", &result)?;
    }
    Ok(result)
}

fn lookup(
    remote: &dyn MailRemote,
    folder: &str,
    message_id: &str,
) -> Result<Option<RemoteLocation>> {
    match remote.find_by_message_id(folder, message_id) {
        // Only the adapter's explicit zero-result condition is absence.
        Err(error) if error.error_code == "imap_uid_missing" => Ok(None),
        result => result,
    }
}
