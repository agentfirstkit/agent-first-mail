use super::*;

impl Workspace {
    pub(crate) fn record_pending_push_item(&self, item: &PushItem) -> Result<()> {
        let pending = WorkspacePendingPush {
            push_id: item.push_id.clone(),
            kind: item.display_kind(),
            queued_rfc3339: item.created_rfc3339.clone(),
            last_error: item.last_error.clone(),
        };
        for message_id in item.message_ids() {
            self.update_message_push_state(message_id, |state| {
                state
                    .pending
                    .retain(|entry| entry.push_id != pending.push_id);
                state.pending.push(pending.clone());
                state.pending.sort_by(|a, b| a.push_id.cmp(&b.push_id));
            })?;
        }
        Ok(())
    }

    pub(crate) fn clear_pending_push_item(&self, item: &PushItem) -> Result<()> {
        let push_ids = [item.push_id.clone()];
        let message_ids = item.message_ids();
        for message_id in message_ids {
            self.clear_message_pending_pushes(message_id, &push_ids, true)?;
        }
        Ok(())
    }

    pub(crate) fn refresh_outbound_push_views(&self, item: &PushItem) -> Result<()> {
        let Some(outbound) = item.outbound() else {
            return Ok(());
        };
        let (_, case_path) = self.resolve_active_case(&outbound.case_uid)?;
        self.refresh_case_message_views(&case_path)
    }

    pub(crate) fn refresh_message_push_views(&self, message_ids: &[String]) -> Result<()> {
        if message_ids.is_empty() {
            return Ok(());
        }
        self.refresh_messages_after_ref_change(message_ids)?;
        self.refresh_disposition_views()?;
        let cases = CaseIndex::build(self)?;
        for case_path in cases.case_paths_for_messages(message_ids) {
            self.refresh_case_message_views(&case_path)?;
        }
        let mut notification_uids = BTreeSet::new();
        for message_id in message_ids {
            let message = self.read_message_by_id(message_id)?;
            if let Some(notification_uid) = message.workspace.notification_uid {
                notification_uids.insert(notification_uid);
            }
        }
        for notification_uid in notification_uids {
            self.refresh_notification(&notification_uid)?;
        }
        Ok(())
    }

    pub(crate) fn mark_pending_push_error(&self, item: &PushItem, error: &str) -> Result<()> {
        let message_ids = item.message_ids();
        for message_id in message_ids {
            self.update_message_push_state(message_id, |state| {
                for pending in &mut state.pending {
                    if pending.push_id == item.push_id {
                        pending.last_error = Some(error.to_string());
                    }
                }
            })?;
        }
        if !message_ids.is_empty() {
            self.refresh_disposition_views()?;
        }
        Ok(())
    }

    pub(super) fn update_message_push_state(
        &self,
        message_id: &str,
        update: impl FnOnce(&mut WorkspacePushState),
    ) -> Result<()> {
        validate_id("message_id", message_id)?;
        let mut message = self.read_message_by_id(message_id)?;
        let mut state = message.workspace.push.unwrap_or_default();
        update(&mut state);
        if state.pending.is_empty() && state.last_completed_rfc3339.is_none() {
            message.workspace.push = None;
        } else {
            message.workspace.push = Some(state);
        }
        self.write_message_materialized_cache(&message)
    }

    pub(super) fn clear_message_pending_pushes(
        &self,
        message_id: &str,
        push_ids: &[String],
        completed: bool,
    ) -> Result<()> {
        if push_ids.is_empty() {
            return Ok(());
        }
        let remove = push_ids.iter().cloned().collect::<BTreeSet<_>>();
        validate_id("message_id", message_id)?;
        let mut message = read_message(&self.message_path(message_id))?;
        let mut state = message.workspace.push.unwrap_or_default();
        let before = state.pending.len();
        state
            .pending
            .retain(|pending| !remove.contains(&pending.push_id));
        if completed && (state.pending.len() != before || before == 0) {
            state.last_completed_rfc3339 = Some(now_rfc3339());
        }
        if state.pending.is_empty() && state.last_completed_rfc3339.is_none() {
            message.workspace.push = None;
        } else {
            message.workspace.push = Some(state);
        }
        self.write_message_materialized_cache(&message)
    }

    pub(super) fn remove_pending_message_pushes_for_messages(
        &self,
        message_ids: &[String],
        push_kind: &str,
    ) -> Result<Vec<String>> {
        let mut push_ids = BTreeSet::new();
        for message_id in message_ids {
            let removed = crate::push_queue::remove_pending_message_pushes(
                &self.root, message_id, push_kind,
            )?;
            let message_push_ids = removed
                .iter()
                .map(|item| item.push_id.clone())
                .collect::<Vec<_>>();
            self.clear_message_pending_pushes(message_id, &message_push_ids, false)?;
            push_ids.extend(message_push_ids);
        }
        if !push_ids.is_empty() {
            self.refresh_disposition_views()?;
        }
        Ok(push_ids.into_iter().collect())
    }
}
