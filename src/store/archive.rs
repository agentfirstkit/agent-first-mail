use super::*;

pub(super) fn archive_index_field_value(
    field: NotificationIndexField,
    message: &MessageFile,
    item: &NotificationMessageItem,
    offset: &FixedOffset,
) -> Option<Value> {
    let value = match field {
        NotificationIndexField::Time => message_time_datetime(message, offset).unwrap_or_default(),
        NotificationIndexField::From => message.from.clone().unwrap_or_default(),
        NotificationIndexField::To => message.to.join(", "),
        NotificationIndexField::Subject => message.subject.clone().unwrap_or_default(),
        NotificationIndexField::Summary => item.summary.clone().unwrap_or_default(),
        NotificationIndexField::MessageId => item.message_id.clone(),
        NotificationIndexField::NotificationTime => item.added_rfc3339.clone(),
        NotificationIndexField::Link => String::new(),
    };
    let keep_empty = matches!(
        field,
        NotificationIndexField::Time
            | NotificationIndexField::From
            | NotificationIndexField::Link
            | NotificationIndexField::MessageId
    );
    if value.is_empty() && !keep_empty {
        None
    } else {
        Some(json!({
            "kind": field.as_str(),
            "value": value,
            "href": format!("views/messages/{}.md", item.message_id),
        }))
    }
}

impl Workspace {
    pub fn create_notification(
        &self,
        name: &str,
        message_id: Option<&str>,
        summary: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_name("notification_name", name)?;
        if let Some(message_id) = message_id {
            validate_id("message_id", message_id)?;
            if summary
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err(AppError::new(
                    "invalid_request",
                    "--summary is required when --message is supplied",
                ));
            }
        }
        let date = workspace_local_date(&self.workspace_date_offset()?);
        if let Some(message_id) = message_id {
            self.ensure_no_related_conversation(message_id)?;
        }
        let notification_uid = self.next_notification_uid(&date)?;
        let notification_dir = self.notification_dir_for_name(&notification_uid, name);
        if notification_dir.exists() {
            return Err(AppError::new(
                "notification_exists",
                format!(
                    "notification category already exists: {}",
                    path_to_string(&notification_dir)
                ),
            ));
        }
        create_dir_all(&notification_dir.join("data"))?;
        create_dir_all(&notification_dir.join("views/messages"))?;
        let mut guard = DirGuard::new(notification_dir.clone());
        write_string_new(
            &notification_dir.join("notes.md"),
            &new_notes_md(&self.root, self.template_language()?)?,
        )?;
        self.write_notifications_named(
            &notification_uid,
            name,
            &NotificationMessages::new_notification(&notification_uid, name, &now_rfc3339()),
        )?;

        let mut result = json!({
            "code": "notification_created",
            "notification_uid": notification_uid,
            "notification_name": name,
            "message_count": 0,
            "path": rel_path(&self.root, &notification_dir),
        });
        if let Some(message_id) = message_id {
            let notification_rfc3339 =
                self.set_direct_message_notification(message_id, &notification_uid)?;
            self.upsert_notification_item(
                &notification_uid,
                message_id,
                summary,
                &notification_rfc3339,
            )?;
            self.refresh_notification(&notification_uid)?;
            let queue =
                self.queue_archive_for_archived_messages(&[message_id.to_string()], None)?;
            result = json!({
                "code": "notification_created",
                "notification_uid": notification_uid,
                "notification_name": name,
                "message_id": message_id,
                "summary": summary,
                "message_count": 1,
                "path": rel_path(&self.root, &notification_dir),
                "eligible_message_ids": queue.eligible_message_ids,
                "location_count": queue.location_count,
                "queued_location_count": queue.queued_location_count,
                "queued": !queue.items.is_empty(),
                "push_ids": queue.items.iter().map(|item| item.push_id.clone()).collect::<Vec<_>>(),
                "push_id": queue.items.first().map(|item| item.push_id.clone())
            });
        } else {
            self.refresh_notification(&notification_uid)?;
        }
        self.append_audit_event(
            "notification_created",
            vec![audit_target("notification", &notification_uid)],
            reason.map(str::trim).filter(|value| !value.is_empty()),
            json!({
                "notification_uid": notification_uid,
                "notification_name": name,
                "message_id": message_id,
                "summary": summary,
                "path": rel_path(&self.root, &notification_dir),
            }),
        )?;
        guard.commit();
        Ok(result)
    }

    pub fn archive_list(&self) -> Result<Value> {
        self.require_workspace()?;
        let cases = self.archive_case_items()?;
        Ok(json!({
            "code": "archive_list",
            "case_count": cases.len(),
            "case_path_templates": {
                "case_path": "archive/cases/{group}/{case_dir}",
                "view_path": "archive/cases/{group}/{case_dir}/case.md",
                "data_path": "archive/cases/{group}/{case_dir}/data/case.json",
            },
            "cases": cases,
        }))
    }

    pub fn archive_list_cases(&self) -> Result<Value> {
        self.require_workspace()?;
        let cases = self.archive_case_items()?;
        Ok(json!({
            "code": "archive_case_list",
            "count": cases.len(),
            "path_templates": {
                "case_path": "archive/cases/{group}/{case_dir}",
                "view_path": "archive/cases/{group}/{case_dir}/case.md",
                "data_path": "archive/cases/{group}/{case_dir}/data/case.json",
            },
            "items": cases,
        }))
    }

    pub fn notification_list(&self) -> Result<Value> {
        self.require_workspace()?;
        let messages = self.notification_items()?;
        Ok(json!({
            "code": "notification_list",
            "count": messages.len(),
            "path_templates": {
                "notification_path": "notifications/{notification_dir}",
                "view_path": "notifications/{notification_dir}/notification.md",
                "data_path": "notifications/{notification_dir}/data/notification.json",
            },
            "items": messages,
        }))
    }

    pub fn notification_show(&self, notification_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let (notification_uid, notification_dir) = self.resolve_notification(notification_ref)?;
        self.refresh_notification(&notification_uid)?;
        let data = self.read_notifications(&notification_uid)?;
        let notification_path = self.notification_index_path(&notification_uid);
        let text = read_to_string(&notification_path, "read notification view")?;
        Ok(json!({
            "code": "notification",
            "notification_uid": notification_uid,
            "notification_name": data.collection_name,
            "path": rel_path(&self.root, &notification_dir),
            "view_path": rel_path(&self.root, &notification_path),
            "notes_path": rel_path(&self.root, &notification_dir.join("notes.md")),
            "message_count": data.items.len(),
            "items": data.items,
            "text": text,
        }))
    }

    pub fn notification_restore(
        &self,
        notification_ref: &str,
        message_id: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        self.restore_direct_notification(
            &notification_uid,
            message_id,
            reason,
            "message_restored",
            "message_restored",
        )
    }

    pub(super) fn restore_direct_notification(
        &self,
        notification_uid: &str,
        message_id: &str,
        reason: Option<&str>,
        event_kind: &str,
        result_code: &str,
    ) -> Result<Value> {
        validate_notification_uid(notification_uid)?;
        validate_id("message_id", message_id)?;
        let mut message = self.read_message_by_id(message_id)?;
        if message.workspace.notification_uid.as_deref() != Some(notification_uid) {
            return Err(AppError::new(
                "notification_entry_not_found",
                format!("message {message_id} is not in notification {notification_uid}"),
            ));
        }
        let removed_push = crate::push_queue::remove_pending_message_pushes(
            &self.root,
            message_id,
            "message.notification",
        )?;
        let push_ids = removed_push
            .iter()
            .map(|item| item.push_id.clone())
            .collect::<Vec<_>>();
        self.remove_notification_item(notification_uid, message_id)?;
        let view_path = self.notification_view_path(notification_uid, message_id);
        if view_path.exists() {
            remove_file(&view_path)?;
        }
        message.workspace.status = "triage".to_string();
        message.workspace.notification_uid = None;
        message.workspace.notification_rfc3339 = None;
        message.workspace.origin = None;
        message.workspace.remote_sync = None;
        self.write_message_cache(&message)?;
        self.refresh_message_after_ref_change(message_id)?;
        self.clear_message_pending_pushes(message_id, &push_ids, false)?;
        self.refresh_notification(notification_uid)?;
        self.append_audit_event(
            event_kind,
            vec![
                audit_target("message", message_id),
                audit_target("notification", notification_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "notification_uid": notification_uid,
                "from_path": format!("notifications/{notification_uid}/views/messages/{message_id}.md"),
                "to_path": format!("triage/{message_id}.md"),
                "removed_push_ids": push_ids.clone(),
            }),
        )?;
        Ok(json!({
            "code": result_code,
            "message_id": message_id,
            "notification_uid": notification_uid,
            "triage_path": format!("triage/{message_id}.md"),
            "removed_push_count": push_ids.len(),
            "push_ids": push_ids,
        }))
    }

    pub fn notification_move(
        &self,
        notification_ref: &str,
        message_id: &str,
        new_notification_ref: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let (new_notification_uid, _) = self.resolve_notification(new_notification_ref)?;
        if notification_uid == new_notification_uid {
            return Ok(json!({
                "code": "notification_message_moved",
                "message_id": message_id,
                "notification_uid": notification_uid,
                "new_notification_uid": new_notification_uid,
                "changed": false,
            }));
        }
        let mut old_data = self.read_notifications(&notification_uid)?;
        let pos = old_data
            .items
            .iter()
            .position(|item| item.message_id == message_id)
            .ok_or_else(|| {
                AppError::new(
                    "notification_entry_not_found",
                    format!("message {message_id} is not in notification {notification_uid}"),
                )
            })?;
        let item = old_data.items.remove(pos);
        old_data.updated_rfc3339 = Some(now_rfc3339());
        self.write_notifications(&notification_uid, &old_data)?;
        let old_view = self.notification_view_path(&notification_uid, message_id);
        if old_view.exists() {
            remove_file(&old_view)?;
        }
        let mut message = self.read_message_by_id(message_id)?;
        message.workspace.status = MessageStatus::Notification.as_str().to_string();
        message.workspace.notification_uid = Some(new_notification_uid.to_string());
        message.workspace.notification_rfc3339 = Some(item.added_rfc3339.clone());
        message.workspace.remote_sync = None;
        self.write_message_cache(&message)?;
        self.upsert_notification_item(
            &new_notification_uid,
            message_id,
            item.summary.as_deref(),
            &item.added_rfc3339,
        )?;
        self.refresh_notification(&notification_uid)?;
        self.refresh_notification(&new_notification_uid)?;
        self.append_audit_event(
            "notification_message_moved",
            vec![
                audit_target("message", message_id),
                audit_target("notification", &notification_uid),
                audit_target("notification", &new_notification_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "from_notification_uid": notification_uid,
                "notification_uid": new_notification_uid,
            }),
        )?;
        Ok(json!({
            "code": "notification_message_moved",
            "message_id": message_id,
            "from_notification_uid": notification_uid,
            "notification_uid": new_notification_uid,
            "changed": true,
        }))
    }

    pub fn notification_rename(
        &self,
        notification_ref: &str,
        name: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_name("notification_name", name)?;
        let (notification_uid, from) = self.resolve_notification(notification_ref)?;
        let mut data = self.read_notifications(&notification_uid)?;
        let old_name = data.collection_name.clone();
        let to = self.notification_dir_for_name(&notification_uid, name);
        let changed_path = to != from;
        if changed_path && to.exists() {
            return Err(AppError::new(
                "notification_exists",
                format!(
                    "notification category already exists: {}",
                    path_to_string(&to)
                ),
            ));
        }
        if let Some(parent) = to.parent() {
            create_dir_all(parent)?;
        }
        if changed_path {
            fs::rename(&from, &to).map_err(|e| AppError::io("rename notification category", &e))?;
        }
        data.collection_name = name.to_string();
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_notifications(&notification_uid, &data)?;
        self.refresh_notification(&notification_uid)?;
        self.append_audit_event(
            "notification_renamed",
            vec![audit_target("notification", &notification_uid)],
            reason,
            json!({
                "notification_uid": notification_uid,
                "old_notification_name": old_name,
                "notification_name": name,
                "message_count": data.items.len(),
            }),
        )?;
        Ok(json!({
            "code": "notification_renamed",
            "notification_uid": notification_uid,
            "old_notification_name": old_name,
            "notification_name": name,
            "path": rel_path(&self.root, &to),
            "message_count": data.items.len(),
            "changed": old_name != name || changed_path,
        }))
    }

    pub fn notification_set_summary(
        &self,
        notification_ref: &str,
        message_id: &str,
        summary: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        self.update_notification_summary(&notification_uid, message_id, summary)?;
        self.refresh_notification(&notification_uid)?;
        self.append_audit_event(
            "notification_summary_set",
            vec![
                audit_target("message", message_id),
                audit_target("notification", &notification_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "notification_uid": notification_uid,
                "summary": summary,
            }),
        )?;
        Ok(json!({
            "code": "notification_summary_set",
            "message_id": message_id,
            "notification_uid": notification_uid,
            "summary": summary,
        }))
    }

    pub fn notification_notes_show(&self, notification_ref: &str) -> Result<Value> {
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let path = self.notification_notes_path(&notification_uid);
        self.notes_show(
            "notification_notes",
            vec![audit_target("notification", &notification_uid)],
            &path,
        )
    }

    pub fn notification_notes_append(&self, notification_ref: &str, text: &str) -> Result<Value> {
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let path = self.notification_notes_path(&notification_uid);
        self.notes_append(
            "notification_notes_appended",
            vec![audit_target("notification", &notification_uid)],
            &path,
            text,
        )
    }

    pub fn notification_notes_replace(&self, notification_ref: &str, text: &str) -> Result<Value> {
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let path = self.notification_notes_path(&notification_uid);
        self.notes_replace(
            "notification_notes_replaced",
            vec![audit_target("notification", &notification_uid)],
            &path,
            text,
        )
    }

    pub(super) fn set_direct_message_notification(
        &self,
        message_id: &str,
        notification_uid: &str,
    ) -> Result<String> {
        let now = now_rfc3339();
        let mut msg = self.read_message_by_id(message_id)?;
        if let Some(existing) = msg.workspace.notification_uid.as_deref() {
            if existing != notification_uid {
                return Err(AppError::new(
                    "message_already_notified",
                    format!(
                        "message {message_id} is already in notification {existing}; use notification {existing} move"
                    ),
                ));
            }
        }
        msg.workspace.status = MessageStatus::Notification.as_str().to_string();
        msg.workspace.notification_uid = Some(notification_uid.to_string());
        let notification_rfc3339 = msg
            .workspace
            .notification_rfc3339
            .clone()
            .unwrap_or_else(|| now.clone());
        msg.workspace.notification_rfc3339 = Some(notification_rfc3339.clone());
        msg.workspace.origin = None;
        msg.workspace.remote_sync = None;
        self.clear_message_from_all_dispositions(message_id)?;
        self.write_message_materialized_cache(&msg)?;
        self.remove_triage_view_for_message(message_id)?;
        Ok(notification_rfc3339)
    }

    pub(super) fn archive_case_path_for_name(
        &self,
        group: &str,
        case_uid: &str,
        name: &str,
    ) -> PathBuf {
        self.root
            .join("archive")
            .join("cases")
            .join(group)
            .join(case_dir_name(case_uid, name))
    }

    pub(super) fn notification_dir(&self, notification_uid: &str) -> PathBuf {
        self.find_notification_dir_by_uid(notification_uid)
            .ok()
            .flatten()
            .unwrap_or_else(|| self.root.join("notifications").join(notification_uid))
    }

    pub(super) fn notification_dir_for_name(&self, notification_uid: &str, name: &str) -> PathBuf {
        self.root
            .join("notifications")
            .join(notification_dir_name(notification_uid, name))
    }

    pub(super) fn notification_index_path(&self, notification_uid: &str) -> PathBuf {
        self.notification_dir(notification_uid)
            .join("notification.md")
    }

    pub(super) fn notification_notes_path(&self, notification_uid: &str) -> PathBuf {
        self.notification_dir(notification_uid).join("notes.md")
    }

    pub(super) fn notification_json_path(&self, notification_uid: &str) -> PathBuf {
        self.notification_dir(notification_uid)
            .join("data")
            .join("notification.json")
    }

    pub(super) fn notification_view_path(
        &self,
        notification_uid: &str,
        message_id: &str,
    ) -> PathBuf {
        self.notification_dir(notification_uid)
            .join("views")
            .join("messages")
            .join(format!("{message_id}.md"))
    }

    pub(super) fn read_notifications(
        &self,
        notification_uid: &str,
    ) -> Result<NotificationMessages> {
        validate_notification_uid(notification_uid)?;
        let path = self.notification_json_path(notification_uid);
        if !path.exists() {
            return Ok(NotificationMessages::new_notification(
                notification_uid,
                "",
                &now_rfc3339(),
            ));
        }
        let data = read_to_string(&path, "read notifications")?;
        let messages: NotificationMessages =
            serde_json::from_str(&data).map_err(|e| AppError::json("parse notifications", &e))?;
        if messages.schema_name != NOTIFICATION_SCHEMA_NAME
            || messages.schema_version != MESSAGE_COLLECTION_SCHEMA_VERSION
            || messages.collection_uid != notification_uid
        {
            return Err(AppError::new(
                "notifications_invalid",
                format!(
                    "invalid notifications schema: {}",
                    rel_path(&self.root, &path)
                ),
            ));
        }
        Ok(messages)
    }

    pub(super) fn write_notifications(
        &self,
        notification_uid: &str,
        data: &NotificationMessages,
    ) -> Result<()> {
        self.write_notifications_named(notification_uid, &data.collection_name, data)
    }

    pub(super) fn write_notifications_named(
        &self,
        notification_uid: &str,
        notification_name: &str,
        data: &NotificationMessages,
    ) -> Result<()> {
        let mut normalized = data.clone();
        normalized.normalize(
            NOTIFICATION_SCHEMA_NAME,
            notification_uid,
            notification_name,
        );
        write_json_pretty(&self.notification_json_path(notification_uid), &normalized)
    }

    pub(super) fn upsert_notification_item(
        &self,
        notification_uid: &str,
        message_id: &str,
        summary: Option<&str>,
        added_rfc3339: &str,
    ) -> Result<()> {
        let mut data = self.read_notifications(notification_uid)?;
        let summary = summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        if let Some(item) = data
            .items
            .iter_mut()
            .find(|item| item.message_id == message_id)
        {
            if summary.is_some() {
                item.summary = summary;
            }
            item.added_rfc3339 = added_rfc3339.to_string();
        } else {
            data.items.push(NotificationMessageItem {
                message_id: message_id.to_string(),
                summary,
                added_rfc3339: added_rfc3339.to_string(),
            });
        }
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_notifications(notification_uid, &data)
    }

    pub(super) fn remove_notification_item(
        &self,
        notification_uid: &str,
        message_id: &str,
    ) -> Result<()> {
        let mut data = self.read_notifications(notification_uid)?;
        let before = data.items.len();
        data.items.retain(|item| item.message_id != message_id);
        if data.items.len() == before {
            return Err(AppError::new(
                "notification_entry_not_found",
                format!("message {message_id} is not in notification {notification_uid}"),
            ));
        }
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_notifications(notification_uid, &data)
    }

    pub(super) fn update_notification_summary(
        &self,
        notification_uid: &str,
        message_id: &str,
        summary: &str,
    ) -> Result<()> {
        let mut data = self.read_notifications(notification_uid)?;
        let item = data
            .items
            .iter_mut()
            .find(|item| item.message_id == message_id)
            .ok_or_else(|| {
                AppError::new(
                    "notification_entry_not_found",
                    format!("message {message_id} is not in notification {notification_uid}"),
                )
            })?;
        item.summary = Some(summary.trim().to_string()).filter(|value| !value.is_empty());
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_notifications(notification_uid, &data)
    }

    pub(super) fn refresh_notification_indexes(&self) -> Result<()> {
        create_dir_all(&self.root.join("archive/cases"))?;
        create_dir_all(&self.root.join("notifications"))?;
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        for notification_uid in self.notification_ids()? {
            self.refresh_notification_with_renderer(&notification_uid, &mut renderer, true)?;
        }
        Ok(())
    }

    pub(super) fn refresh_notification_indexes_cached(
        &self,
        cache: &mut RenderCache,
    ) -> Result<NotificationViewRefresh> {
        create_dir_all(&self.root.join("archive/cases"))?;
        create_dir_all(&self.root.join("notifications"))?;
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        let mut totals = NotificationViewRefresh::default();
        for notification_uid in self.notification_ids()? {
            let refresh = self.refresh_notification_with_cache(
                &notification_uid,
                &mut renderer,
                true,
                cache,
            )?;
            totals.notification_index_count += refresh.notification_index_count;
            totals.notification_message_count += refresh.notification_message_count;
            totals.notification_index_unchanged_count += refresh.notification_index_unchanged_count;
            totals.notification_message_unchanged_count +=
                refresh.notification_message_unchanged_count;
        }
        Ok(totals)
    }

    pub(super) fn refresh_notification(&self, notification_uid: &str) -> Result<()> {
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        self.refresh_notification_with_renderer(notification_uid, &mut renderer, true)?;
        Ok(())
    }

    pub(super) fn refresh_notification_with_renderer(
        &self,
        notification_uid: &str,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        sync_state: bool,
    ) -> Result<NotificationViewRefresh> {
        self.refresh_notification_inner(notification_uid, renderer, sync_state, None)
    }

    pub(super) fn refresh_notification_with_cache(
        &self,
        notification_uid: &str,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        sync_state: bool,
        cache: &mut RenderCache,
    ) -> Result<NotificationViewRefresh> {
        self.refresh_notification_inner(notification_uid, renderer, sync_state, Some(cache))
    }

    fn refresh_notification_inner(
        &self,
        notification_uid: &str,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        sync_state: bool,
        mut cache: Option<&mut RenderCache>,
    ) -> Result<NotificationViewRefresh> {
        validate_notification_uid(notification_uid)?;
        let notification_dir = self.notification_dir(notification_uid);
        let messages_dir = notification_dir.join("views").join("messages");
        create_dir_all(&messages_dir)?;
        let mut data = self.read_notifications(notification_uid)?;
        if sync_state {
            let before = data.items.len();
            data.items.retain(|item| {
                self.read_message_by_id(&item.message_id)
                    .map(|message| {
                        message.workspace.notification_uid.as_deref() == Some(notification_uid)
                    })
                    .unwrap_or(false)
            });
            if data.items.len() != before {
                data.updated_rfc3339 = Some(now_rfc3339());
                self.write_notifications(notification_uid, &data)?;
            }
        }
        let items = data
            .items
            .iter()
            .filter(|item| {
                self.read_message_by_id(&item.message_id)
                    .map(|message| {
                        message.workspace.notification_uid.as_deref() == Some(notification_uid)
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        let desired = items
            .iter()
            .map(|item| item.message_id.clone())
            .collect::<BTreeSet<_>>();
        let mut message_stats = RenderWriteStats::default();
        for item in &items {
            let message = self.read_message_by_id(&item.message_id)?;
            let view_path = self.notification_view_path(notification_uid, &item.message_id);
            let outcome = if let Some(cache) = cache.as_deref_mut() {
                let input_hash = self.render_input_hash_cached(
                    cache,
                    "notification",
                    &view_path,
                    &[
                        TemplateKey::NotificationMessage,
                        TemplateKey::MessageSection,
                    ],
                    json!({
                        "notification_uid": notification_uid,
                        "notification_name": data.collection_name.as_str(),
                        "item": item,
                        "message_hash": blake3_json_fingerprint(&message)?,
                    }),
                )?;
                self.render_cached_string(cache, &view_path, "notification", input_hash, || {
                    self.render_notification_view(
                        &message,
                        notification_uid,
                        &data.collection_name,
                        item,
                        renderer,
                        view_path.parent(),
                    )
                })?
            } else {
                let rendered = self.render_notification_view(
                    &message,
                    notification_uid,
                    &data.collection_name,
                    item,
                    renderer,
                    view_path.parent(),
                )?;
                write_string(&view_path, &rendered)?;
                FileWriteOutcome::Written
            };
            message_stats.record(outcome);
        }
        if messages_dir.exists() {
            for entry in read_dir(&messages_dir, "read notification views")? {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !desired.contains(stem) {
                    remove_file(&path)?;
                }
            }
        }
        let config = MailConfig::load(&self.root)?;
        let index_path = notification_dir.join("notification.md");
        let index_outcome = if let Some(cache) = cache {
            let input_hash = self.render_input_hash_cached(
                cache,
                "notification_index",
                &index_path,
                &[TemplateKey::NotificationIndex],
                json!({
                    "notification_uid": notification_uid,
                    "notification_name": data.collection_name.as_str(),
                    "items": items.clone(),
                    "message_hashes": items
                        .iter()
                        .map(|item| {
                            self.read_message_by_id(&item.message_id)
                                .and_then(|message| blake3_json_fingerprint(&message))
                        })
                        .collect::<Result<Vec<_>>>()?,
                }),
            )?;
            self.render_cached_string(cache, &index_path, "notification_index", input_hash, || {
                self.render_notification_index(
                    notification_uid,
                    &data.collection_name,
                    &items,
                    &config,
                    renderer,
                )
            })?
        } else {
            let rendered = self.render_notification_index(
                notification_uid,
                &data.collection_name,
                &items,
                &config,
                renderer,
            )?;
            write_string(&index_path, &rendered)?;
            FileWriteOutcome::Written
        };
        Ok(NotificationViewRefresh {
            notification_index_count: usize::from(index_outcome == FileWriteOutcome::Written),
            notification_message_count: message_stats.written_count,
            notification_index_unchanged_count: usize::from(
                index_outcome == FileWriteOutcome::Unchanged,
            ),
            notification_message_unchanged_count: message_stats.unchanged_count,
        })
    }

    pub(super) fn render_notification_view(
        &self,
        message: &MessageFile,
        notification_uid: &str,
        notification_name: &str,
        item: &NotificationMessageItem,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        output_dir: Option<&Path>,
    ) -> Result<String> {
        let config = MailConfig::load(&self.root)?;
        let title = message.subject.as_deref().unwrap_or("");
        let message_value = message_template_value(message)?;
        let item_value = serde_json::to_value(item)
            .map_err(|e| AppError::json("serialize notification item", &e))?;
        let conversation =
            self.message_conversation_with_renderer(message, &config, renderer, output_dir)?;
        let context = json!({
            "frontmatter": {
                "kind": "notification",
                "message_id": message.message_id.as_str(),
                "notification_uid": notification_uid,
                "notification_name": notification_name,
                "added_rfc3339": item.added_rfc3339.as_str(),
            },
            "notification": {
                "notification_uid": notification_uid,
                "notification_name": notification_name,
            },
            "message": message_value,
            "item": item_value,
            "view": {
                "language": config.resolved_language_bcp47(),
                "title": title,
                "summary": item.summary.as_deref().unwrap_or(""),
                "added_rfc3339": item.added_rfc3339.as_str(),
                "conversation": conversation.trim(),
            },
        });
        renderer.render(TemplateKey::NotificationMessage, &context)
    }

    pub(super) fn render_notification_index(
        &self,
        notification_uid: &str,
        notification_name: &str,
        data_items: &[NotificationMessageItem],
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<String> {
        let mut items = data_items.to_vec();
        items.sort_by(|a, b| {
            let a_time = self
                .read_message_by_id(&a.message_id)
                .ok()
                .and_then(|message| message_time(&message))
                .unwrap_or_else(|| a.added_rfc3339.clone());
            let b_time = self
                .read_message_by_id(&b.message_id)
                .ok()
                .and_then(|message| message_time(&message))
                .unwrap_or_else(|| b.added_rfc3339.clone());
            compare_rfc3339_asc(&b_time, &a_time).then_with(|| a.message_id.cmp(&b.message_id))
        });
        let mut rendered_items = Vec::new();
        let offset = config.resolved_timezone_offset();
        for item in &items {
            let message = self.read_message_by_id(&item.message_id)?;
            let fields = config
                .notification
                .message_index
                .item_fields
                .iter()
                .filter_map(|field| archive_index_field_value(*field, &message, item, &offset))
                .collect::<Vec<_>>();
            let has_message_id = config
                .notification
                .message_index
                .item_fields
                .contains(&NotificationIndexField::MessageId);
            let has_link = config
                .notification
                .message_index
                .item_fields
                .contains(&NotificationIndexField::Link);
            let mut second = Vec::new();
            if !has_message_id || !has_link {
                if !has_message_id {
                    second.push(json!({
                        "kind": "message_id",
                        "value": item.message_id.as_str(),
                    }));
                }
                if !has_link {
                    second.push(json!({
                        "kind": "link",
                        "href": format!("views/messages/{}.md", item.message_id),
                    }));
                }
            }
            let title = item
                .summary
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or(message.subject.as_deref())
                .unwrap_or(item.message_id.as_str())
                .to_string();
            let mut rendered_item = thread_item_common(
                &message,
                &offset,
                format!("views/messages/{}.md", item.message_id),
                title,
            )?;
            if let Value::Object(map) = &mut rendered_item {
                if let Some(Value::Object(view)) = map.get_mut("view") {
                    view.insert(
                        "summary".to_string(),
                        json!(item.summary.as_deref().unwrap_or("")),
                    );
                    view.insert(
                        "display_summary".to_string(),
                        json!(item
                            .summary
                            .as_deref()
                            .map(markdown_inline)
                            .unwrap_or_default()),
                    );
                    view.insert(
                        "added_rfc3339".to_string(),
                        json!(item.added_rfc3339.as_str()),
                    );
                    view.insert(
                        "added_time".to_string(),
                        time_context(&item.added_rfc3339, &offset),
                    );
                    view.insert("fields".to_string(), json!(fields));
                    view.insert("secondary".to_string(), json!(second));
                }
                map.insert(
                    "item".to_string(),
                    serde_json::to_value(item)
                        .map_err(|e| AppError::json("serialize notification item", &e))?,
                );
            }
            rendered_items.push(rendered_item);
        }
        let context = json!({
            "notification": {
                "notification_uid": notification_uid,
                "notification_name": notification_name,
            },
            "items": rendered_items,
            "view": {
                "language": config.resolved_language_bcp47(),
                "message_count": rendered_items.len(),
            },
            "config": {
                "notification": {
                    "message_index": {
                        "item_fields": config.notification.message_index.item_fields
                            .iter()
                            .map(|field| field.as_str())
                            .collect::<Vec<_>>(),
                    },
                },
            },
        });
        renderer.render(TemplateKey::NotificationIndex, &context)
    }

    pub(super) fn notification_ids(&self) -> Result<Vec<String>> {
        let dir = self.root.join("notifications");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in read_dir(&dir, "read notifications directory")? {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if path.join("data").join("notification.json").is_file() {
                        if let Some(uid) = notification_uid_from_dir_name(name) {
                            ids.push(uid);
                        }
                    }
                }
            }
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    pub(super) fn notification_items(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for notification_uid in self.notification_ids()? {
            let data = self.read_notifications(&notification_uid)?;
            let path = self.notification_dir(&notification_uid);
            out.push(json!({
                "notification_uid": notification_uid,
                "notification_name": data.collection_name,
                "notification_dir": path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
            }));
        }
        Ok(out)
    }

    pub(super) fn resolve_notification(&self, notification_ref: &str) -> Result<(String, PathBuf)> {
        let notification_uid = parse_notification_ref(notification_ref)?;
        self.find_notification_dir_by_uid(&notification_uid)?
            .map(|path| (notification_uid.clone(), path))
            .ok_or_else(|| {
                AppError::new(
                    "archive_not_found",
                    format!("notification category not found: {notification_uid}"),
                )
            })
    }

    pub(super) fn find_notification_dir_by_uid(
        &self,
        notification_uid: &str,
    ) -> Result<Option<PathBuf>> {
        validate_notification_uid(notification_uid)?;
        let dir = self.root.join("notifications");
        if !dir.exists() {
            return Ok(None);
        }
        let mut matches = Vec::new();
        for entry in read_dir(&dir, "read notifications directory")? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if notification_uid_from_dir_name(name).as_deref() == Some(notification_uid) {
                matches.push(path);
            }
        }
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(AppError::new(
                "duplicate_notification_uid",
                format!("duplicate archive uid found: {notification_uid}"),
            )),
        }
    }
}
