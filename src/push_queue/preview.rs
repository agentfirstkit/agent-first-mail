use super::*;

pub(super) fn actions_for(
    root: &Path,
    config: &MailConfig,
    item: &PushItem,
) -> Result<Vec<String>> {
    if let Some(payload) = item.message_action() {
        return Ok(payload.steps.iter().map(step_label).collect());
    }
    if let Some(outbound) = item.outbound() {
        if !outbound.steps.is_empty() {
            return Ok(outbound.steps.iter().map(step_label).collect());
        }
        let steps = match outbound.action {
            OutboundAction::SaveDraft => &config.actions.draft_save.steps,
            OutboundAction::Send => &config.actions.draft_send.steps,
        };
        if !steps.is_empty() {
            return Ok(filter_reply_to_steps(root, item, steps.clone())?
                .iter()
                .map(step_label)
                .collect());
        }
        return Ok(match outbound.action {
            OutboundAction::Send => {
                vec![
                    "append_to_mailbox_id_drafts".to_string(),
                    "smtp_send".to_string(),
                    "append_to_mailbox_id_sent".to_string(),
                ]
            }
            OutboundAction::SaveDraft => vec!["append_to_mailbox_id_drafts".to_string()],
        });
    }
    Ok(Vec::new())
}

pub(super) fn item_summary_label(item: &PushItem) -> &str {
    if item.outbound().is_some() {
        return "drafts";
    }
    if let Some(payload) = item.message_action() {
        return payload.action.mode_label();
    }
    match &item.payload {
        PushPayload::Draft(_) => "drafts",
        PushPayload::Message(_) => "message",
    }
}

pub(super) fn step_label(step: &ActionStep) -> String {
    if !step.add_flags.is_empty() {
        let target = step.on.map(|on| format!(":{on:?}")).unwrap_or_default();
        return format!("add_flags{target}");
    }
    if let Some(mailbox_id) = &step.move_to_mailbox_id {
        return format!("move_to_mailbox_id_{mailbox_id}");
    }
    if let Some(mailbox_id) = &step.append_to_mailbox_id {
        return format!("append_to_mailbox_id_{mailbox_id}");
    }
    if step.smtp_send.is_some() {
        return "smtp_send".to_string();
    }
    "noop".to_string()
}
