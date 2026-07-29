use super::*;

#[derive(Clone, Debug)]
struct NotificationRenderRow {
    item: NotificationMessageItem,
    message: MessageFile,
}

pub(super) fn validate_notification_collection(
    root: &Path,
    path: &Path,
    notification_uid: &str,
    expected_name: Option<&str>,
    messages: &NotificationMessages,
) -> Result<()> {
    let duplicate_message_id = messages.duplicate_message_id();
    let content_issues = message_collection_content_issues(messages);
    let valid = messages.schema_name == NOTIFICATION_SCHEMA_NAME
        && messages.schema_version == SUGGESTIBLE_COLLECTION_SCHEMA_VERSION
        && messages.collection_uid == notification_uid
        && validate_notification_uid(&messages.collection_uid).is_ok()
        && expected_name.is_none_or(|name| messages.collection_name == name)
        && messages.status.is_empty()
        && messages.archived_rfc3339.is_none()
        && content_issues.is_empty()
        && duplicate_message_id.is_none();
    if valid {
        return Ok(());
    }
    Err(AppError::new(
        "notifications_invalid",
        format!("invalid notification collection: {}", rel_path(root, path)),
    )
    .with_details(json!({
        "path": rel_path(root, path),
        "expected_schema_name": NOTIFICATION_SCHEMA_NAME,
        "schema_name": messages.schema_name,
        "expected_schema_version": SUGGESTIBLE_COLLECTION_SCHEMA_VERSION,
        "schema_version": messages.schema_version,
        "expected_collection_uid": notification_uid,
        "collection_uid": messages.collection_uid,
        "expected_collection_name": expected_name,
        "collection_name": messages.collection_name,
        "expected_status": "",
        "status": messages.status,
        "archived_rfc3339": messages.archived_rfc3339,
        "duplicate_message_id": duplicate_message_id,
        "content_issues": content_issues,
    })))
}

fn validate_notification_message(
    notification_uid: &str,
    row: &NotificationRenderRow,
) -> Result<()> {
    let message = &row.message;
    if message.workspace.status == MessageStatus::Notification.as_str()
        && message.workspace.notification_uid.as_deref() == Some(notification_uid)
        && message.workspace.notification_rfc3339.as_deref() == Some(&row.item.added_rfc3339)
        && message.workspace.origin.is_none()
    {
        return Ok(());
    }
    Err(AppError::new(
        "notification_state_invalid",
        format!(
            "message {} does not match canonical notification {} membership",
            message.message_id, notification_uid
        ),
    )
    .with_details(json!({
        "message_id": message.message_id,
        "notification_uid": notification_uid,
        "expected_notification_rfc3339": row.item.added_rfc3339,
        "status": message.workspace.status,
        "message_notification_uid": message.workspace.notification_uid,
        "message_notification_rfc3339": message.workspace.notification_rfc3339,
        "origin": message.workspace.origin,
    })))
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
                self.queue_archive_for_archived_messages(&[message_id.to_string()], None, None)?;
            result = json!({
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

    pub fn notification_list(&self) -> Result<Value> {
        self.require_workspace()?;
        let messages = self.notification_items()?;
        let pt_notification_path = [layout::NOTIFICATIONS, "/{notification_dir}"].concat();
        let pt_view_path = [layout::NOTIFICATIONS, "/{notification_dir}/notification.md"].concat();
        let pt_data_path = [
            layout::NOTIFICATIONS,
            "/{notification_dir}/data/notification.json",
        ]
        .concat();
        Ok(json!({
            "count": messages.len(),
            "path_templates": {
                "notification_path": pt_notification_path,
                "view_path": pt_view_path,
                "data_path": pt_data_path,
            },
            "items": messages,
        }))
    }

    pub fn notification_show(&self, notification_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let (notification_uid, notification_dir) = self.resolve_notification(notification_ref)?;
        let context = self.notification_render_document(&notification_uid)?;
        let notification_path = self.notification_index_path(&notification_uid);
        serde_json::to_value(CollectionShowDocument {
            schema_name: "afmail_collection_show_document".to_string(),
            schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
            context,
            locator: CollectionShowLocator {
                collection_path: rel_path(&self.root, &notification_dir),
                view_path: notification_path
                    .is_file()
                    .then(|| rel_path(&self.root, &notification_path)),
                notes_path: rel_path(&self.root, &notification_dir.join("notes.md")),
                messages_path: rel_path(
                    &self.root,
                    &notification_dir.join("views").join("messages"),
                ),
            },
            review_base_hash: self.notification_review_base_hash(notification_ref)?,
        })
        .map_err(|error| AppError::json("serialize notification show document", &error))
    }

    pub fn notification_remove(
        &self,
        notification_ref: &str,
        message_id: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        self.remove_direct_notification(
            &notification_uid,
            message_id,
            reason,
            "notification_message_removed",
            "notification_message_removed",
        )
    }

    pub(super) fn remove_direct_notification(
        &self,
        notification_uid: &str,
        message_id: &str,
        reason: Option<&str>,
        event_kind: &str,
        _result_code: &str,
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
                "from_path": format!("{}/{notification_uid}/views/messages/{message_id}.md", layout::NOTIFICATIONS),
                "to_path": format!("{}/{message_id}.md", layout::TRIAGE),
                "removed_push_ids": push_ids.clone(),
            }),
        )?;
        Ok(json!({
            "message_id": message_id,
            "notification_uid": notification_uid,
            "triage_path": format!("{}/{message_id}.md", layout::TRIAGE),
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
            "message_id": message_id,
            "notification_uid": notification_uid,
            "summary": summary,
        }))
    }

    pub fn notification_delete(
        &self,
        notification_ref: &str,
        allow_notes: bool,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (notification_uid, notification_dir) = self.resolve_notification(notification_ref)?;
        let data = self.read_notifications(&notification_uid)?;
        let message_ids = data
            .items
            .iter()
            .map(|item| item.message_id.clone())
            .collect::<Vec<_>>();
        if !message_ids.is_empty() {
            return Err(AppError::new(
                "notification_not_empty",
                format!(
                    "notification {notification_uid} still contains {} message(s)",
                    message_ids.len()
                ),
            )
            .with_hint(
                "Move or remove all messages from the notification before deleting the collection.",
            )
            .with_details(json!({
                "notification_uid": notification_uid,
                "message_count": message_ids.len(),
                "message_ids": message_ids,
                "suggested_commands": [
                    format!("afmail notification show {notification_ref}"),
                    format!("afmail notification remove {notification_ref} MESSAGE_ID --reason TEXT"),
                    format!("afmail notification move {notification_ref} MESSAGE_ID OTHER_NOTIFICATION_REF --reason TEXT")
                ]
            })));
        }

        let notes_path = notification_dir.join("notes.md");
        let notes_exists = notes_path.is_file();
        let notes_text = if notes_exists {
            read_to_string(&notes_path, "read notification notes")?
        } else {
            String::new()
        };
        let notes_present = !notes_text.trim().is_empty();
        if notes_present && !allow_notes {
            return Err(AppError::new(
                "notification_notes_present",
                format!("notification {notification_uid} has notes.md content"),
            )
            .with_hint(
                "Inspect the notes, then rerun with `--allow-notes` if deleting them is intended.",
            )
            .with_details(json!({
                "notification_uid": notification_uid,
                "notes_path": rel_path(&self.root, &notes_path),
                "notes_len_bytes": notes_text.len(),
                "notes_hash": stable_text_hash(&notes_text),
                "suggested_commands": [
                    format!("afmail notification notes show {notification_ref}"),
                    format!("afmail notification delete {notification_ref} --allow-notes --reason TEXT")
                ]
            })));
        }

        let path = rel_path(&self.root, &notification_dir);
        let transaction = self.begin_transaction("notification_delete", vec![path.clone()])?;
        remove_dir_all(&notification_dir)?;
        transaction.commit()?;
        self.append_audit_event(
            "notification_deleted",
            vec![audit_target("notification", &notification_uid)],
            reason,
            json!({
                "notification_uid": notification_uid.clone(),
                "notification_name": data.collection_name.clone(),
                "from_path": path.clone(),
                "notes_deleted": notes_exists,
                "notes_present": notes_present,
                "notes_len_bytes": notes_text.len(),
                "notes_hash": stable_text_hash(&notes_text),
            }),
        )?;
        Ok(json!({
            "notification_uid": notification_uid,
            "notification_name": data.collection_name,
            "from_path": path,
            "deleted": true,
            "notes_deleted": notes_exists,
            "notes_present": notes_present,
        }))
    }

    pub fn notification_notes_show(&self, notification_ref: &str) -> Result<Value> {
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let path = self.notification_notes_path(&notification_uid);
        self.notes_show(&path)
    }

    pub(crate) fn notification_notes_render_document(
        &self,
        notification_ref: &str,
    ) -> Result<NotesRenderDocument> {
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        self.notes_render_document(&self.notification_notes_path(&notification_uid))
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
        if let Some(existing) = msg.workspace.notification_uid.as_deref()
            && existing != notification_uid
        {
            return Err(AppError::new(
                "message_already_notified",
                format!(
                    "message {message_id} is already in notification {existing}; use notification {existing} move"
                ),
            ));
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

    pub(super) fn notification_dir(&self, notification_uid: &str) -> PathBuf {
        self.find_notification_dir_by_uid(notification_uid)
            .ok()
            .flatten()
            .unwrap_or_else(|| self.root.join(layout::NOTIFICATIONS).join(notification_uid))
    }

    pub(super) fn notification_dir_for_name(&self, notification_uid: &str, name: &str) -> PathBuf {
        self.root
            .join(layout::NOTIFICATIONS)
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
            return Err(AppError::new(
                "notifications_invalid",
                format!(
                    "notification collection is missing: {}",
                    rel_path(&self.root, &path)
                ),
            ));
        }
        let data = read_to_string(&path, "read notifications")?;
        let messages: NotificationMessages =
            serde_json::from_str(&data).map_err(|e| AppError::json("parse notifications", &e))?;
        validate_notification_collection(&self.root, &path, notification_uid, None, &messages)?;
        super::suggestions::validate_notification_suggestion_pair(
            messages.agent_suggestion.as_ref(),
            messages.human_suggestion.as_ref(),
        )
        .map_err(|error| {
            AppError::new(
                "notifications_invalid",
                format!(
                    "invalid suggestion metadata in {}: {error}",
                    rel_path(&self.root, &path)
                ),
            )
        })?;
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
        let path = self.notification_json_path(notification_uid);
        validate_notification_collection(
            &self.root,
            &path,
            notification_uid,
            Some(notification_name),
            data,
        )?;
        super::suggestions::validate_notification_suggestion_pair(
            data.agent_suggestion.as_ref(),
            data.human_suggestion.as_ref(),
        )?;
        write_json_pretty(&path, data)
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
        create_dir_all(&self.root.join(layout::ARCHIVED_CASES))?;
        create_dir_all(&self.root.join(layout::NOTIFICATIONS))?;
        let language = self.template_language()?;
        let mut renderer = TemplateRenderer::new(&self.root, language);
        for notification_uid in self.notification_ids()? {
            self.refresh_notification_with_renderer(&notification_uid, &mut renderer)?;
        }
        Ok(())
    }

    pub(super) fn refresh_notification_indexes_cached(
        &self,
        cache: &mut RenderCache,
        render_context: &RenderContext,
        mut progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<NotificationViewRefresh> {
        create_dir_all(&self.root.join(layout::ARCHIVED_CASES))?;
        create_dir_all(&self.root.join(layout::NOTIFICATIONS))?;
        let language = render_context.config().template_language();
        let mut renderer = TemplateRenderer::new(&self.root, language);
        let mut totals = NotificationViewRefresh::default();
        let notification_ids = self.notification_ids()?;
        let total_count = notification_ids.len();
        for (index, notification_uid) in notification_ids.into_iter().enumerate() {
            let refresh = self.refresh_notification_with_cache(
                &notification_uid,
                &mut renderer,
                cache,
                render_context,
            )?;
            totals.notification_index_count += refresh.notification_index_count;
            totals.notification_message_count += refresh.notification_message_count;
            totals.notification_index_unchanged_count += refresh.notification_index_unchanged_count;
            totals.notification_message_unchanged_count +=
                refresh.notification_message_unchanged_count;
            let processed_count = index + 1;
            if processed_count == total_count || processed_count.is_multiple_of(25) {
                crate::progress::emit(
                    &mut progress,
                    "pull_render_notifications_progress",
                    json!({
                        "render_stage": "notifications",
                        "processed_count": processed_count,
                        "total_count": total_count,
                        "notification_count": processed_count,
                        "notification_view_written_count": totals.notification_index_count + totals.notification_message_count,
                        "notification_view_unchanged_count": totals.notification_index_unchanged_count + totals.notification_message_unchanged_count,
                    }),
                );
            }
        }
        Ok(totals)
    }

    pub(super) fn refresh_notification(&self, notification_uid: &str) -> Result<()> {
        let language = self.template_language()?;
        let mut renderer = TemplateRenderer::new(&self.root, language);
        self.refresh_notification_with_renderer(notification_uid, &mut renderer)?;
        Ok(())
    }

    pub(super) fn refresh_notification_with_renderer(
        &self,
        notification_uid: &str,
        renderer: &mut TemplateRenderer<'_>,
    ) -> Result<NotificationViewRefresh> {
        self.refresh_notification_inner(notification_uid, renderer, None, None)
    }

    pub(super) fn refresh_notification_with_cache(
        &self,
        notification_uid: &str,
        renderer: &mut TemplateRenderer<'_>,
        cache: &mut RenderCache,
        render_context: &RenderContext,
    ) -> Result<NotificationViewRefresh> {
        self.refresh_notification_inner(
            notification_uid,
            renderer,
            Some(cache),
            Some(render_context),
        )
    }

    fn refresh_notification_inner(
        &self,
        notification_uid: &str,
        renderer: &mut TemplateRenderer<'_>,
        mut cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<NotificationViewRefresh> {
        validate_notification_uid(notification_uid)?;
        let notification_dir = self.notification_dir(notification_uid);
        let messages_dir = notification_dir.join("views").join("messages");
        create_dir_all(&messages_dir)?;
        let owned_config = if render_context.is_none() {
            Some(MailConfig::load(&self.root)?)
        } else {
            None
        };
        let config = render_context
            .map(RenderContext::config)
            .or(owned_config.as_ref())
            .ok_or_else(|| AppError::new("internal", "render config was not initialized"))?;
        let data = self.read_notifications(notification_uid)?;
        let rows = self.notification_render_rows_with_context(&data.items, render_context)?;
        for row in &rows {
            validate_notification_message(notification_uid, row)?;
        }
        let desired = rows
            .iter()
            .map(|row| row.item.message_id.clone())
            .collect::<BTreeSet<_>>();
        let mut message_stats = RenderWriteStats::default();
        for row in &rows {
            let item = &row.item;
            let message = &row.message;
            let view_path = self.notification_view_path(notification_uid, &item.message_id);
            let document =
                self.notification_message_render_document(message, &data, config, render_context)?;
            let outcome = if let Some(cache) = cache.as_deref_mut() {
                let input_hash =
                    self.render_input_hash_cached(cache, "notification", &view_path, &document)?;
                self.render_cached_string(cache, &view_path, "notification", input_hash, || {
                    renderer.render_with_options(
                        TemplateKey::NotificationMessage,
                        &document,
                        TemplateRenderOptions {
                            timezone_offset: Some(config.resolved_timezone_offset()),
                            output_dir: view_path.parent(),
                        },
                    )
                })?
            } else {
                let rendered = renderer.render_with_options(
                    TemplateKey::NotificationMessage,
                    &document,
                    TemplateRenderOptions {
                        timezone_offset: Some(config.resolved_timezone_offset()),
                        output_dir: view_path.parent(),
                    },
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
        let index_path = notification_dir.join("notification.md");
        let document =
            self.notification_index_render_document(&data, &rows, config, render_context)?;
        let index_outcome = if let Some(cache) = cache {
            let input_hash =
                self.render_input_hash_cached(cache, "notification_index", &index_path, &document)?;
            self.render_cached_string(cache, &index_path, "notification_index", input_hash, || {
                renderer.render_with_options(
                    TemplateKey::NotificationIndex,
                    &document,
                    TemplateRenderOptions {
                        timezone_offset: Some(config.resolved_timezone_offset()),
                        output_dir: index_path.parent(),
                    },
                )
            })?
        } else {
            let rendered = renderer.render_with_options(
                TemplateKey::NotificationIndex,
                &document,
                TemplateRenderOptions {
                    timezone_offset: Some(config.resolved_timezone_offset()),
                    output_dir: index_path.parent(),
                },
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

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_notification_view(
        &self,
        message: &MessageFile,
        notification: &NotificationMessages,
        renderer: &mut TemplateRenderer<'_>,
        output_dir: Option<&Path>,
        render_context: Option<&RenderContext>,
    ) -> Result<String> {
        let owned_config = if render_context.is_none() {
            Some(MailConfig::load(&self.root)?)
        } else {
            None
        };
        let config = render_context
            .map(RenderContext::config)
            .or(owned_config.as_ref())
            .ok_or_else(|| AppError::new("internal", "render config was not initialized"))?;
        let document = self.notification_message_render_document(
            message,
            notification,
            config,
            render_context,
        )?;
        renderer.render_with_options(
            TemplateKey::NotificationMessage,
            &document,
            TemplateRenderOptions {
                timezone_offset: Some(config.resolved_timezone_offset()),
                output_dir,
            },
        )
    }

    fn notification_message_render_document(
        &self,
        message: &MessageFile,
        notification: &NotificationMessages,
        config: &MailConfig,
        render_context: Option<&RenderContext>,
    ) -> Result<CollectionMessageRenderDocument> {
        let message = self.message_render_document_with_context(message, config, render_context)?;
        Ok(collection_message_render_document(
            notification.clone(),
            message,
            None,
        ))
    }

    #[cfg(test)]
    pub(super) fn render_notification_index(
        &self,
        notification_uid: &str,
        notification_name: &str,
        data_items: &[NotificationMessageItem],
        config: &MailConfig,
        renderer: &mut TemplateRenderer<'_>,
    ) -> Result<String> {
        let rows = self.notification_render_rows_with_context(data_items, None)?;
        let mut notification = NotificationMessages::new_notification(
            notification_uid,
            notification_name,
            &now_rfc3339(),
        );
        notification.items = data_items.to_vec();
        self.render_notification_index_rows(&notification, &rows, config, renderer, None)
    }

    fn notification_render_rows_with_context(
        &self,
        data_items: &[NotificationMessageItem],
        render_context: Option<&RenderContext>,
    ) -> Result<Vec<NotificationRenderRow>> {
        data_items
            .iter()
            .map(|item| {
                Ok(NotificationRenderRow {
                    item: item.clone(),
                    message: match render_context {
                        Some(context) => context.read_message(self, &item.message_id)?,
                        None => self.read_message_by_id(&item.message_id)?,
                    },
                })
            })
            .collect()
    }

    #[cfg(test)]
    fn render_notification_index_rows(
        &self,
        notification: &NotificationMessages,
        rows: &[NotificationRenderRow],
        config: &MailConfig,
        renderer: &mut TemplateRenderer<'_>,
        render_context: Option<&RenderContext>,
    ) -> Result<String> {
        let document =
            self.notification_index_render_document(notification, rows, config, render_context)?;
        let output_dir = self.notification_dir(&notification.collection_uid);
        renderer.render_with_options(
            TemplateKey::NotificationIndex,
            &document,
            TemplateRenderOptions {
                timezone_offset: Some(config.resolved_timezone_offset()),
                output_dir: Some(&output_dir),
            },
        )
    }

    fn notification_index_render_document(
        &self,
        notification: &NotificationMessages,
        rows: &[NotificationRenderRow],
        config: &MailConfig,
        render_context: Option<&RenderContext>,
    ) -> Result<CollectionRenderDocument> {
        let messages = rows
            .iter()
            .map(|row| row.message.clone())
            .collect::<Vec<_>>();
        let messages =
            self.message_render_documents_with_context(&messages, config, render_context)?;
        Ok(collection_render_document(
            notification.clone(),
            messages,
            None,
        ))
    }

    pub(super) fn notification_ids(&self) -> Result<Vec<String>> {
        let dir = self.root.join(layout::NOTIFICATIONS);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in read_dir(&dir, "read notifications directory")? {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|s| s.to_str())
                && path.join("data").join("notification.json").is_file()
                && let Some(uid) = notification_uid_from_dir_name(name)
            {
                ids.push(uid);
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
                    "notification_not_found",
                    format!("notification category not found: {notification_uid}"),
                )
            })
    }

    pub(super) fn find_notification_dir_by_uid(
        &self,
        notification_uid: &str,
    ) -> Result<Option<PathBuf>> {
        validate_notification_uid(notification_uid)?;
        let dir = self.root.join(layout::NOTIFICATIONS);
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
                format!("duplicate notification uid found: {notification_uid}"),
            )),
        }
    }
}
