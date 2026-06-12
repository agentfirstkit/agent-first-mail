use super::*;

#[derive(Debug, Clone)]
pub(super) struct ArchivedCaseEntry {
    pub(super) case_uid: String,
    pub(super) path: PathBuf,
}

pub(super) fn case_name(case_path: &Path) -> Result<String> {
    Ok(read_case_file(case_path)?.case_name)
}

pub(super) fn update_case_name(case_path: &Path, case_name: &str) -> Result<()> {
    let mut case = read_case_file(case_path)?;
    case.case_name = case_name.to_string();
    case.updated_rfc3339 = Some(now_rfc3339());
    write_case_file(case_path, &case)
}

pub(super) fn update_case_archive_state(case_path: &Path, status: &str) -> Result<()> {
    let mut case = read_case_file(case_path)?;
    case.status = status.to_string();
    case.updated_rfc3339 = Some(now_rfc3339());
    if status == "archived" {
        case.archived_rfc3339.get_or_insert_with(now_rfc3339);
    } else {
        case.archived_rfc3339 = None;
    }
    write_case_file(case_path, &case)
}

pub(super) fn new_case_file(
    case_uid: &str,
    case_name: &str,
    message_ids: &[String],
) -> CaseFrontmatter {
    let now = now_rfc3339();
    CaseFrontmatter {
        kind: "case".to_string(),
        case_uid: case_uid.to_string(),
        case_name: case_name.to_string(),
        status: "active".to_string(),
        tags: Vec::new(),
        created_rfc3339: Some(now.clone()),
        updated_rfc3339: Some(now.clone()),
        archived_rfc3339: None,
        message_count: message_ids.len(),
        thread_count: 0,
        attachment_count: 0,
        last_message_rfc3339: Some(now),
    }
}

pub(super) fn new_notes_md(root: &Path, language: TemplateLanguage) -> Result<String> {
    render_template(
        root,
        language,
        TemplateKey::NotesDefault,
        &json!({"language": language.as_str()}),
    )
}

pub(super) fn merge_case_notes(
    root: &Path,
    language: TemplateLanguage,
    case_uid: &str,
    primary: &Path,
    other: &Path,
    other_case_uid: &str,
) -> Result<()> {
    let other_notes_path = other.join("notes.md");
    if !other_notes_path.exists() {
        return Ok(());
    }
    let other_notes = read_to_string(&other_notes_path, "read merged notes.md")?;
    if other_notes.trim().is_empty() {
        return Ok(());
    }
    let primary_notes_path = primary.join("notes.md");
    if !primary_notes_path.exists() {
        return Err(notes_missing_error(root, &primary_notes_path));
    }
    let existing = read_to_string(&primary_notes_path, "read primary notes.md")?;
    let section = render_template(
        root,
        language,
        TemplateKey::NotesMergeSection,
        &json!({
            "language": language.as_str(),
            "case_uid": case_uid,
            "other_case_uid": other_case_uid,
            "other_body": other_notes,
        }),
    )?;
    let merged = format!("{}\n\n{}", existing.trim_end(), section.trim_start());
    write_string(&primary_notes_path, &merged)
}

pub(super) fn update_case_counts(
    case: &mut CaseFrontmatter,
    added_ids: &[String],
    attachment_count: Option<usize>,
) {
    case.message_count += added_ids.len();
    case.updated_rfc3339 = Some(now_rfc3339());
    if let Some(count) = attachment_count {
        case.attachment_count = count;
    }
}

pub(super) fn read_case_messages(path: &Path, case_uid: &str) -> Result<CaseMessages> {
    if !path.exists() {
        return Ok(CaseMessages::new(case_uid));
    }
    let data = read_to_string(path, "read case messages")?;
    let mut messages: CaseMessages =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse case messages", &e))?;
    if messages.schema_name != "case_messages" || messages.schema_version != 1 {
        return Err(AppError::new(
            "case_messages_invalid",
            format!("invalid case messages schema: {}", path_to_string(path)),
        ));
    }
    messages.case_uid = case_uid.to_string();
    Ok(messages)
}

pub(super) fn existing_triage_suggestion(path: &Path) -> Result<(Vec<String>, Option<String>)> {
    if !path.exists() {
        return Ok((Vec::new(), None));
    }
    let text = read_to_string(path, "read triage file")?;
    let (fm, _) = read_doc::<TriageFrontmatter>(&text)?;
    if fm.suggested_case_uids.is_empty() {
        return Ok((Vec::new(), None));
    }
    Ok((fm.suggested_case_uids, fm.suggested_reason))
}

impl Workspace {
    pub fn create_case(
        &self,
        name: &str,
        group: Option<&str>,
        message_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = if message_id.is_some() {
            self.checked_reason(reason)?
        } else {
            reason.map(str::trim).filter(|value| !value.is_empty())
        };
        validate_name("case_name", name)?;
        if let Some(message_id) = message_id {
            validate_id("message_id", message_id)?;
        }
        let config = MailConfig::load(&self.root)?;
        let group = group.unwrap_or(config.case.default_group.as_str());
        validate_id("group", group)?;
        let date = if let Some(message_id) = message_id {
            self.first_related_message_date(message_id)?
        } else {
            workspace_local_date(&config.resolved_timezone_offset())
        };
        let case_uid = self.next_case_uid(&date)?;
        let case_path = self
            .root
            .join("cases")
            .join(group)
            .join(case_dir_name(&case_uid, name));
        if case_path.exists() {
            return Err(AppError::new(
                "case_exists",
                format!("case path already exists: {}", path_to_string(&case_path)),
            ));
        }
        create_dir_all(&case_data_dir(&case_path))?;
        create_dir_all(&case_views_messages_dir(&case_path))?;
        create_dir_all(&case_path.join("drafts"))?;
        create_dir_all(&case_path.join("files"))?;
        let message_ids = if let Some(message_id) = message_id {
            vec![message_id.to_string()]
        } else {
            Vec::new()
        };
        write_case_file(&case_path, &new_case_file(&case_uid, name, &message_ids))?;
        let mut case_messages = CaseMessages::new(&case_uid);
        case_messages.merge_ids(&message_ids);
        write_json_pretty(&case_messages_json_path(&case_path), &case_messages)?;
        write_string_new(
            &case_path.join("notes.md"),
            &new_notes_md(&self.root, config.template_language())?,
        )?;
        if !message_ids.is_empty() {
            self.refresh_messages_after_ref_change(&message_ids)?;
        }
        self.refresh_case_message_views(&case_path)?;
        let mut result = json!({
            "code": "case_created",
            "case_uid": case_uid,
            "case_name": name,
            "group": group,
            "message_ids": message_ids,
            "message_count": case_messages.message_ids.len(),
            "case_path": rel_path(&self.root, &case_path)
        });
        if !case_messages.message_ids.is_empty() {
            let locations = self.message_remote_locations_any(&case_messages.message_ids)?;
            let item = crate::push_queue::queue_action_steps(
                &self.root,
                "case.add",
                &case_messages.message_ids,
                &locations,
                &config.actions.case_add.steps,
                None,
            )?;
            if let Some(item) = &item {
                self.record_pending_push_item(item)?;
            }
            add_queue_fields(&mut result, locations.len(), item.as_ref());
        }
        self.append_audit_event(
            "case_created",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "case_name": name,
                "group": group,
                "message_ids": case_messages.message_ids,
                "case_path": rel_path(&self.root, &case_path),
            }),
        )?;
        Ok(result)
    }

    pub fn add_message_to_case(
        &self,
        case_ref: &str,
        message_id: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let case_uid = parse_case_ref(case_ref)?;
        validate_id("message_id", message_id)?;
        let existing = self.find_case_by_uid(&case_uid)?;
        if existing.is_none() && self.find_archived_case_by_uid(&case_uid)?.is_some() {
            return Err(case_archived_error(&case_uid));
        }
        let case_path = existing.ok_or_else(|| {
            AppError::new("case_not_found", format!("case not found: {case_uid}"))
        })?;
        let mut result = self.add_message_to_existing_case(&case_uid, message_id, &case_path)?;
        let config = MailConfig::load(&self.root)?;
        let message_ids = vec![message_id.to_string()];
        let locations = self.message_remote_locations_any(&message_ids)?;
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            "case.add",
            &message_ids,
            &locations,
            &config.actions.case_add.steps,
            None,
        )?;
        if let Some(item) = &item {
            self.record_pending_push_item(item)?;
        }
        add_queue_fields(&mut result, locations.len(), item.as_ref());
        self.append_audit_event(
            "case_message_added",
            vec![
                audit_target("case", &case_uid),
                audit_target("message", message_id),
            ],
            reason,
            json!({
                "case_uid": case_uid,
                "message_id": message_id,
                "group": result.get("group").and_then(Value::as_str).unwrap_or_default(),
            }),
        )?;
        Ok(result)
    }

    pub(super) fn add_message_to_existing_case(
        &self,
        case_uid: &str,
        message_id: &str,
        case_path: &Path,
    ) -> Result<Value> {
        let related_message_ids = self.related_message_ids(message_id)?;
        let messages_path = case_messages_json_path(case_path);
        let mut case_messages = read_case_messages(&messages_path, case_uid)?;
        let already_present = case_messages
            .message_ids
            .iter()
            .any(|existing| existing == message_id);
        if !already_present {
            let mut case = read_case_file(case_path)?;
            update_case_counts(&mut case, &[message_id.to_string()], None);
            write_case_file(case_path, &case)?;
            case_messages.merge_ids(&[message_id.to_string()]);
            write_json_pretty(&messages_path, &case_messages)?;
        }
        self.refresh_messages_after_ref_change(&[message_id.to_string()])?;
        self.refresh_case_message_views(case_path)?;
        let group = case_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        Ok(json!({
            "code": "case_message_added",
            "case_uid": case_uid,
            "message_id": message_id,
            "group": group,
            "created_case": false,
            "message_count": 1,
            "already_present": already_present,
            "related_message_ids": related_message_ids,
            "case_path": rel_path(&self.root, case_path)
        }))
    }

    pub fn move_case(&self, case_ref: &str, group: &str) -> Result<Value> {
        self.require_workspace()?;
        validate_id("group", group)?;
        let (case_uid, from) = self.resolve_active_case(case_ref)?;
        let from_group = from
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let from_parent = from.parent().map(Path::to_path_buf);
        let dir_name = from
            .file_name()
            .ok_or_else(|| AppError::new("store_error", "case has no directory name"))?;
        let to = self.root.join("cases").join(group).join(dir_name);
        if to == from {
            return Ok(json!({
                "code": "case_moved",
                "case_uid": case_uid,
                "from_group": from_group,
                "to_group": group,
                "case_path": rel_path(&self.root, &to)
            }));
        }
        if to.exists() {
            return Err(AppError::new(
                "duplicate_case_uid",
                format!("target case path already exists: {}", path_to_string(&to)),
            ));
        }
        if let Some(parent) = to.parent() {
            create_dir_all(parent)?;
        }
        fs::rename(&from, &to).map_err(|e| AppError::io("move case", &e))?;
        if let Some(parent) = from_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        self.refresh_case_message_views(&to)?;
        Ok(json!({
            "code": "case_moved",
            "case_uid": case_uid,
            "from_group": from_group,
            "to_group": group,
            "case_path": rel_path(&self.root, &to)
        }))
    }

    pub(super) fn ensure_case_has_no_local_drafts(
        &self,
        case_uid: &str,
        case_path: &Path,
    ) -> Result<()> {
        let drafts_dir = case_path.join("drafts");
        let mut draft_names = Vec::new();
        if drafts_dir.exists() {
            for entry in read_dir(&drafts_dir, "read drafts")? {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    draft_names.push(path_file_name(&path));
                }
            }
        }
        draft_names.sort();
        if draft_names.is_empty() {
            return Ok(());
        }
        Err(AppError::new(
            "case_has_local_drafts",
            format!(
                "case {case_uid} has local drafts: {}",
                draft_names.join(", ")
            ),
        )
        .with_hint(
            "Validate and compose drafts to queue them, or remove drafts before archive/merge.",
        )
        .with_details(json!({
            "case_uid": case_uid,
            "draft_names": draft_names,
            "suggested_commands": [
                format!("afmail case draft validate {case_uid} DRAFT_NAME"),
                format!("afmail case compose {case_uid} DRAFT_NAME"),
                format!("afmail case draft remove {case_uid} DRAFT_NAME --reason TEXT")
            ]
        })))
    }

    pub(super) fn ensure_case_has_no_outbound_push(&self, case_uid: &str) -> Result<()> {
        let push_dir = self.root.join(".afmail/push");
        if !push_dir.exists() {
            return Ok(());
        }
        let mut push_ids = Vec::new();
        for entry in read_dir(&push_dir, "read push queue")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let data = read_to_string(&path, "read push item")?;
            let item = PushItem::parse_json(&data)?;
            if item
                .outbound()
                .is_some_and(|outbound| outbound.case_uid == case_uid)
            {
                push_ids.push(item.push_id);
            }
        }
        push_ids.sort();
        if push_ids.is_empty() {
            return Ok(());
        }
        Err(AppError::new(
            "case_has_outbound_push",
            format!(
                "case {case_uid} has queued outbound push items: {}",
                push_ids.join(", ")
            ),
        )
        .with_hint("Push queued drafts or remove the corresponding drafts before archive/merge.")
        .with_details(json!({
            "case_uid": case_uid,
            "push_ids": push_ids,
            "suggested_commands": [
                "afmail push drafts-send --dry-run",
                "afmail push drafts-send --confirm",
                format!("afmail case draft remove {case_uid} DRAFT_NAME --reason TEXT")
            ]
        })))
    }

    pub fn archive_case(&self, case_ref: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        self.ensure_case_has_no_local_drafts(&case_uid, &case_path)?;
        self.ensure_case_has_no_outbound_push(&case_uid)?;
        let from_path = rel_path(&self.root, &case_path);
        let messages = read_case_messages(&case_messages_json_path(&case_path), &case_uid)?;
        let transaction = self.begin_transaction(
            "case_archive",
            vec![
                from_path.clone(),
                format!("archive/cases/{case_uid}"),
                ".afmail/push".to_string(),
            ],
        )?;
        let archived_path = self.archive_active_case_workspace(&case_uid, &case_path)?;
        self.refresh_messages_after_ref_change(&messages.message_ids)?;
        self.refresh_case_message_views(&archived_path)?;
        let queue = self.queue_archive_for_archived_messages(&messages.message_ids, None)?;
        transaction.commit()?;
        self.append_audit_event(
            "case_archived",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "from_path": from_path,
                "to_path": rel_path(&self.root, &archived_path),
                "message_ids": messages.message_ids,
            }),
        )?;
        Ok(json!({
            "code": "case_archived",
            "case_uid": case_uid,
            "message_count": messages.message_ids.len(),
            "eligible_message_ids": queue.eligible_message_ids,
            "location_count": queue.location_count,
            "queued_location_count": queue.queued_location_count,
            "queued": !queue.items.is_empty(),
            "push_ids": queue.items.iter().map(|item| item.push_id.clone()).collect::<Vec<_>>(),
            "push_id": queue.items.first().map(|item| item.push_id.clone()),
            "from_path": from_path,
            "case_path": rel_path(&self.root, &archived_path)
        }))
    }

    pub fn reopen_case(&self, case_ref: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let messages = read_case_messages(&case_messages_json_path(&case_path), &case_uid)?;
        self.set_case_status(&case_path, "active")?;
        self.refresh_messages_after_ref_change(&messages.message_ids)?;
        let result = json!({
            "code": "case_reopened",
            "case_uid": case_uid,
            "status": "active",
            "message_count": messages.message_ids.len(),
            "case_path": rel_path(&self.root, &case_path)
        });
        self.append_audit_event(
            "case_reopened",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({"case_uid": case_uid}),
        )?;
        Ok(result)
    }

    pub fn tag_case(&self, case_ref: &str, tag: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("tag", tag)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let tags = self.update_case_tags(&case_path, Some(tag), None)?;
        let result = json!({
            "code": "case_tagged",
            "case_uid": case_uid,
            "tag": tag,
            "tags": tags,
            "case_path": rel_path(&self.root, &case_path)
        });
        self.append_audit_event(
            "case_tagged",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({"case_uid": case_uid, "tag": tag, "tags": tags}),
        )?;
        Ok(result)
    }

    pub fn untag_case(&self, case_ref: &str, tag: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("tag", tag)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let tags = self.update_case_tags(&case_path, None, Some(tag))?;
        let result = json!({
            "code": "case_untagged",
            "case_uid": case_uid,
            "tag": tag,
            "tags": tags,
            "case_path": rel_path(&self.root, &case_path)
        });
        self.append_audit_event(
            "case_untagged",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({"case_uid": case_uid, "tag": tag, "tags": tags}),
        )?;
        Ok(result)
    }

    pub fn merge_case(
        &self,
        case_ref: &str,
        other_case_ref: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (case_uid, primary) = self.resolve_active_case(case_ref)?;
        let (other_case_uid, other) = self.resolve_active_case(other_case_ref)?;
        if case_uid == other_case_uid {
            return Err(AppError::new(
                "invalid_request",
                "cannot merge a case into itself",
            ));
        }
        self.ensure_case_has_no_local_drafts(&case_uid, &primary)?;
        self.ensure_case_has_no_local_drafts(&other_case_uid, &other)?;
        self.ensure_case_has_no_outbound_push(&case_uid)?;
        self.ensure_case_has_no_outbound_push(&other_case_uid)?;
        ensure_no_name_conflicts(&primary.join("files"), &other.join("files"), "files")?;
        ensure_no_name_conflicts(&primary.join("drafts"), &other.join("drafts"), "drafts")?;
        let mut primary_messages =
            read_case_messages(&case_messages_json_path(&primary), &case_uid)?;
        let other_messages = read_case_messages(&case_messages_json_path(&other), &other_case_uid)?;
        primary_messages.merge_ids(&other_messages.message_ids);
        write_json_pretty(&case_messages_json_path(&primary), &primary_messages)?;
        let mut primary_case = read_case_file(&primary)?;
        primary_case.message_count = primary_messages.message_ids.len();
        primary_case.updated_rfc3339 = Some(now_rfc3339());
        write_case_file(&primary, &primary_case)?;
        merge_case_notes(
            &self.root,
            self.template_language()?,
            &case_uid,
            &primary,
            &other,
            &other_case_uid,
        )?;
        move_children(&other.join("files"), &primary.join("files"))?;
        move_children(&other.join("drafts"), &primary.join("drafts"))?;
        let other_parent = other.parent().map(Path::to_path_buf);
        remove_dir_all(&other)?;
        if let Some(parent) = other_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        self.refresh_messages_after_ref_change(&other_messages.message_ids)?;
        self.refresh_case_message_views(&primary)?;
        self.append_audit_event(
            "case_merged",
            vec![
                audit_target("case", &case_uid),
                audit_target("case", &other_case_uid),
            ],
            reason,
            json!({
                "case_uid": case_uid,
                "merged_case_uid": other_case_uid,
                "message_ids": other_messages.message_ids,
            }),
        )?;
        Ok(json!({
            "code": "case_merged",
            "case_uid": case_uid,
            "merged_case_uid": other_case_uid,
            "message_count": other_messages.message_ids.len()
        }))
    }

    pub fn rename_active_case(
        &self,
        case_ref: &str,
        name: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_name("case_name", name)?;
        let (case_uid, from) = self.resolve_active_case(case_ref)?;
        let old_name = case_name(&from)?;
        let group = from
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let to = from
            .parent()
            .ok_or_else(|| AppError::new("store_error", "case has no parent directory"))?
            .join(case_dir_name(&case_uid, name));
        let changed_path = to != from;
        if changed_path && to.exists() {
            return Err(AppError::new(
                "duplicate_case_uid",
                format!("target case path already exists: {}", path_to_string(&to)),
            ));
        }
        if changed_path {
            fs::rename(&from, &to).map_err(|e| AppError::io("rename case", &e))?;
        }
        update_case_name(&to, name)?;
        self.refresh_case_message_views(&to)?;
        self.append_audit_event(
            "case_renamed",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "old_case_name": old_name,
                "case_name": name,
                "group": group,
                "from_path": rel_path(&self.root, &from),
                "to_path": rel_path(&self.root, &to),
            }),
        )?;
        Ok(json!({
            "code": "case_renamed",
            "case_uid": case_uid,
            "old_case_name": old_name,
            "case_name": name,
            "group": group,
            "case_path": rel_path(&self.root, &to),
            "changed": old_name != name || changed_path
        }))
    }

    pub fn active_case_show(&self, case_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        self.refresh_case_message_views(&case_path)?;
        let view_path = case_path.join("case.md");
        let text = read_to_string(&view_path, "read active case")?;
        let case = read_case_file(&case_path)?;
        let group = case_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        Ok(json!({
            "code": "case",
            "case_uid": case_uid,
            "case_name": case.case_name,
            "group": group,
            "case_path": rel_path(&self.root, &case_path),
            "view_path": rel_path(&self.root, &view_path),
            "messages_path": rel_path(&self.root, &case_views_messages_dir(&case_path)),
            "text": text,
        }))
    }

    pub fn case_list(&self) -> Result<Value> {
        self.require_workspace()?;
        let items = self.active_case_items()?;
        Ok(json!({
            "code": "case_list",
            "count": items.len(),
            "path_templates": {
                "case_path": "cases/{group}/{case_dir}",
                "view_path": "cases/{group}/{case_dir}/case.md",
                "data_path": "cases/{group}/{case_dir}/data/case.json",
            },
            "items": items,
        }))
    }

    pub fn active_case_notes_show(&self, case_ref: &str) -> Result<Value> {
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        self.notes_show(
            "case_notes",
            vec![audit_target("case", &case_uid)],
            &case_path.join("notes.md"),
        )
    }

    pub fn active_case_notes_append(&self, case_ref: &str, text: &str) -> Result<Value> {
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        self.notes_append(
            "case_notes_appended",
            vec![audit_target("case", &case_uid)],
            &case_path.join("notes.md"),
            text,
        )
    }

    pub fn active_case_notes_replace(&self, case_ref: &str, text: &str) -> Result<Value> {
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        self.notes_replace(
            "case_notes_replaced",
            vec![audit_target("case", &case_uid)],
            &case_path.join("notes.md"),
            text,
        )
    }

    pub fn archive_case_show(&self, case_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let (case_uid, entry) = self.resolve_archived_case(case_ref)?;
        self.refresh_case_message_views(&entry.path)?;
        let path = entry.path.join("case.md");
        let text = read_to_string(&path, "read archived case")?;
        let name = case_name(&entry.path)?;
        Ok(json!({
            "code": "archive_case",
            "case_uid": case_uid,
            "case_name": name,
            "case_path": rel_path(&self.root, &entry.path),
            "view_path": rel_path(&self.root, &path),
            "text": text,
        }))
    }

    pub fn archive_case_restore(
        &self,
        case_ref: &str,
        group: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("group", group)?;
        let (case_uid, entry) = self.resolve_archived_case(case_ref)?;
        let dir_name = entry
            .path
            .file_name()
            .ok_or_else(|| AppError::new("store_error", "case has no directory name"))?;
        let active_path = self.root.join("cases").join(group).join(dir_name);
        if active_path.exists() {
            return Err(AppError::new(
                "case_exists",
                format!(
                    "active case path already exists: {}",
                    path_to_string(&active_path)
                ),
            ));
        }
        let messages = read_case_messages(&case_messages_json_path(&entry.path), &case_uid)?;
        update_case_archive_state(&entry.path, "active")?;
        if let Some(parent) = active_path.parent() {
            create_dir_all(parent)?;
        }
        fs::rename(&entry.path, &active_path).map_err(|e| AppError::io("restore case", &e))?;
        self.refresh_messages_after_ref_change(&messages.message_ids)?;
        self.refresh_case_message_views(&active_path)?;
        self.append_audit_event(
            "case_restored",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "to_group": group,
                "from_path": rel_path(&self.root, &entry.path),
                "to_path": rel_path(&self.root, &active_path),
            }),
        )?;
        Ok(json!({
            "code": "case_restored",
            "case_uid": case_uid,
            "group": group,
            "case_path": rel_path(&self.root, &active_path)
        }))
    }

    pub fn archive_case_rename(
        &self,
        case_ref: &str,
        name: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_name("case_name", name)?;
        let (case_uid, entry) = self.resolve_archived_case(case_ref)?;
        let old_name = case_name(&entry.path)?;
        let to = self.archive_case_path_for_name(&case_uid, name);
        let changed_path = to != entry.path;
        if changed_path && to.exists() {
            return Err(AppError::new(
                "duplicate_case_uid",
                format!(
                    "target archived case path already exists: {}",
                    path_to_string(&to)
                ),
            ));
        }
        if let Some(parent) = to.parent() {
            create_dir_all(parent)?;
        }
        if changed_path {
            fs::rename(&entry.path, &to).map_err(|e| AppError::io("rename archived case", &e))?;
        }
        update_case_name(&to, name)?;
        self.refresh_case_message_views(&to)?;
        self.append_audit_event(
            "archive_case_renamed",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "old_case_name": old_name,
                "case_name": name,
                "from_path": rel_path(&self.root, &entry.path),
                "to_path": rel_path(&self.root, &to),
            }),
        )?;
        Ok(json!({
            "code": "archive_case_renamed",
            "case_uid": case_uid,
            "old_case_name": old_name,
            "case_name": name,
            "case_path": rel_path(&self.root, &to),
            "changed": old_name != name || changed_path
        }))
    }

    pub fn archive_case_notes_show(&self, case_ref: &str) -> Result<Value> {
        let (case_uid, entry) = self.resolve_archived_case(case_ref)?;
        self.notes_show(
            "case_notes",
            vec![audit_target("case", &case_uid)],
            &entry.path.join("notes.md"),
        )
    }

    pub fn archive_case_notes_append(&self, case_ref: &str, text: &str) -> Result<Value> {
        let (case_uid, entry) = self.resolve_archived_case(case_ref)?;
        self.notes_append(
            "case_notes_appended",
            vec![audit_target("case", &case_uid)],
            &entry.path.join("notes.md"),
            text,
        )
    }

    pub fn archive_case_notes_replace(&self, case_ref: &str, text: &str) -> Result<Value> {
        let (case_uid, entry) = self.resolve_archived_case(case_ref)?;
        self.notes_replace(
            "case_notes_replaced",
            vec![audit_target("case", &case_uid)],
            &entry.path.join("notes.md"),
            text,
        )
    }

    pub(super) fn archive_active_case_workspace(
        &self,
        case_uid: &str,
        case_path: &Path,
    ) -> Result<PathBuf> {
        validate_case_uid(case_uid)?;
        let dir_name = case_path
            .file_name()
            .ok_or_else(|| AppError::new("store_error", "case has no directory name"))?;
        let archived_path = self.root.join("archive").join("cases").join(dir_name);
        if archived_path.exists() {
            return Err(AppError::new(
                "case_exists",
                format!(
                    "archived case already exists: {}",
                    path_to_string(&archived_path)
                ),
            ));
        }
        if let Some(parent) = archived_path.parent() {
            create_dir_all(parent)?;
        }
        let source_parent = case_path.parent().map(Path::to_path_buf);
        fs::rename(case_path, &archived_path).map_err(|e| AppError::io("archive case", &e))?;
        update_case_archive_state(&archived_path, "archived")?;
        if let Some(parent) = source_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        Ok(archived_path)
    }

    pub(super) fn remove_empty_case_container_dir(&self, dir: &Path) -> Result<bool> {
        if !self.is_removable_case_container_dir(dir) {
            return Ok(false);
        }
        match fs::remove_dir(dir) {
            Ok(()) => Ok(true),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(AppError::io("remove empty case container directory", &e)),
        }
    }

    pub(super) fn is_removable_case_container_dir(&self, dir: &Path) -> bool {
        let active_cases_dir = self.root.join("cases");
        dir.parent() == Some(active_cases_dir.as_path()) && dir != active_cases_dir
    }

    pub(super) fn active_case_items(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for (case_uid, path) in self.case_entries()? {
            let group = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            out.push(json!({
                "case_uid": case_uid,
                "case_name": case_name(&path).unwrap_or_default(),
                "group": group,
                "case_dir": path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
            }));
        }
        out.sort_by(|a, b| {
            let a_key = (
                a.get("group").and_then(Value::as_str).unwrap_or_default(),
                a.get("case_uid")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            let b_key = (
                b.get("group").and_then(Value::as_str).unwrap_or_default(),
                b.get("case_uid")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            a_key.cmp(&b_key)
        });
        Ok(out)
    }

    pub(super) fn archive_case_items(&self) -> Result<Vec<Value>> {
        Ok(self
            .archived_case_entries()?
            .into_iter()
            .map(|entry| {
                json!({
                    "case_uid": entry.case_uid,
                    "case_name": case_name(&entry.path).unwrap_or_default(),
                    "case_dir": entry
                        .path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default(),
                })
            })
            .collect())
    }

    pub(super) fn notes_show(&self, code: &str, targets: Vec<Value>, path: &Path) -> Result<Value> {
        let text = read_existing_notes(&self.root, path)?;
        Ok(json!({
            "code": code,
            "targets": targets,
            "notes_path": rel_path(&self.root, path),
            "text": text
        }))
    }

    pub(super) fn notes_append(
        &self,
        kind: &str,
        targets: Vec<Value>,
        path: &Path,
        text: &str,
    ) -> Result<Value> {
        let mut existing = read_existing_notes(&self.root, path)?;
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(text);
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        write_string(path, &existing)?;
        self.append_audit_event(
            kind,
            targets.clone(),
            None,
            json!({
                "operation": "append",
                "notes_path": rel_path(&self.root, path),
                "text_len_bytes": text.len(),
                "text_hash": stable_text_hash(text),
            }),
        )?;
        Ok(json!({
            "code": kind,
            "targets": targets,
            "notes_path": rel_path(&self.root, path),
            "text_len_bytes": text.len(),
            "text_hash": stable_text_hash(text)
        }))
    }

    pub(super) fn notes_replace(
        &self,
        kind: &str,
        targets: Vec<Value>,
        path: &Path,
        text: &str,
    ) -> Result<Value> {
        let mut data = text.to_string();
        if !data.ends_with('\n') {
            data.push('\n');
        }
        write_string(path, &data)?;
        self.append_audit_event(
            kind,
            targets.clone(),
            None,
            json!({
                "operation": "replace",
                "notes_path": rel_path(&self.root, path),
                "text_len_bytes": text.len(),
                "text_hash": stable_text_hash(text),
            }),
        )?;
        Ok(json!({
            "code": kind,
            "targets": targets,
            "notes_path": rel_path(&self.root, path),
            "text_len_bytes": text.len(),
            "text_hash": stable_text_hash(text)
        }))
    }

    pub fn find_case_required(&self, case_ref: &str) -> Result<PathBuf> {
        self.resolve_active_case(case_ref).map(|(_, path)| path)
    }

    pub(super) fn resolve_active_case(&self, case_ref: &str) -> Result<(String, PathBuf)> {
        let case_uid = parse_case_ref(case_ref)?;
        if let Some(path) = self.find_case_by_uid(&case_uid)? {
            return Ok((case_uid, path));
        }
        if self.find_archived_case_by_uid(&case_uid)?.is_some() {
            return Err(case_archived_error(&case_uid));
        }
        Err(AppError::new(
            "case_not_found",
            format!("case not found: {case_uid}"),
        ))
    }

    pub(super) fn resolve_archived_case(
        &self,
        case_ref: &str,
    ) -> Result<(String, ArchivedCaseEntry)> {
        let case_uid = parse_case_ref(case_ref)?;
        self.find_archived_case_by_uid(&case_uid)?
            .map(|entry| (case_uid.clone(), entry))
            .ok_or_else(|| {
                AppError::new(
                    "case_not_found",
                    format!("archived case not found: {case_uid}"),
                )
            })
    }

    pub fn find_case(&self, case_ref: &str) -> Result<Option<PathBuf>> {
        let case_uid = parse_case_ref(case_ref)?;
        self.find_case_by_uid(&case_uid)
    }

    pub(super) fn find_case_by_uid(&self, case_uid: &str) -> Result<Option<PathBuf>> {
        validate_case_uid(case_uid)?;
        let mut matches = Vec::new();
        for (id, path) in self.case_entries()? {
            if id == case_uid {
                matches.push(path);
            }
        }
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(AppError::new(
                "duplicate_case_uid",
                format!("duplicate case uid found: {case_uid}"),
            )),
        }
    }

    pub(super) fn find_archived_case_by_uid(
        &self,
        case_uid: &str,
    ) -> Result<Option<ArchivedCaseEntry>> {
        validate_case_uid(case_uid)?;
        let mut matches = Vec::new();
        for entry in self.archived_case_entries()? {
            if entry.case_uid == case_uid {
                matches.push(entry);
            }
        }
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(AppError::new(
                "duplicate_case_uid",
                format!("duplicate archived case uid found: {case_uid}"),
            )),
        }
    }

    pub(super) fn case_entries(&self) -> Result<Vec<(String, PathBuf)>> {
        let cases_dir = self.root.join("cases");
        if !cases_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for group_entry in read_dir(&cases_dir, "read cases directory")? {
            let group_path = group_entry.path();
            if !group_path.is_dir() {
                continue;
            }
            for case_entry in read_dir(&group_path, "read case group")? {
                let case_path = case_entry.path();
                if !case_path.is_dir() || !case_json_path(&case_path).is_file() {
                    continue;
                }
                let fm = read_case_file(&case_path)?;
                out.push((fm.case_uid, case_path));
            }
        }
        Ok(out)
    }

    pub(super) fn all_case_entries(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut out = self.case_entries()?;
        out.extend(
            self.archived_case_entries()?
                .into_iter()
                .map(|entry| (entry.case_uid, entry.path)),
        );
        Ok(out)
    }

    pub(super) fn archived_case_entries(&self) -> Result<Vec<ArchivedCaseEntry>> {
        let cases_dir = self.root.join("archive/cases");
        if !cases_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for case_entry in read_dir(&cases_dir, "read archived cases")? {
            let case_path = case_entry.path();
            if !case_path.is_dir() || !case_json_path(&case_path).is_file() {
                continue;
            }
            let fm = read_case_file(&case_path)?;
            out.push(ArchivedCaseEntry {
                case_uid: fm.case_uid,
                path: case_path,
            });
        }
        out.sort_by(|a, b| a.case_uid.cmp(&b.case_uid));
        Ok(out)
    }

    pub(super) fn set_case_status(&self, case_path: &Path, status: &str) -> Result<()> {
        let mut fm = read_case_file(case_path)?;
        fm.status = status.to_string();
        fm.updated_rfc3339 = Some(now_rfc3339());
        if status != "archived" {
            fm.archived_rfc3339 = None;
        }
        write_case_file(case_path, &fm)
    }

    pub(super) fn update_case_tags(
        &self,
        case_path: &Path,
        add_tag: Option<&str>,
        remove_tag: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut fm = read_case_file(case_path)?;
        if let Some(tag) = add_tag {
            merge_string(&mut fm.tags, tag);
        }
        if let Some(tag) = remove_tag {
            fm.tags.retain(|item| item != tag);
        }
        fm.updated_rfc3339 = Some(now_rfc3339());
        let tags = fm.tags.clone();
        write_case_file(case_path, &fm)?;
        Ok(tags)
    }
}
