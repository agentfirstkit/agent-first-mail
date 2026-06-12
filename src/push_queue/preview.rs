use super::*;

pub(super) fn filtered_items(root: &Path, mode: PushMode) -> Result<Vec<PushItem>> {
    let mut items = sorted_items(root)?;
    items.retain(|item| match mode {
        PushMode::All => true,
        PushMode::Drafts | PushMode::DraftsSend => item.outbound().is_some(),
        PushMode::Archive => item
            .message_action()
            .is_some_and(|payload| payload.action == MessagePushAction::Archive),
        PushMode::Spam => item
            .message_action()
            .is_some_and(|payload| payload.action == MessagePushAction::Spam),
        PushMode::Trash => item
            .message_action()
            .is_some_and(|payload| payload.action == MessagePushAction::Trash),
    });
    Ok(items)
}

pub(super) fn actions_for(mode: PushMode, item: &PushItem) -> Vec<String> {
    if let Some(payload) = item.message_action() {
        return payload.steps.iter().map(step_label).collect();
    }
    if let Some(outbound) = item.outbound() {
        let steps = match mode {
            PushMode::DraftsSend => &outbound.draft_send_steps,
            _ => &outbound.draft_save_steps,
        };
        if !steps.is_empty() {
            return steps
                .iter()
                .filter(|step| {
                    outbound.reply_to_message_id.is_some()
                        || step.on != Some(ActionStepOn::ReplyToMessage)
                })
                .map(step_label)
                .collect();
        }
        return match mode {
            PushMode::DraftsSend => {
                vec![
                    "smtp_send".to_string(),
                    "append_to_mailbox_id_sent".to_string(),
                ]
            }
            _ => vec!["append_to_mailbox_id_drafts".to_string()],
        };
    }
    Vec::new()
}

pub(super) fn item_summary_label(item: &PushItem) -> &str {
    match &item.payload {
        PushPayload::Outbound(_) => "drafts",
        PushPayload::MessageAction(payload) => payload.action.mode_label(),
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
