use super::*;

pub(super) fn archive_index_field_value(
    field: ArchiveMessageIndexField,
    message: &MessageFile,
    item: &ArchiveMessageItem,
    offset: &FixedOffset,
) -> Option<Value> {
    let value = match field {
        ArchiveMessageIndexField::Time => {
            message_time_datetime(message, offset).unwrap_or_default()
        }
        ArchiveMessageIndexField::From => message.from.clone().unwrap_or_default(),
        ArchiveMessageIndexField::To => message.to.join(", "),
        ArchiveMessageIndexField::Subject => message.subject.clone().unwrap_or_default(),
        ArchiveMessageIndexField::Summary => item.summary.clone().unwrap_or_default(),
        ArchiveMessageIndexField::MessageId => item.message_id.clone(),
        ArchiveMessageIndexField::ArchiveTime => item.archived_rfc3339.clone(),
        ArchiveMessageIndexField::Link => String::new(),
    };
    let keep_empty = matches!(
        field,
        ArchiveMessageIndexField::Time
            | ArchiveMessageIndexField::From
            | ArchiveMessageIndexField::Link
            | ArchiveMessageIndexField::MessageId
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
    pub fn create_archive_message_category(
        &self,
        name: &str,
        message_id: Option<&str>,
        summary: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_name("archive_name", name)?;
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
        let date = if let Some(message_id) = message_id {
            self.message_date(message_id)?
        } else {
            workspace_local_date(&self.workspace_date_offset()?)
        };
        let archive_uid = self.next_archive_uid(&date)?;
        let archive_dir = self.archive_message_dir_for_name(&archive_uid, name);
        if archive_dir.exists() {
            return Err(AppError::new(
                "archive_exists",
                format!(
                    "archive message category already exists: {}",
                    path_to_string(&archive_dir)
                ),
            ));
        }
        create_dir_all(&archive_dir.join("data"))?;
        create_dir_all(&archive_dir.join("views/messages"))?;
        write_string_new(
            &archive_dir.join("notes.md"),
            &new_notes_md(&self.root, self.template_language()?)?,
        )?;
        self.write_archive_messages_named(
            &archive_uid,
            name,
            &ArchiveMessages::new(&archive_uid, name),
        )?;

        let mut result = json!({
            "code": "archive_message_created",
            "archive_uid": archive_uid,
            "archive_name": name,
            "message_count": 0,
            "path": rel_path(&self.root, &archive_dir),
        });
        if let Some(message_id) = message_id {
            self.ensure_no_related_conversation(message_id)?;
            let archived_rfc3339 = self.set_direct_message_archive(message_id, &archive_uid)?;
            self.upsert_archive_message_item(&archive_uid, message_id, summary, &archived_rfc3339)?;
            self.refresh_archive_message_category(&archive_uid)?;
            let queue =
                self.queue_archive_for_archived_messages(&[message_id.to_string()], None)?;
            result = json!({
                "code": "archive_message_created",
                "archive_uid": archive_uid,
                "archive_name": name,
                "message_id": message_id,
                "summary": summary,
                "message_count": 1,
                "path": rel_path(&self.root, &archive_dir),
                "eligible_message_ids": queue.eligible_message_ids,
                "location_count": queue.location_count,
                "queued_location_count": queue.queued_location_count,
                "queued": !queue.items.is_empty(),
                "push_ids": queue.items.iter().map(|item| item.push_id.clone()).collect::<Vec<_>>(),
                "push_id": queue.items.first().map(|item| item.push_id.clone())
            });
        } else {
            self.refresh_archive_message_category(&archive_uid)?;
        }
        self.append_audit_event(
            "archive_message_created",
            vec![audit_target("archive", &archive_uid)],
            reason.map(str::trim).filter(|value| !value.is_empty()),
            json!({
                "archive_uid": archive_uid,
                "archive_name": name,
                "message_id": message_id,
                "summary": summary,
                "path": rel_path(&self.root, &archive_dir),
            }),
        )?;
        Ok(result)
    }

    pub fn archive_list(&self) -> Result<Value> {
        self.require_workspace()?;
        let cases = self.archive_case_items()?;
        let messages = self.archive_message_category_items()?;
        Ok(json!({
            "code": "archive_list",
            "case_count": cases.len(),
            "message_count": messages.len(),
            "case_path_templates": {
                "case_path": "archive/cases/{case_dir}",
                "view_path": "archive/cases/{case_dir}/case.md",
                "data_path": "archive/cases/{case_dir}/data/case.json",
            },
            "message_path_templates": {
                "archive_path": "archive/notifications/{archive_dir}",
                "view_path": "archive/notifications/{archive_dir}/archive.md",
                "data_path": "archive/notifications/{archive_dir}/data/archive.json",
            },
            "cases": cases,
            "messages": messages,
        }))
    }

    pub fn archive_list_cases(&self) -> Result<Value> {
        self.require_workspace()?;
        let cases = self.archive_case_items()?;
        Ok(json!({
            "code": "archive_case_list",
            "count": cases.len(),
            "path_templates": {
                "case_path": "archive/cases/{case_dir}",
                "view_path": "archive/cases/{case_dir}/case.md",
                "data_path": "archive/cases/{case_dir}/data/case.json",
            },
            "items": cases,
        }))
    }

    pub fn archive_list_messages(&self) -> Result<Value> {
        self.require_workspace()?;
        let messages = self.archive_message_category_items()?;
        Ok(json!({
            "code": "archive_message_list",
            "count": messages.len(),
            "path_templates": {
                "archive_path": "archive/notifications/{archive_dir}",
                "view_path": "archive/notifications/{archive_dir}/archive.md",
                "data_path": "archive/notifications/{archive_dir}/data/archive.json",
            },
            "items": messages,
        }))
    }

    pub fn archive_message_show(&self, archive_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let (archive_uid, archive_dir) = self.resolve_archive_message_category(archive_ref)?;
        self.refresh_archive_message_category(&archive_uid)?;
        let data = self.read_archive_messages(&archive_uid)?;
        let archive_path = self.archive_message_index_path(&archive_uid);
        let text = read_to_string(&archive_path, "read archive message view")?;
        Ok(json!({
            "code": "archive_message",
            "archive_uid": archive_uid,
            "archive_name": data.archive_name,
            "path": rel_path(&self.root, &archive_dir),
            "view_path": rel_path(&self.root, &archive_path),
            "notes_path": rel_path(&self.root, &archive_dir.join("notes.md")),
            "message_count": data.items.len(),
            "items": data.items,
            "text": text,
        }))
    }

    pub fn archive_message_restore(
        &self,
        archive_ref: &str,
        message_id: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (archive_uid, _) = self.resolve_archive_message_category(archive_ref)?;
        self.restore_direct_archive_message(
            &archive_uid,
            message_id,
            reason,
            "message_restored",
            "message_restored",
        )
    }

    pub(super) fn restore_direct_archive_message(
        &self,
        archive_uid: &str,
        message_id: &str,
        reason: Option<&str>,
        event_kind: &str,
        result_code: &str,
    ) -> Result<Value> {
        validate_archive_uid(archive_uid)?;
        validate_id("message_id", message_id)?;
        let mut message = self.read_message_by_id(message_id)?;
        if message.workspace.archive_uid.as_deref() != Some(archive_uid) {
            return Err(AppError::new(
                "archive_entry_not_found",
                format!("message {message_id} is not in archive {archive_uid}"),
            ));
        }
        let removed_push = crate::push_queue::remove_pending_message_pushes(
            &self.root,
            message_id,
            "message.archive",
        )?;
        let push_ids = removed_push
            .iter()
            .map(|item| item.push_id.clone())
            .collect::<Vec<_>>();
        self.remove_archive_message_item(archive_uid, message_id)?;
        let view_path = self.archive_message_view_path(archive_uid, message_id);
        if view_path.exists() {
            remove_file(&view_path)?;
        }
        message.workspace.status = "triage".to_string();
        message.workspace.archive_uid = None;
        message.workspace.archived_rfc3339 = None;
        message.workspace.origin = None;
        message.workspace.remote_sync = None;
        self.write_message_cache(&message)?;
        self.refresh_message_after_ref_change(message_id)?;
        self.clear_message_pending_pushes(message_id, &push_ids, false)?;
        self.refresh_archive_message_category(archive_uid)?;
        self.append_audit_event(
            event_kind,
            vec![
                audit_target("message", message_id),
                audit_target("archive", archive_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "archive_uid": archive_uid,
                "from_path": format!("archive/notifications/{archive_uid}/views/messages/{message_id}.md"),
                "to_path": format!("triage/{message_id}.md"),
                "removed_push_ids": push_ids.clone(),
            }),
        )?;
        Ok(json!({
            "code": result_code,
            "message_id": message_id,
            "archive_uid": archive_uid,
            "triage_path": format!("triage/{message_id}.md"),
            "removed_push_count": push_ids.len(),
            "push_ids": push_ids,
        }))
    }

    pub fn archive_message_move(
        &self,
        archive_ref: &str,
        message_id: &str,
        new_archive_ref: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let (archive_uid, _) = self.resolve_archive_message_category(archive_ref)?;
        let (new_archive_uid, _) = self.resolve_archive_message_category(new_archive_ref)?;
        if archive_uid == new_archive_uid {
            return Ok(json!({
                "code": "message_archive_moved",
                "message_id": message_id,
                "archive_uid": archive_uid,
                "new_archive_uid": new_archive_uid,
                "changed": false,
            }));
        }
        let mut old_data = self.read_archive_messages(&archive_uid)?;
        let pos = old_data
            .items
            .iter()
            .position(|item| item.message_id == message_id)
            .ok_or_else(|| {
                AppError::new(
                    "archive_entry_not_found",
                    format!("message {message_id} is not in archive {archive_uid}"),
                )
            })?;
        let item = old_data.items.remove(pos);
        self.write_archive_messages(&archive_uid, &old_data)?;
        let old_view = self.archive_message_view_path(&archive_uid, message_id);
        if old_view.exists() {
            remove_file(&old_view)?;
        }
        let mut message = self.read_message_by_id(message_id)?;
        message.workspace.status = "archived".to_string();
        message.workspace.archive_uid = Some(new_archive_uid.to_string());
        message.workspace.archived_rfc3339 = Some(item.archived_rfc3339.clone());
        message.workspace.remote_sync = None;
        self.write_message_cache(&message)?;
        self.upsert_archive_message_item(
            &new_archive_uid,
            message_id,
            item.summary.as_deref(),
            &item.archived_rfc3339,
        )?;
        self.refresh_archive_message_category(&archive_uid)?;
        self.refresh_archive_message_category(&new_archive_uid)?;
        self.append_audit_event(
            "message_archive_moved",
            vec![
                audit_target("message", message_id),
                audit_target("archive", &archive_uid),
                audit_target("archive", &new_archive_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "from_archive_uid": archive_uid,
                "archive_uid": new_archive_uid,
            }),
        )?;
        Ok(json!({
            "code": "message_archive_moved",
            "message_id": message_id,
            "from_archive_uid": archive_uid,
            "archive_uid": new_archive_uid,
            "changed": true,
        }))
    }

    pub fn archive_message_rename(
        &self,
        archive_ref: &str,
        name: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_name("archive_name", name)?;
        let (archive_uid, from) = self.resolve_archive_message_category(archive_ref)?;
        let mut data = self.read_archive_messages(&archive_uid)?;
        let old_name = data.archive_name.clone();
        let to = self.archive_message_dir_for_name(&archive_uid, name);
        let changed_path = to != from;
        if changed_path && to.exists() {
            return Err(AppError::new(
                "archive_exists",
                format!(
                    "archive message category already exists: {}",
                    path_to_string(&to)
                ),
            ));
        }
        if let Some(parent) = to.parent() {
            create_dir_all(parent)?;
        }
        if changed_path {
            fs::rename(&from, &to)
                .map_err(|e| AppError::io("rename archive message category", &e))?;
        }
        data.archive_name = name.to_string();
        self.write_archive_messages(&archive_uid, &data)?;
        self.refresh_archive_message_category(&archive_uid)?;
        self.append_audit_event(
            "message_archive_category_renamed",
            vec![audit_target("archive", &archive_uid)],
            reason,
            json!({
                "archive_uid": archive_uid,
                "old_archive_name": old_name,
                "archive_name": name,
                "message_count": data.items.len(),
            }),
        )?;
        Ok(json!({
            "code": "message_archive_category_renamed",
            "archive_uid": archive_uid,
            "old_archive_name": old_name,
            "archive_name": name,
            "path": rel_path(&self.root, &to),
            "message_count": data.items.len(),
            "changed": old_name != name || changed_path,
        }))
    }

    pub fn archive_message_set_summary(
        &self,
        archive_ref: &str,
        message_id: &str,
        summary: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let (archive_uid, _) = self.resolve_archive_message_category(archive_ref)?;
        self.update_archive_message_summary(&archive_uid, message_id, summary)?;
        self.refresh_archive_message_category(&archive_uid)?;
        self.append_audit_event(
            "message_archive_summary_set",
            vec![
                audit_target("message", message_id),
                audit_target("archive", &archive_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "archive_uid": archive_uid,
                "summary": summary,
            }),
        )?;
        Ok(json!({
            "code": "message_archive_summary_set",
            "message_id": message_id,
            "archive_uid": archive_uid,
            "summary": summary,
        }))
    }

    pub fn archive_message_notes_show(&self, archive_ref: &str) -> Result<Value> {
        let (archive_uid, _) = self.resolve_archive_message_category(archive_ref)?;
        let path = self.archive_message_notes_path(&archive_uid);
        self.notes_show(
            "archive_message_notes",
            vec![audit_target("archive", &archive_uid)],
            &path,
        )
    }

    pub fn archive_message_notes_append(&self, archive_ref: &str, text: &str) -> Result<Value> {
        let (archive_uid, _) = self.resolve_archive_message_category(archive_ref)?;
        let path = self.archive_message_notes_path(&archive_uid);
        self.notes_append(
            "archive_message_notes_appended",
            vec![audit_target("archive", &archive_uid)],
            &path,
            text,
        )
    }

    pub fn archive_message_notes_replace(&self, archive_ref: &str, text: &str) -> Result<Value> {
        let (archive_uid, _) = self.resolve_archive_message_category(archive_ref)?;
        let path = self.archive_message_notes_path(&archive_uid);
        self.notes_replace(
            "archive_message_notes_replaced",
            vec![audit_target("archive", &archive_uid)],
            &path,
            text,
        )
    }

    pub(super) fn set_direct_message_archive(
        &self,
        message_id: &str,
        archive_uid: &str,
    ) -> Result<String> {
        let now = now_rfc3339();
        let mut msg = self.read_message_by_id(message_id)?;
        if let Some(existing) = msg.workspace.archive_uid.as_deref() {
            if existing != archive_uid {
                return Err(AppError::new(
                    "message_already_archived",
                    format!(
                        "message {message_id} is already archived in {existing}; use archive message {existing} move"
                    ),
                ));
            }
        }
        msg.workspace.status = "archived".to_string();
        msg.workspace.archive_uid = Some(archive_uid.to_string());
        let archived_rfc3339 = msg
            .workspace
            .archived_rfc3339
            .clone()
            .unwrap_or_else(|| now.clone());
        msg.workspace.archived_rfc3339 = Some(archived_rfc3339.clone());
        msg.workspace.origin = None;
        msg.workspace.remote_sync = None;
        self.write_message_cache(&msg)?;
        self.remove_triage_view_for_message(message_id)?;
        Ok(archived_rfc3339)
    }

    pub(super) fn archive_case_path_for_name(&self, case_uid: &str, name: &str) -> PathBuf {
        self.root
            .join("archive")
            .join("cases")
            .join(case_dir_name(case_uid, name))
    }

    pub(super) fn archive_message_dir(&self, archive_uid: &str) -> PathBuf {
        self.find_archive_message_dir_by_uid(archive_uid)
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                self.root
                    .join("archive")
                    .join("notifications")
                    .join(archive_uid)
            })
    }

    pub(super) fn archive_message_dir_for_name(&self, archive_uid: &str, name: &str) -> PathBuf {
        self.root
            .join("archive")
            .join("notifications")
            .join(archive_dir_name(archive_uid, name))
    }

    pub(super) fn archive_message_index_path(&self, archive_uid: &str) -> PathBuf {
        self.archive_message_dir(archive_uid).join("archive.md")
    }

    pub(super) fn archive_message_notes_path(&self, archive_uid: &str) -> PathBuf {
        self.archive_message_dir(archive_uid).join("notes.md")
    }

    pub(super) fn archive_message_json_path(&self, archive_uid: &str) -> PathBuf {
        self.archive_message_dir(archive_uid)
            .join("data")
            .join("archive.json")
    }

    pub(super) fn archive_message_view_path(&self, archive_uid: &str, message_id: &str) -> PathBuf {
        self.archive_message_dir(archive_uid)
            .join("views")
            .join("messages")
            .join(format!("{message_id}.md"))
    }

    pub(super) fn read_archive_messages(&self, archive_uid: &str) -> Result<ArchiveMessages> {
        validate_archive_uid(archive_uid)?;
        let path = self.archive_message_json_path(archive_uid);
        if !path.exists() {
            return Ok(ArchiveMessages::new(archive_uid, ""));
        }
        let data = read_to_string(&path, "read archive messages")?;
        let mut messages: ArchiveMessages = serde_json::from_str(&data)
            .map_err(|e| AppError::json("parse archive messages", &e))?;
        if messages.schema_name != "archive_messages" || messages.schema_version != 1 {
            return Err(AppError::new(
                "archive_messages_invalid",
                format!(
                    "invalid archive messages schema: {}",
                    rel_path(&self.root, &path)
                ),
            ));
        }
        messages.archive_uid = archive_uid.to_string();
        Ok(messages)
    }

    pub(super) fn write_archive_messages(
        &self,
        archive_uid: &str,
        data: &ArchiveMessages,
    ) -> Result<()> {
        self.write_archive_messages_named(archive_uid, &data.archive_name, data)
    }

    pub(super) fn write_archive_messages_named(
        &self,
        archive_uid: &str,
        archive_name: &str,
        data: &ArchiveMessages,
    ) -> Result<()> {
        let mut normalized = data.clone();
        normalized.schema_name = "archive_messages".to_string();
        normalized.schema_version = 1;
        normalized.archive_uid = archive_uid.to_string();
        normalized.archive_name = archive_name.to_string();
        normalized
            .items
            .sort_by(|a, b| a.message_id.cmp(&b.message_id));
        normalized
            .items
            .dedup_by(|a, b| a.message_id == b.message_id);
        write_json_pretty(&self.archive_message_json_path(archive_uid), &normalized)
    }

    pub(super) fn upsert_archive_message_item(
        &self,
        archive_uid: &str,
        message_id: &str,
        summary: Option<&str>,
        archived_rfc3339: &str,
    ) -> Result<()> {
        let mut data = self.read_archive_messages(archive_uid)?;
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
            item.archived_rfc3339 = archived_rfc3339.to_string();
        } else {
            data.items.push(ArchiveMessageItem {
                message_id: message_id.to_string(),
                summary,
                archived_rfc3339: archived_rfc3339.to_string(),
            });
        }
        self.write_archive_messages(archive_uid, &data)
    }

    pub(super) fn remove_archive_message_item(
        &self,
        archive_uid: &str,
        message_id: &str,
    ) -> Result<()> {
        let mut data = self.read_archive_messages(archive_uid)?;
        let before = data.items.len();
        data.items.retain(|item| item.message_id != message_id);
        if data.items.len() == before {
            return Err(AppError::new(
                "archive_entry_not_found",
                format!("message {message_id} is not in archive {archive_uid}"),
            ));
        }
        self.write_archive_messages(archive_uid, &data)
    }

    pub(super) fn update_archive_message_summary(
        &self,
        archive_uid: &str,
        message_id: &str,
        summary: &str,
    ) -> Result<()> {
        let mut data = self.read_archive_messages(archive_uid)?;
        let item = data
            .items
            .iter_mut()
            .find(|item| item.message_id == message_id)
            .ok_or_else(|| {
                AppError::new(
                    "archive_entry_not_found",
                    format!("message {message_id} is not in archive {archive_uid}"),
                )
            })?;
        item.summary = Some(summary.trim().to_string()).filter(|value| !value.is_empty());
        self.write_archive_messages(archive_uid, &data)
    }

    pub(super) fn refresh_archive_indexes(&self) -> Result<()> {
        create_dir_all(&self.root.join("archive/cases"))?;
        create_dir_all(&self.root.join("archive/notifications"))?;
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        for archive_uid in self.archive_message_category_ids()? {
            self.refresh_archive_message_category_with_renderer(&archive_uid, &mut renderer, true)?;
        }
        Ok(())
    }

    pub(super) fn refresh_archive_message_category(&self, archive_uid: &str) -> Result<()> {
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        self.refresh_archive_message_category_with_renderer(archive_uid, &mut renderer, true)?;
        Ok(())
    }

    pub(super) fn refresh_archive_message_category_with_renderer(
        &self,
        archive_uid: &str,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        sync_state: bool,
    ) -> Result<ArchiveMessageViewRefresh> {
        validate_archive_uid(archive_uid)?;
        let archive_dir = self.archive_message_dir(archive_uid);
        let messages_dir = archive_dir.join("views").join("messages");
        create_dir_all(&messages_dir)?;
        let mut data = self.read_archive_messages(archive_uid)?;
        if sync_state {
            data.items.retain(|item| {
                self.read_message_by_id(&item.message_id)
                    .map(|message| message.workspace.archive_uid.as_deref() == Some(archive_uid))
                    .unwrap_or(false)
            });
            self.write_archive_messages(archive_uid, &data)?;
        }
        let items = data
            .items
            .iter()
            .filter(|item| {
                self.read_message_by_id(&item.message_id)
                    .map(|message| message.workspace.archive_uid.as_deref() == Some(archive_uid))
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        let desired = items
            .iter()
            .map(|item| item.message_id.clone())
            .collect::<BTreeSet<_>>();
        let mut message_count = 0usize;
        for item in &items {
            let message = self.read_message_by_id(&item.message_id)?;
            let view_path = self.archive_message_view_path(archive_uid, &item.message_id);
            write_string(
                &view_path,
                &self.render_archive_message_view(
                    &message,
                    archive_uid,
                    &data.archive_name,
                    item,
                    renderer,
                    view_path.parent(),
                )?,
            )?;
            message_count += 1;
        }
        if messages_dir.exists() {
            for entry in read_dir(&messages_dir, "read archive message views")? {
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
        write_string(
            &archive_dir.join("archive.md"),
            &self.render_archive_message_index(
                archive_uid,
                &data.archive_name,
                &items,
                &config,
                renderer,
            )?,
        )?;
        Ok(ArchiveMessageViewRefresh {
            archive_message_index_count: 1,
            archive_message_count: message_count,
        })
    }

    pub(super) fn render_archive_message_view(
        &self,
        message: &MessageFile,
        archive_uid: &str,
        archive_name: &str,
        item: &ArchiveMessageItem,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        output_dir: Option<&Path>,
    ) -> Result<String> {
        let config = MailConfig::load(&self.root)?;
        let title = message.subject.as_deref().unwrap_or("");
        let message_value = message_template_value(message)?;
        let item_value = serde_json::to_value(item)
            .map_err(|e| AppError::json("serialize archive message item", &e))?;
        let generated_rfc3339 = now_rfc3339();
        let conversation =
            self.message_conversation_with_renderer(message, &config, renderer, output_dir)?;
        let context = json!({
            "frontmatter": {
                "kind": "archive_message",
                "message_id": message.message_id.as_str(),
                "archive_uid": archive_uid,
                "archive_name": archive_name,
                "archived_rfc3339": item.archived_rfc3339.as_str(),
                "generated_rfc3339": generated_rfc3339.as_str(),
            },
            "language": config.resolved_language_bcp47(),
            "archive_uid": archive_uid,
            "archive_name": archive_name,
            "message_id": message.message_id.as_str(),
            "title": title,
            "summary": item.summary.as_deref().unwrap_or(""),
            "archived_rfc3339": item.archived_rfc3339.as_str(),
            "generated_rfc3339": generated_rfc3339.as_str(),
            "conversation": conversation.trim(),
            "message": message_value,
            "item": item_value,
        });
        renderer.render(TemplateKey::ArchiveMessage, &context)
    }

    pub(super) fn render_archive_message_index(
        &self,
        archive_uid: &str,
        archive_name: &str,
        data_items: &[ArchiveMessageItem],
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<String> {
        let mut items = data_items.to_vec();
        items.sort_by(|a, b| {
            let a_time = self
                .read_message_by_id(&a.message_id)
                .ok()
                .and_then(|message| message_time(&message))
                .unwrap_or_else(|| a.archived_rfc3339.clone());
            let b_time = self
                .read_message_by_id(&b.message_id)
                .ok()
                .and_then(|message| message_time(&message))
                .unwrap_or_else(|| b.archived_rfc3339.clone());
            compare_rfc3339_asc(&b_time, &a_time).then_with(|| a.message_id.cmp(&b.message_id))
        });
        let mut rendered_items = Vec::new();
        let offset = config.resolved_timezone_offset();
        for item in &items {
            let message = self.read_message_by_id(&item.message_id)?;
            let fields = config
                .archive
                .message_index
                .item_fields
                .iter()
                .filter_map(|field| archive_index_field_value(*field, &message, item, &offset))
                .collect::<Vec<_>>();
            let has_message_id = config
                .archive
                .message_index
                .item_fields
                .contains(&ArchiveMessageIndexField::MessageId);
            let has_link = config
                .archive
                .message_index
                .item_fields
                .contains(&ArchiveMessageIndexField::Link);
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
                config.template_language(),
                format!("views/messages/{}.md", item.message_id),
                title,
            )?;
            if let Value::Object(map) = &mut rendered_item {
                map.insert(
                    "summary".to_string(),
                    json!(item.summary.as_deref().unwrap_or("")),
                );
                map.insert(
                    "display_summary".to_string(),
                    json!(item
                        .summary
                        .as_deref()
                        .map(markdown_inline)
                        .unwrap_or_default()),
                );
                map.insert(
                    "archived_rfc3339".to_string(),
                    json!(item.archived_rfc3339.as_str()),
                );
                map.insert(
                    "archived_time".to_string(),
                    time_context(&item.archived_rfc3339, &offset),
                );
                map.insert("fields".to_string(), json!(fields));
                map.insert("secondary".to_string(), json!(second));
                map.insert(
                    "item".to_string(),
                    serde_json::to_value(item)
                        .map_err(|e| AppError::json("serialize archive message item", &e))?,
                );
            }
            rendered_items.push(rendered_item);
        }
        let context = json!({
            "archive_uid": archive_uid,
            "archive_name": archive_name,
            "message_count": rendered_items.len(),
            "generated_rfc3339": now_rfc3339(),
            "language": config.resolved_language_bcp47(),
            "items": rendered_items,
            "config": {
                "archive": {
                    "message_index": {
                        "item_fields": config.archive.message_index.item_fields
                            .iter()
                            .map(|field| field.as_str())
                            .collect::<Vec<_>>(),
                    },
                },
            },
        });
        renderer.render(TemplateKey::ArchiveMessageIndex, &context)
    }

    pub(super) fn archive_message_category_ids(&self) -> Result<Vec<String>> {
        let dir = self.root.join("archive/notifications");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in read_dir(&dir, "read archive messages directory")? {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if path.join("data").join("archive.json").is_file() {
                        if let Some(uid) = archive_uid_from_dir_name(name) {
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

    pub(super) fn archive_message_category_items(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for archive_uid in self.archive_message_category_ids()? {
            let data = self.read_archive_messages(&archive_uid)?;
            let path = self.archive_message_dir(&archive_uid);
            out.push(json!({
                "archive_uid": archive_uid,
                "archive_name": data.archive_name,
                "archive_dir": path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
            }));
        }
        Ok(out)
    }

    pub(super) fn resolve_archive_message_category(
        &self,
        archive_ref: &str,
    ) -> Result<(String, PathBuf)> {
        let archive_uid = parse_archive_ref(archive_ref)?;
        self.find_archive_message_dir_by_uid(&archive_uid)?
            .map(|path| (archive_uid.clone(), path))
            .ok_or_else(|| {
                AppError::new(
                    "archive_not_found",
                    format!("archive message category not found: {archive_uid}"),
                )
            })
    }

    pub(super) fn find_archive_message_dir_by_uid(
        &self,
        archive_uid: &str,
    ) -> Result<Option<PathBuf>> {
        validate_archive_uid(archive_uid)?;
        let dir = self.root.join("archive/notifications");
        if !dir.exists() {
            return Ok(None);
        }
        let mut matches = Vec::new();
        for entry in read_dir(&dir, "read archive messages directory")? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if archive_uid_from_dir_name(name).as_deref() == Some(archive_uid) {
                matches.push(path);
            }
        }
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(AppError::new(
                "duplicate_archive_uid",
                format!("duplicate archive uid found: {archive_uid}"),
            )),
        }
    }
}
