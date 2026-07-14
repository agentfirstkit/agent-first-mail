use super::*;

#[derive(Debug, Clone)]
pub(super) struct ArchivedCaseEntry {
    pub(super) case_uid: String,
    pub(super) group: String,
    pub(super) path: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedCase {
    pub(super) case_uid: String,
    pub(super) group: String,
    pub(super) path: PathBuf,
    pub(super) archived: bool,
}

#[derive(Debug, Clone, Copy)]
struct CaseMessageRemovalContext<'a> {
    event_kind: &'a str,
    result_code: &'a str,
    undo_push_kind: &'a str,
    archived: bool,
}

#[cfg(test)]
thread_local! {
    static FAIL_CASE_CREATE_AFTER_LOCAL_WRITES: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn set_case_create_after_local_writes_failure(enabled: bool) {
    FAIL_CASE_CREATE_AFTER_LOCAL_WRITES.with(|fail| fail.set(enabled));
}

#[cfg(test)]
fn maybe_fail_case_create_after_local_writes() -> Result<()> {
    if FAIL_CASE_CREATE_AFTER_LOCAL_WRITES.with(std::cell::Cell::get) {
        return Err(AppError::new(
            "case_create_test_failure",
            "forced case create failure after local writes",
        ));
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_fail_case_create_after_local_writes() -> Result<()> {
    Ok(())
}

pub(super) fn case_name(case_path: &Path) -> Result<String> {
    Ok(read_case_file(case_path)?.collection_name)
}

pub(super) fn update_case_name(case_path: &Path, case_name: &str) -> Result<()> {
    let mut case = read_case_file(case_path)?;
    case.collection_name = case_name.to_string();
    case.updated_rfc3339 = Some(now_rfc3339());
    write_case_file(case_path, &case)
}

pub(super) fn update_case_archive_state(case_path: &Path, archived: bool) -> Result<()> {
    let mut case = read_case_file(case_path)?;
    case.updated_rfc3339 = Some(now_rfc3339());
    if archived {
        case.status = "archived".to_string();
        case.archived_rfc3339.get_or_insert_with(now_rfc3339);
    } else {
        case.status.clear();
        case.archived_rfc3339 = None;
    }
    write_case_file(case_path, &case)
}

fn update_case_archiving_state(case_path: &Path) -> Result<()> {
    let mut case = read_case_file(case_path)?;
    case.status = "archiving".to_string();
    case.archived_rfc3339 = None;
    case.updated_rfc3339 = Some(now_rfc3339());
    write_case_file(case_path, &case)
}

fn case_lifecycle(case: &CaseFrontmatter, stored_under_archived: bool) -> &'static str {
    if stored_under_archived && case.status == "archiving" && case.archived_rfc3339.is_none() {
        "archiving"
    } else if stored_under_archived || case.archived_rfc3339.is_some() {
        "archived"
    } else {
        "active"
    }
}

pub(super) fn new_case_file(
    case_uid: &str,
    case_name: &str,
    items: Vec<MessageCollectionItem>,
) -> CaseFrontmatter {
    let now = now_rfc3339();
    let mut case = CaseFrontmatter::new_case(case_uid, case_name, &now);
    case.items = items;
    case.message_count = case.items.len();
    if !case.items.is_empty() {
        case.last_message_rfc3339 = Some(now);
    }
    case
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

fn remove_case_item(case: &mut CaseFrontmatter, message_id: &str) -> Option<MessageCollectionItem> {
    let position = case
        .items
        .iter()
        .position(|item| item.message_id == message_id)?;
    let item = case.items.remove(position);
    case.message_count = case.items.len();
    case.updated_rfc3339 = Some(now_rfc3339());
    if case.items.is_empty() {
        case.last_message_rfc3339 = None;
    }
    Some(item)
}

fn dir_entry_names(path: &Path, context: &str) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut names = read_dir(path, context)?
        .into_iter()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

pub(super) fn read_case_messages(case_path: &Path, case_uid: &str) -> Result<CaseMessages> {
    let messages = read_case_file(case_path)?;
    if messages.collection_uid != case_uid {
        return Err(AppError::new(
            "case_messages_invalid",
            format!(
                "invalid case uid in metadata: {}",
                path_to_string(&case_json_path(case_path))
            ),
        ));
    }
    Ok(messages)
}

pub(super) fn existing_triage_suggestion(path: &Path) -> Result<(Vec<String>, Option<String>)> {
    if !path.exists() {
        return Ok((Vec::new(), None));
    }
    let file = fs::File::open(path).map_err(|err| AppError::io("read triage file", &err))?;
    let mut lines = BufReader::new(file).lines();
    match lines.next() {
        Some(Ok(line)) if line == "---" => {}
        Some(Ok(_)) => {
            return Err(AppError::new(
                "invalid_request",
                "markdown file is missing frontmatter",
            ));
        }
        Some(Err(err)) => return Err(AppError::io("read triage file", &err)),
        None => {
            return Err(AppError::new(
                "invalid_request",
                "markdown file is missing frontmatter",
            ));
        }
    }
    let mut frontmatter = Vec::new();
    for line in lines {
        let line = line.map_err(|err| AppError::io("read triage file", &err))?;
        if line == "---" {
            let fm = parse_frontmatter::<TriageFrontmatter>(&frontmatter.join("\n"))?;
            if fm.suggested_case_uids.is_empty() {
                return Ok((Vec::new(), None));
            }
            return Ok((fm.suggested_case_uids, fm.suggested_reason));
        }
        frontmatter.push(line);
    }
    Err(AppError::new(
        "invalid_request",
        "markdown frontmatter is not closed",
    ))
}

impl Workspace {
    fn ensure_case_drafts_can_archive(&self, case_uid: &str, case_path: &Path) -> Result<()> {
        let draft_names = self.local_draft_names(case_path)?;
        if draft_names.is_empty() {
            return Ok(());
        }
        let pushes = self.outbound_pushes_for_case(case_uid)?;
        let queued_names = pushes
            .iter()
            .filter_map(PushItem::draft)
            .map(|draft| draft.draft_name.as_str())
            .collect::<BTreeSet<_>>();
        let blockers = draft_names
            .into_iter()
            .filter(|name| {
                if !queued_names.contains(name.as_str()) {
                    return true;
                }
                let path = case_path.join("drafts").join(name);
                read_to_string(&path, "read archive draft")
                    .and_then(|text| read_doc::<DraftFrontmatter>(&text).map(|(fm, _)| fm))
                    .map(|fm| fm.sync_intent != Some(OutboundAction::Send.into()))
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();
        if blockers.is_empty() {
            Ok(())
        } else {
            Err(case_has_local_drafts_error(case_uid, blockers).with_hint(
                "Only drafts queued with `case draft send` may continue through case archiving; push or remove upload/unqueued drafts first.",
            ))
        }
    }

    fn case_has_pending_archive_work(
        &self,
        case_uid: &str,
        message_ids: &[String],
    ) -> Result<bool> {
        let push_dir = self
            .root
            .join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
        if push_dir.exists() {
            for entry in read_dir(&push_dir, "read case archive push queue")? {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let item =
                    PushItem::parse_json(&read_to_string(&path, "read case archive push item")?)?;
                if item.draft().is_some_and(|draft| draft.case_uid == case_uid) {
                    return Ok(true);
                }
            }
        }
        for message_id in message_ids {
            let message = self.read_message_by_id(message_id)?;
            if message.workspace.push.is_some_and(|state| {
                state
                    .pending
                    .iter()
                    .any(|pending| pending.kind == "message.notification")
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn finalize_archiving_cases(&self) -> Result<()> {
        for entry in self.archived_case_entries()? {
            let case = read_case_file(&entry.path)?;
            if case_lifecycle(&case, true) != "archiving" {
                continue;
            }
            let message_ids = case.message_ids();
            if self.case_has_pending_archive_work(&entry.case_uid, &message_ids)? {
                continue;
            }
            update_case_archive_state(&entry.path, true)?;
            self.append_audit_event(
                "case_archived",
                vec![audit_target("case", &entry.case_uid)],
                None,
                json!({
                    "case_uid": entry.case_uid,
                    "case_path": rel_path(&self.root, &entry.path),
                    "message_ids": message_ids,
                    "lifecycle": "archived",
                }),
            )?;
        }
        Ok(())
    }

    pub fn create_case(
        &self,
        name: &str,
        group: Option<&str>,
        message_id: Option<&str>,
        summary: Option<&str>,
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
        let group = group.ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "case create requires --group CATEGORY; choose a meaningful path-safe category in the mailbox/workspace language, such as support, invoice, 客服, 发票, 合同, or 退款",
            )
        })?;
        let config = MailConfig::load(&self.root)?;
        validate_id("group", group)?;
        let date = if let Some(message_id) = message_id {
            self.first_related_message_date(message_id)?
        } else {
            workspace_local_date(&config.resolved_timezone_offset())
        };
        let case_uid = self.next_case_uid(&date)?;
        let case_path = self
            .root
            .join(layout::CASES)
            .join(group)
            .join(case_dir_name(&case_uid, name));
        if case_path.exists() {
            return Err(AppError::new(
                "case_exists",
                format!("case path already exists: {}", path_to_string(&case_path)),
            ));
        }
        let mut transaction_paths = vec![rel_path(&self.root, &case_path)];
        if let Some(message_id) = message_id {
            transaction_paths.push(format!("messages/{message_id}.json"));
            transaction_paths.push(".afmail/push".to_string());
        }
        let transaction = self.begin_transaction("case_create", transaction_paths)?;
        create_dir_all(&case_data_dir(&case_path))?;
        create_dir_all(&case_views_messages_dir(&case_path))?;
        create_dir_all(&case_path.join("drafts"))?;
        create_dir_all(&case_path.join("files"))?;
        let mut guard = DirGuard::new(case_path.clone());
        let added_rfc3339 = now_rfc3339();
        let items = message_id
            .map(|message_id| {
                vec![MessageCollectionItem {
                    message_id: message_id.to_string(),
                    summary: summary
                        .or(reason)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToString::to_string),
                    added_rfc3339: added_rfc3339.clone(),
                }]
            })
            .unwrap_or_default();
        let case_data = new_case_file(&case_uid, name, items);
        write_case_file(&case_path, &case_data)?;
        write_string_new(
            &case_path.join("notes.md"),
            &new_notes_md(&self.root, config.template_language())?,
        )?;
        let message_ids = case_data.message_ids();
        if !message_ids.is_empty() {
            for message_id in &message_ids {
                self.clear_message_from_all_dispositions(message_id)?;
            }
            self.refresh_messages_after_ref_change(&message_ids)?;
        }
        self.refresh_case_message_views(&case_path)?;
        let mut result = json!({
            "case_uid": case_uid,
            "case_name": name,
            "group": group,
            "message_ids": message_ids,
            "message_count": case_data.message_count,
            "case_path": rel_path(&self.root, &case_path)
        });
        if !message_ids.is_empty() {
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
        }
        maybe_fail_case_create_after_local_writes()?;
        transaction.commit()?;
        guard.commit();
        self.append_audit_event(
            "case_created",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "case_name": name,
                "group": group,
                "message_ids": message_ids,
                "summary": summary,
                "case_path": rel_path(&self.root, &case_path),
            }),
        )?;
        Ok(result)
    }

    pub fn add_message_to_case(
        &self,
        case_ref: &str,
        message_id: &str,
        summary: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let case_uid = parse_case_ref(case_ref)?;
        validate_id("message_id", message_id)?;
        let existing = self.find_case_by_uid(&case_uid)?;
        if existing.is_none()
            && let Some(entry) = self.find_archived_case_by_uid(&case_uid)?
        {
            return Err(case_archived_error(&case_uid, Some(entry.group.as_str())));
        }
        let case_path = existing.ok_or_else(|| {
            AppError::new("case_not_found", format!("case not found: {case_uid}"))
        })?;
        let mut result = self.add_message_to_existing_case(
            &case_uid,
            message_id,
            summary.or(reason),
            &case_path,
        )?;
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
                "summary": summary,
                "group": result.get("group").and_then(Value::as_str).unwrap_or_default(),
            }),
        )?;
        Ok(result)
    }

    pub fn remove_message_from_case(
        &self,
        case_ref: &str,
        message_id: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let case = self.resolve_case_any(case_ref)?;
        self.remove_message_from_case_workspace(
            &case.case_uid,
            &case.path,
            message_id,
            CaseMessageRemovalContext {
                event_kind: "case_message_removed",
                result_code: "case_message_removed",
                undo_push_kind: if case.archived {
                    "message.notification"
                } else {
                    "case.add"
                },
                archived: case.archived,
            },
            reason,
        )
    }

    pub(super) fn add_message_to_existing_case(
        &self,
        case_uid: &str,
        message_id: &str,
        summary: Option<&str>,
        case_path: &Path,
    ) -> Result<Value> {
        let related_message_ids = self.related_message_ids(message_id)?;
        let mut case = read_case_file(case_path)?;
        let already_present = case.contains_message(message_id);
        if !already_present {
            let added_rfc3339 = now_rfc3339();
            case.upsert_item(message_id, summary, &added_rfc3339);
            update_case_counts(&mut case, &[message_id.to_string()], None);
            write_case_file(case_path, &case)?;
        } else if summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            let added_rfc3339 = now_rfc3339();
            case.upsert_item(message_id, summary, &added_rfc3339);
            case.updated_rfc3339 = Some(added_rfc3339);
            write_case_file(case_path, &case)?;
        }
        self.clear_message_from_all_dispositions(message_id)?;
        self.refresh_messages_after_ref_change(&[message_id.to_string()])?;
        self.refresh_case_message_views(case_path)?;
        let group = case_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        Ok(json!({
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

    pub fn case_set_summary(
        &self,
        case_ref: &str,
        message_id: &str,
        summary: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let case = self.resolve_case_any(case_ref)?;
        validate_id("message_id", message_id)?;
        let mut data = read_case_file(&case.path)?;
        let Some(item) = data
            .items
            .iter_mut()
            .find(|item| item.message_id == message_id)
        else {
            return Err(AppError::new(
                "case_message_not_found",
                format!("message {message_id} is not in case {}", case.case_uid),
            ));
        };
        item.summary = Some(summary.trim().to_string());
        data.updated_rfc3339 = Some(now_rfc3339());
        write_case_file(&case.path, &data)?;
        self.refresh_case_message_views(&case.path)?;
        self.append_audit_event(
            "case_summary_set",
            vec![
                audit_target("case", &case.case_uid),
                audit_target("message", message_id),
            ],
            reason,
            json!({
                "case_uid": case.case_uid,
                "message_id": message_id,
                "summary": summary,
            }),
        )?;
        Ok(json!({
            "case_uid": case.case_uid,
            "message_id": message_id,
            "summary": summary,
        }))
    }

    pub fn move_case(&self, case_ref: &str, group: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("group", group)?;
        let case = self.resolve_case_any(case_ref)?;
        let from_path = rel_path(&self.root, &case.path);
        let from_parent = case.path.parent().map(Path::to_path_buf);
        let dir_name = case
            .path
            .file_name()
            .ok_or_else(|| AppError::new("store_error", "case has no directory name"))?;
        let root_dir = if case.archived {
            layout::ARCHIVED_CASES
        } else {
            layout::CASES
        };
        let to = self.root.join(root_dir).join(group).join(dir_name);
        let to_path = rel_path(&self.root, &to);
        if to == case.path {
            self.append_audit_event(
                "case_moved",
                vec![audit_target("case", &case.case_uid)],
                reason,
                json!({
                    "case_uid": case.case_uid,
                    "from_group": case.group,
                    "to_group": group,
                    "from_path": from_path,
                    "to_path": to_path,
                    "archived": case.archived,
                    "changed": false,
                }),
            )?;
            return Ok(json!({
                "case_uid": case.case_uid,
                "from_group": case.group,
                "to_group": group,
                "archived": case.archived,
                "changed": false,
                "from_path": from_path,
                "to_path": to_path,
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
        fs::rename(&case.path, &to).map_err(|e| AppError::io("move case", &e))?;
        if let Some(parent) = from_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        self.refresh_case_message_views(&to)?;
        self.append_audit_event(
            "case_moved",
            vec![audit_target("case", &case.case_uid)],
            reason,
            json!({
                "case_uid": case.case_uid,
                "from_group": case.group,
                "to_group": group,
                "from_path": from_path,
                "to_path": to_path,
                "archived": case.archived,
                "changed": true,
            }),
        )?;
        Ok(json!({
            "case_uid": case.case_uid,
            "from_group": case.group,
            "to_group": group,
            "archived": case.archived,
            "changed": true,
            "from_path": from_path,
            "to_path": to_path,
            "case_path": rel_path(&self.root, &to)
        }))
    }

    pub(super) fn ensure_case_has_no_local_drafts(
        &self,
        case_uid: &str,
        case_path: &Path,
    ) -> Result<()> {
        let draft_names = self.local_draft_names(case_path)?;
        if draft_names.is_empty() {
            return Ok(());
        }
        Err(case_has_local_drafts_error(case_uid, draft_names))
    }

    fn local_draft_names(&self, case_path: &Path) -> Result<Vec<String>> {
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
        Ok(draft_names)
    }

    fn outbound_pushes_for_case(&self, case_uid: &str) -> Result<Vec<PushItem>> {
        self.outbound_pushes_for_case_except(case_uid, None)
    }

    fn outbound_pushes_for_case_except(
        &self,
        case_uid: &str,
        ignored_push_id: Option<&str>,
    ) -> Result<Vec<PushItem>> {
        let push_dir = self
            .root
            .join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
        if !push_dir.exists() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        for entry in read_dir(&push_dir, "read push queue")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let data = read_to_string(&path, "read push item")?;
            let item = PushItem::parse_json(&data)?;
            if ignored_push_id.is_some_and(|push_id| push_id == item.push_id) {
                continue;
            }
            if item.draft().is_some_and(|draft| draft.case_uid == case_uid) {
                items.push(item);
            }
        }
        items.sort_by(|a, b| a.push_id.cmp(&b.push_id));
        Ok(items)
    }

    pub(super) fn ensure_case_has_no_outbound_push(&self, case_uid: &str) -> Result<()> {
        let push_ids = self
            .outbound_pushes_for_case(case_uid)?
            .into_iter()
            .map(|item| item.push_id)
            .collect::<Vec<_>>();
        if push_ids.is_empty() {
            return Ok(());
        }
        Err(case_has_outbound_push_error(case_uid, push_ids))
    }
}

fn case_has_local_drafts_error(case_uid: &str, draft_names: Vec<String>) -> AppError {
    AppError::new(
            "case_has_local_drafts",
            format!(
                "case {case_uid} has local drafts: {}",
                draft_names.join(", ")
            ),
        )
        .with_hint(
            "Queue drafts with `afmail case draft upload` or `afmail case draft send`, push them, or remove drafts before archive/merge/trash.",
        )
        .with_details(json!({
            "case_uid": case_uid,
            "draft_names": draft_names,
            "suggested_commands": [
                format!("afmail case draft validate {case_uid} DRAFT_NAME"),
                format!("afmail case draft upload {case_uid} DRAFT_NAME"),
                format!("afmail case draft send {case_uid} DRAFT_NAME"),
                format!("afmail case draft remove {case_uid} DRAFT_NAME --reason TEXT")
            ]
        }))
}

fn case_has_outbound_push_error(case_uid: &str, push_ids: Vec<String>) -> AppError {
    AppError::new(
        "case_has_outbound_push",
        format!(
            "case {case_uid} has queued outbound push items: {}",
            push_ids.join(", ")
        ),
    )
    .with_hint("Push queued drafts or remove the corresponding drafts before archive/merge/trash.")
    .with_details(json!({
        "case_uid": case_uid,
        "push_ids": push_ids,
        "suggested_commands": [
            "afmail push",
            format!("afmail case draft remove {case_uid} DRAFT_NAME --reason TEXT")
        ]
    }))
}

impl Workspace {
    pub(super) fn ensure_case_messages_unreferenced_except(
        &self,
        case_uid: &str,
        case_path: &Path,
        message_ids: &[String],
    ) -> Result<()> {
        let ids = message_ids.iter().cloned().collect::<BTreeSet<_>>();
        if ids.is_empty() {
            return Ok(());
        }
        let source_path = rel_path(&self.root, case_path);
        let mut references = Vec::new();
        for (_, other_case_path) in self.all_case_entries()? {
            if rel_path(&self.root, &other_case_path) == source_path {
                continue;
            }
            let data_path = case_json_path(&other_case_path);
            if !data_path.exists() {
                continue;
            }
            let data = read_to_string(&data_path, "read case metadata")?;
            let value = serde_json::from_str::<Value>(&data)
                .map_err(|e| AppError::json("parse case metadata", &e))?;
            if json_contains_any_id(&value, &ids) {
                references.push(rel_path(&self.root, &data_path));
            }
        }
        self.collect_draft_references(&ids, &mut references)?;
        self.collect_push_references(&ids, None, &mut references)?;
        self.collect_notification_references(&ids, &mut references)?;
        if references.is_empty() {
            return Ok(());
        }
        Err(AppError::new(
            "case_messages_referenced",
            format!(
                "case {case_uid} messages cannot be trashed while referenced by {}",
                references.join(", ")
            ),
        )
        .with_hint(
            "Remove or move the listed case/archived-case/push references before trashing the case.",
        )
        .with_details(json!({
            "case_uid": case_uid,
            "message_ids": message_ids,
            "references": references,
            "suggested_commands": [
                "afmail status",
                "afmail push list",
                format!("afmail case show {case_uid}"),
                "afmail notification show NOTIFICATION_REF"
            ]
        })))
    }

    pub fn archive_case(&self, case_ref: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let from_path = rel_path(&self.root, &case_path);
        let messages = read_case_messages(&case_path, &case_uid)?;
        let message_ids = messages.message_ids();
        self.ensure_case_drafts_can_archive(&case_uid, &case_path)?;
        let transaction = self.begin_transaction(
            "case_archive",
            vec![
                from_path.clone(),
                format!("{}/*/{case_uid}", layout::ARCHIVED_CASES),
                ".afmail/push".to_string(),
            ],
        )?;
        let target_path =
            self.move_active_case_workspace(&case_uid, &case_path, layout::ARCHIVED_CASES)?;
        self.refresh_messages_after_ref_change(&message_ids)?;
        self.refresh_case_message_views(&target_path)?;
        let queue =
            self.queue_archive_for_archived_messages(&message_ids, None, Some(&case_uid))?;
        let lifecycle = if self.case_has_pending_archive_work(&case_uid, &message_ids)? {
            update_case_archiving_state(&target_path)?;
            "archiving"
        } else {
            "archived"
        };
        transaction.commit()?;
        self.append_audit_event(
            if lifecycle == "archived" {
                "case_archived"
            } else {
                "case_archiving"
            },
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "from_path": from_path,
                "to_path": rel_path(&self.root, &target_path),
                "message_ids": message_ids,
                "lifecycle": lifecycle,
            }),
        )?;
        Ok(json!({
            "lifecycle": lifecycle,
            "case_uid": case_uid,
            "message_count": messages.message_count,
            "eligible_message_ids": queue.eligible_message_ids,
            "location_count": queue.location_count,
            "queued_location_count": queue.queued_location_count,
            "queued": !queue.items.is_empty(),
            "push_ids": queue.items.iter().map(|item| item.push_id.clone()).collect::<Vec<_>>(),
            "push_id": queue.items.first().map(|item| item.push_id.clone()),
            "from_path": from_path,
            "case_path": rel_path(&self.root, &target_path),
        }))
    }

    pub fn trash_case(&self, case_ref: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let case = self.resolve_case_any(case_ref)?;
        let source_status = if case.archived { "archived" } else { "active" };
        self.trash_case_workspace(&case.case_uid, &case.path, source_status, reason)
    }

    pub fn delete_case(
        &self,
        case_ref: &str,
        allow_notes: bool,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let case = self.resolve_case_any(case_ref)?;
        self.delete_case_workspace(
            &case.case_uid,
            &case.path,
            case.archived,
            allow_notes,
            "case_deleted",
            reason,
        )
    }

    pub fn tag_case(&self, case_ref: &str, tag: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("tag", tag)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let tags = self.update_case_tags(&case_path, Some(tag), None)?;
        let result = json!({
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
        let mut primary_messages = read_case_messages(&primary, &case_uid)?;
        let other_messages = read_case_messages(&other, &other_case_uid)?;
        primary_messages.merge_items(&other_messages.items);
        primary_messages.updated_rfc3339 = Some(now_rfc3339());
        write_case_file(&primary, &primary_messages)?;
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
        let other_message_ids = other_messages.message_ids();
        self.refresh_messages_after_ref_change(&other_message_ids)?;
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
                "message_ids": other_message_ids,
            }),
        )?;
        Ok(json!({
            "case_uid": case_uid,
            "merged_case_uid": other_case_uid,
            "message_count": other_messages.message_count
        }))
    }

    pub fn rename_case(&self, case_ref: &str, name: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_name("case_name", name)?;
        let case = self.resolve_case_any(case_ref)?;
        let old_name = case_name(&case.path)?;
        let to = case
            .path
            .parent()
            .ok_or_else(|| AppError::new("store_error", "case has no parent directory"))?
            .join(case_dir_name(&case.case_uid, name));
        let changed_path = to != case.path;
        if changed_path && to.exists() {
            return Err(AppError::new(
                "duplicate_case_uid",
                format!("target case path already exists: {}", path_to_string(&to)),
            ));
        }
        if changed_path {
            fs::rename(&case.path, &to).map_err(|e| AppError::io("rename case", &e))?;
        }
        update_case_name(&to, name)?;
        self.refresh_case_message_views(&to)?;
        self.append_audit_event(
            "case_renamed",
            vec![audit_target("case", &case.case_uid)],
            reason,
            json!({
                "case_uid": case.case_uid,
                "old_case_name": old_name,
                "case_name": name,
                "group": case.group,
                "archived": case.archived,
                "from_path": rel_path(&self.root, &case.path),
                "to_path": rel_path(&self.root, &to),
            }),
        )?;
        Ok(json!({
            "case_uid": case.case_uid,
            "old_case_name": old_name,
            "case_name": name,
            "group": case.group,
            "archived": case.archived,
            "case_path": rel_path(&self.root, &to),
            "changed": old_name != name || changed_path
        }))
    }

    pub fn case_show(&self, case_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let case = self.resolve_case_any(case_ref)?;
        let data = read_case_file(&case.path)?;
        for message_id in data.message_ids() {
            self.read_message_by_id(&message_id)?;
        }
        let view_path = case.path.join("case.md");
        let text = read_generated_view(&self.root, &view_path, "case index")?;
        Ok(json!({
            "case_uid": case.case_uid,
            "case_name": data.collection_name,
            "group": case.group,
            "archived": case.archived,
            "lifecycle": case_lifecycle(&data, case.archived),
            "case_path": rel_path(&self.root, &case.path),
            "view_path": rel_path(&self.root, &view_path),
            "messages_path": rel_path(&self.root, &case_views_messages_dir(&case.path)),
            "text": text,
        }))
    }

    pub fn case_list(&self, archived: bool, all: bool) -> Result<Value> {
        self.require_workspace()?;
        let selected_count = usize::from(archived) + usize::from(all);
        if selected_count > 1 {
            return Err(AppError::new(
                "invalid_request",
                "--archived and --all cannot be combined",
            ));
        }
        let all_archived_items = self.archived_case_items()?;
        let archiving_items = all_archived_items
            .iter()
            .filter(|item| item["lifecycle"] == "archiving")
            .cloned()
            .collect::<Vec<_>>();
        let completed_archived_items = all_archived_items
            .iter()
            .filter(|item| item["lifecycle"] == "archived")
            .cloned()
            .collect::<Vec<_>>();
        let (scope, mut active_items, archived_items) = if all {
            ("all", self.active_case_items()?, all_archived_items)
        } else if archived {
            ("archived", Vec::new(), completed_archived_items)
        } else {
            ("active", self.active_case_items()?, Vec::new())
        };
        let active_count = active_items.len();
        let archiving_count = if archived { 0 } else { archiving_items.len() };
        if !archived && !all {
            active_items.extend(archiving_items);
        }
        let mut items = active_items.clone();
        items.extend(archived_items.clone());
        let pt_active_case = [layout::CASES, "/{group}/{case_dir}"].concat();
        let pt_active_view = [layout::CASES, "/{group}/{case_dir}/case.md"].concat();
        let pt_active_data = [layout::CASES, "/{group}/{case_dir}/data/case.json"].concat();
        let pt_archived_case = [layout::ARCHIVED_CASES, "/{group}/{case_dir}"].concat();
        let pt_archived_view = [layout::ARCHIVED_CASES, "/{group}/{case_dir}/case.md"].concat();
        let pt_archived_data =
            [layout::ARCHIVED_CASES, "/{group}/{case_dir}/data/case.json"].concat();
        let path_templates = if all {
            json!({
                "active_case_path": pt_active_case,
                "active_view_path": pt_active_view,
                "active_data_path": pt_active_data,
                "archived_case_path": pt_archived_case,
                "archived_view_path": pt_archived_view,
                "archived_data_path": pt_archived_data,
            })
        } else if archived {
            json!({
                "case_path": pt_archived_case,
                "view_path": pt_archived_view,
                "data_path": pt_archived_data,
            })
        } else {
            json!({
                "active_case_path": pt_active_case,
                "active_view_path": pt_active_view,
                "active_data_path": pt_active_data,
                "archiving_case_path": pt_archived_case,
                "archiving_view_path": pt_archived_view,
                "archiving_data_path": pt_archived_data,
            })
        };
        Ok(json!({
            "scope": scope,
            "count": items.len(),
            "active_count": active_count,
            "archiving_count": archiving_count,
            "archived_count": archived_items.len(),
            "path_templates": path_templates,
            "items": items,
        }))
    }

    pub fn case_notes_show(&self, case_ref: &str) -> Result<Value> {
        let case = self.resolve_case_any(case_ref)?;
        self.notes_show(
            "case_notes",
            vec![audit_target("case", &case.case_uid)],
            &case.path.join("notes.md"),
        )
    }

    pub fn case_notes_append(&self, case_ref: &str, text: &str) -> Result<Value> {
        let case = self.resolve_case_any(case_ref)?;
        self.notes_append(
            "case_notes_appended",
            vec![audit_target("case", &case.case_uid)],
            &case.path.join("notes.md"),
            text,
        )
    }

    pub fn case_notes_replace(&self, case_ref: &str, text: &str) -> Result<Value> {
        let case = self.resolve_case_any(case_ref)?;
        self.notes_replace(
            "case_notes_replaced",
            vec![audit_target("case", &case.case_uid)],
            &case.path.join("notes.md"),
            text,
        )
    }

    pub fn unarchive_case(
        &self,
        case_ref: &str,
        group: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("group", group)?;
        let (case_uid, entry, source_status) = self.resolve_unarchive_case(case_ref)?;
        let dir_name = entry
            .path
            .file_name()
            .ok_or_else(|| AppError::new("store_error", "case has no directory name"))?;
        let active_path = self.root.join(layout::CASES).join(group).join(dir_name);
        if active_path.exists() {
            return Err(AppError::new(
                "case_exists",
                format!(
                    "active case path already exists: {}",
                    path_to_string(&active_path)
                ),
            ));
        }
        let messages = read_case_messages(&entry.path, &case_uid)?;
        update_case_archive_state(&entry.path, false)?;
        if let Some(parent) = active_path.parent() {
            create_dir_all(parent)?;
        }
        let archived_parent = entry.path.parent().map(Path::to_path_buf);
        fs::rename(&entry.path, &active_path).map_err(|e| AppError::io("unarchive case", &e))?;
        if let Some(parent) = archived_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        let message_ids = messages.message_ids();
        let removed_pushes =
            self.remove_pending_message_pushes_for_messages(&message_ids, "message.notification")?;
        self.refresh_messages_after_ref_change(&message_ids)?;
        self.refresh_case_message_views(&active_path)?;
        self.append_audit_event(
            "case_unarchived",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "to_group": group,
                "source_status": source_status,
                "from_path": rel_path(&self.root, &entry.path),
                "to_path": rel_path(&self.root, &active_path),
                "removed_push_ids": removed_pushes.clone(),
            }),
        )?;
        Ok(json!({
            "case_uid": case_uid,
            "group": group,
            "source_status": source_status,
            "case_path": rel_path(&self.root, &active_path),
            "removed_push_count": removed_pushes.len(),
            "push_ids": removed_pushes
        }))
    }

    pub(super) fn move_active_case_workspace(
        &self,
        case_uid: &str,
        case_path: &Path,
        target_root: &str,
    ) -> Result<PathBuf> {
        validate_case_uid(case_uid)?;
        let dir_name = case_path
            .file_name()
            .ok_or_else(|| AppError::new("store_error", "case has no directory name"))?;
        let group = case_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::new("store_error", "case has no group directory"))?;
        validate_id("group", group)?;
        let target_path = self.root.join(target_root).join(group).join(dir_name);
        if target_path.exists() {
            return Err(AppError::new(
                "case_exists",
                format!(
                    "target case already exists: {}",
                    path_to_string(&target_path)
                ),
            ));
        }
        if let Some(parent) = target_path.parent() {
            create_dir_all(parent)?;
        }
        let source_parent = case_path.parent().map(Path::to_path_buf);
        fs::rename(case_path, &target_path).map_err(|e| AppError::io("archive case", &e))?;
        update_case_archive_state(&target_path, target_root == layout::ARCHIVED_CASES)?;
        if let Some(parent) = source_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        Ok(target_path)
    }

    fn remove_message_from_case_workspace(
        &self,
        case_uid: &str,
        case_path: &Path,
        message_id: &str,
        context: CaseMessageRemovalContext<'_>,
        reason: Option<&str>,
    ) -> Result<Value> {
        validate_case_uid(case_uid)?;
        validate_id("message_id", message_id)?;
        let mut case = read_case_messages(case_path, case_uid)?;
        let removed_item = remove_case_item(&mut case, message_id).ok_or_else(|| {
            AppError::new(
                "case_message_not_found",
                format!("message {message_id} is not in case {case_uid}"),
            )
            .with_details(json!({"case_uid": case_uid, "message_id": message_id}))
        })?;
        let transaction = self.begin_transaction(
            context.result_code,
            vec![
                rel_path(&self.root, case_path),
                format!("messages/{message_id}.json"),
                ".afmail/push".to_string(),
            ],
        )?;
        write_case_file(case_path, &case)?;
        let view_path = case_message_view_path(case_path, message_id);
        if view_path.exists() {
            remove_file(&view_path)?;
        }
        let removed_push = crate::push_queue::remove_pending_message_pushes(
            &self.root,
            message_id,
            context.undo_push_kind,
        )?;
        let push_ids = removed_push
            .iter()
            .map(|item| item.push_id.clone())
            .collect::<Vec<_>>();
        let mut message = self.read_message_by_id(message_id)?;
        message.workspace.status = MessageStatus::Triage.as_str().to_string();
        message.workspace.origin = None;
        message.workspace.remote_sync = None;
        self.write_message_materialized_cache(&message)?;
        self.refresh_message_after_ref_change(message_id)?;
        self.clear_message_pending_pushes(message_id, &push_ids, false)?;
        self.refresh_case_message_views(case_path)?;
        transaction.commit()?;
        self.append_audit_event(
            context.event_kind,
            vec![
                audit_target("case", case_uid),
                audit_target("message", message_id),
            ],
            reason,
            json!({
                "case_uid": case_uid,
                "message_id": message_id,
                "summary": removed_item.summary,
                "removed_push_ids": push_ids.clone(),
            }),
        )?;
        Ok(json!({
            "case_uid": case_uid,
            "message_id": message_id,
            "message_count": case.items.len(),
            "archived": context.archived,
            "triage_path": format!("{}/{message_id}.md", layout::TRIAGE),
            "removed_push_count": push_ids.len(),
            "push_ids": push_ids,
        }))
    }

    fn delete_case_workspace(
        &self,
        case_uid: &str,
        case_path: &Path,
        archived: bool,
        allow_notes: bool,
        result_code: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        validate_case_uid(case_uid)?;
        let case = read_case_messages(case_path, case_uid)?;
        let message_ids = case.message_ids();
        if !message_ids.is_empty() {
            return Err(AppError::new(
                "case_not_empty",
                format!(
                    "case {case_uid} still contains {} message(s)",
                    message_ids.len()
                ),
            )
            .with_hint("Remove all messages from the case before deleting the collection.")
            .with_details(json!({
                "case_uid": case_uid,
                "message_count": message_ids.len(),
                "message_ids": message_ids,
                "suggested_commands": [
                    format!("afmail case remove {case_uid} MESSAGE_ID --reason TEXT"),
                ],
            })));
        }
        self.ensure_case_has_no_local_drafts(case_uid, case_path)?;
        self.ensure_case_has_no_outbound_push(case_uid)?;
        let files = dir_entry_names(&case_path.join("files"), "read case files")?;
        if !files.is_empty() {
            return Err(AppError::new(
                "case_files_present",
                format!("case {case_uid} still has files"),
            )
            .with_hint("Move or delete case files manually before deleting the empty case.")
            .with_details(json!({"case_uid": case_uid, "files": files})));
        }
        let notes_path = case_path.join("notes.md");
        let notes_exists = notes_path.is_file();
        let notes_text = if notes_exists {
            read_to_string(&notes_path, "read case notes")?
        } else {
            String::new()
        };
        let default_notes = new_notes_md(&self.root, self.template_language()?)?;
        let notes_modified =
            !notes_text.trim().is_empty() && notes_text.trim() != default_notes.trim();
        if notes_modified && !allow_notes {
            return Err(AppError::new(
                "case_notes_present",
                format!("case {case_uid} has notes.md content"),
            )
            .with_hint(
                "Inspect the notes, then rerun with `--allow-notes` if deleting them is intended.",
            )
            .with_details(json!({
                "case_uid": case_uid,
                "notes_path": rel_path(&self.root, &notes_path),
                "notes_len_bytes": notes_text.len(),
                "notes_hash": stable_text_hash(&notes_text),
                "suggested_commands": [
                    format!("afmail case notes show {case_uid}"),
                    format!("afmail case delete {case_uid} --allow-notes --reason TEXT"),
                ],
            })));
        }

        let path = rel_path(&self.root, case_path);
        let source_parent = case_path.parent().map(Path::to_path_buf);
        let transaction = self.begin_transaction(result_code, vec![path.clone()])?;
        remove_dir_all(case_path)?;
        if let Some(parent) = source_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        transaction.commit()?;
        self.append_audit_event(
            result_code,
            vec![audit_target("case", case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "case_name": case.collection_name,
                "from_path": path.clone(),
                "archived": archived,
                "notes_deleted": notes_exists,
                "notes_modified": notes_modified,
                "notes_len_bytes": notes_text.len(),
                "notes_hash": stable_text_hash(&notes_text),
            }),
        )?;
        Ok(json!({
            "case_uid": case_uid,
            "case_name": case.collection_name,
            "from_path": path,
            "deleted": true,
            "archived": archived,
            "notes_deleted": notes_exists,
            "notes_modified": notes_modified,
        }))
    }

    fn trash_case_workspace(
        &self,
        case_uid: &str,
        case_path: &Path,
        source_status: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.ensure_case_has_no_local_drafts(case_uid, case_path)?;
        self.ensure_case_has_no_outbound_push(case_uid)?;
        let messages = read_case_messages(case_path, case_uid)?;
        let message_ids = messages.message_ids();
        self.ensure_case_messages_unreferenced_except(case_uid, case_path, &message_ids)?;
        let config = MailConfig::load(&self.root)?;
        let locations = self.message_remote_locations_any(&message_ids)?;
        let mut eligible_message_ids = Vec::new();
        for location in &locations {
            merge_string(&mut eligible_message_ids, &location.message_id);
        }
        let source_path = rel_path(&self.root, case_path);
        let source_parent = case_path.parent().map(Path::to_path_buf);
        let mut transaction_paths = vec![
            source_path.clone(),
            [layout::TRASH, "/data/trash.json"].concat(),
            ".afmail/push".to_string(),
        ];
        for message_id in &message_ids {
            transaction_paths.push(format!("messages/{message_id}.json"));
        }
        let transaction = self.begin_transaction("case_trash", transaction_paths)?;
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            "message.trash",
            &eligible_message_ids,
            &locations,
            &config.actions.message_trash.steps,
            None,
        )?;
        self.update_messages_workspace(&message_ids, MessageStatus::Trashed.as_str())?;
        if let Some(item) = &item {
            self.record_pending_push_item(item)?;
        }
        remove_dir_all(case_path)?;
        if let Some(parent) = source_parent {
            self.remove_empty_case_container_dir(&parent)?;
        }
        self.refresh_messages_after_ref_change(&message_ids)?;
        self.refresh_disposition_views()?;
        transaction.commit()?;
        let push_ids = item
            .as_ref()
            .map(|item| vec![item.push_id.clone()])
            .unwrap_or_default();
        let queued_location_count = if item.is_some() { locations.len() } else { 0 };
        self.append_audit_event(
            "case_trashed",
            vec![audit_target("case", case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "source_status": source_status,
                "from_path": source_path.clone(),
                "message_ids": message_ids.clone(),
                "eligible_message_ids": eligible_message_ids.clone(),
                "push_ids": push_ids.clone(),
            }),
        )?;
        Ok(json!({
            "case_uid": case_uid,
            "source_status": source_status,
            "from_path": source_path,
            "message_ids": message_ids,
            "trashed_message_count": messages.message_count,
            "eligible_message_ids": eligible_message_ids,
            "location_count": locations.len(),
            "queued_location_count": queued_location_count,
            "queued": item.is_some(),
            "push_ids": push_ids,
            "push_id": item.as_ref().map(|item| item.push_id.clone()),
        }))
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
        let active_cases_dir = self.root.join(layout::CASES);
        let archived_cases_dir = self.root.join(layout::ARCHIVED_CASES);
        (dir.parent() == Some(active_cases_dir.as_path()) && dir != active_cases_dir)
            || (dir.parent() == Some(archived_cases_dir.as_path()) && dir != archived_cases_dir)
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
                "archived": false,
                "lifecycle": "active",
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

    pub(super) fn archived_case_items(&self) -> Result<Vec<Value>> {
        Ok(self
            .archived_case_entries()?
            .into_iter()
            .map(|entry| {
                let data = read_case_file(&entry.path).ok();
                let lifecycle = data
                    .as_ref()
                    .map(|case| case_lifecycle(case, true))
                    .unwrap_or("archived");
                json!({
                    "case_uid": entry.case_uid,
                    "case_name": case_name(&entry.path).unwrap_or_default(),
                    "group": entry.group,
                    "archived": true,
                    "lifecycle": lifecycle,
                    "case_dir": entry
                        .path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default(),
                })
            })
            .collect())
    }

    pub(super) fn notes_show(
        &self,
        _code: &str,
        targets: Vec<Value>,
        path: &Path,
    ) -> Result<Value> {
        let text = read_existing_notes(&self.root, path)?;
        Ok(json!({
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
        if let Some(entry) = self.find_archived_case_by_uid(&case_uid)? {
            return Err(case_archived_error(&case_uid, Some(entry.group.as_str())));
        }
        Err(AppError::new(
            "case_not_found",
            format!("case not found: {case_uid}"),
        ))
    }

    pub(super) fn resolve_case_any(&self, case_ref: &str) -> Result<ResolvedCase> {
        let case_uid = parse_case_ref(case_ref)?;
        if let Some(path) = self.find_case_by_uid(&case_uid)? {
            let group = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            return Ok(ResolvedCase {
                case_uid,
                group,
                path,
                archived: false,
            });
        }
        if let Some(entry) = self.find_archived_case_by_uid(&case_uid)? {
            return Ok(ResolvedCase {
                case_uid,
                group: entry.group,
                path: entry.path,
                archived: true,
            });
        }
        Err(AppError::new(
            "case_not_found",
            format!("case not found: {case_uid}"),
        ))
    }

    pub(crate) fn resolve_queued_outbound_case(&self, case_ref: &str) -> Result<(String, PathBuf)> {
        let case = self.resolve_case_any(case_ref)?;
        Ok((case.case_uid, case.path))
    }

    pub(super) fn resolve_unarchive_case(
        &self,
        case_ref: &str,
    ) -> Result<(String, ArchivedCaseEntry, &'static str)> {
        let case_uid = parse_case_ref(case_ref)?;
        self.find_archived_case_by_uid(&case_uid)?
            .map(|entry| (case_uid.clone(), entry, "archived"))
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
        let cases_dir = self.root.join(layout::CASES);
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
                out.push((fm.collection_uid, case_path));
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
        self.case_entries_under(layout::ARCHIVED_CASES, "read archived cases")
    }

    fn case_entries_under(&self, root_dir: &str, context: &str) -> Result<Vec<ArchivedCaseEntry>> {
        let cases_dir = self.root.join(root_dir);
        if !cases_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for group_entry in read_dir(&cases_dir, context)? {
            let group_path = group_entry.path();
            if !group_path.is_dir() {
                continue;
            }
            let group = group_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            for case_entry in read_dir(&group_path, "read archived case group")? {
                let case_path = case_entry.path();
                if !case_path.is_dir() || !case_json_path(&case_path).is_file() {
                    continue;
                }
                let fm = read_case_file(&case_path)?;
                out.push(ArchivedCaseEntry {
                    case_uid: fm.collection_uid,
                    group: group.clone(),
                    path: case_path,
                });
            }
        }
        out.sort_by(|a, b| {
            let a_key = (&a.group, &a.case_uid);
            let b_key = (&b.group, &b.case_uid);
            a_key.cmp(&b_key)
        });
        Ok(out)
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
