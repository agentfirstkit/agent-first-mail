use super::*;

impl Workspace {
    pub fn validate_draft(&self, case_ref: &str, draft_name: &str) -> Result<Value> {
        self.require_workspace()?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let validation = self.validate_draft_inner(&case_uid, draft_name, &case_path)?;
        let now = now_rfc3339();
        let mut draft_state = read_draft_state(&case_path)?;
        let entry = draft_state
            .drafts
            .entry(draft_name.to_string())
            .or_default();
        entry.last_validated_hash = Some(validation.draft_hash.clone());
        entry.last_validated_rfc3339 = Some(now.clone());
        write_draft_state(&case_path, &draft_state)?;
        Ok(json!({
            "code": "draft_valid",
            "case_uid": case_uid,
            "draft_name": draft_name,
            "draft_hash": validation.draft_hash,
            "last_validated_rfc3339": now
        }))
    }

    pub fn attach_file_to_draft(
        &self,
        case_ref: &str,
        draft_name: &str,
        source_path: &str,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_file_name("draft_name", draft_name)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let draft_path = case_path.join("drafts").join(draft_name);
        if !draft_path.is_file() {
            return Err(AppError::new(
                "draft_not_found",
                format!("draft not found: {draft_name}"),
            ));
        }

        let source = resolve_cli_path(source_path)?;
        if !source.is_file() {
            return Err(AppError::new(
                "draft_invalid",
                format!("draft attachment source is not a file: {source_path}"),
            ));
        }
        let source_abs =
            fs::canonicalize(&source).map_err(|e| AppError::io("canonicalize attachment", &e))?;
        let case_abs =
            fs::canonicalize(&case_path).map_err(|e| AppError::io("canonicalize case", &e))?;
        let text = read_to_string(&draft_path, "read draft")?;
        let (mut fm, body) = read_doc::<DraftFrontmatter>(&text)?;

        let (attachment, file_path, copied) = if source_abs.starts_with(&case_abs) {
            let relative = source_abs
                .strip_prefix(&case_abs)
                .map_err(|e| AppError::new("draft_invalid", e.to_string()))?;
            (
                path_to_string(relative),
                rel_path(&self.root, &source_abs),
                false,
            )
        } else {
            let files_dir = case_path.join("files");
            create_dir_all(&files_dir)?;
            let filename = source_abs
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("attachment");
            let saved_filename = safe_attachment_filename(filename, "attachment");
            let candidate_attachment = format!("files/{saved_filename}");
            let already_present = fm
                .attachments
                .iter()
                .any(|item| item == &candidate_attachment);
            let dest = if already_present && files_dir.join(&saved_filename).is_file() {
                files_dir.join(&saved_filename)
            } else {
                let dest = unique_dest_path(&files_dir, &saved_filename);
                fs::copy(&source_abs, &dest)
                    .map_err(|e| AppError::io("copy draft attachment", &e))?;
                dest
            };
            (
                format!("files/{}", path_file_name(&dest)),
                rel_path(&self.root, &dest),
                !already_present,
            )
        };

        let already_present = fm.attachments.iter().any(|item| item == &attachment);
        if !already_present {
            fm.attachments.push(attachment.clone());
            write_string(&draft_path, &render_frontmatter(&fm, &body)?)?;
        }
        let size_bytes = fs::metadata(self.root.join(&file_path))
            .or_else(|_| fs::metadata(&source_abs))
            .map_err(|e| AppError::io("stat draft attachment", &e))?
            .len();
        Ok(json!({
            "code": "draft_attachment_added",
            "case_uid": case_uid,
            "draft_name": draft_name,
            "draft_path": rel_path(&self.root, &draft_path),
            "source_path": path_to_string(&source_abs),
            "attachment": attachment,
            "file_path": file_path,
            "copied": copied,
            "already_present": already_present,
            "size_bytes": size_bytes,
            "requires_validate": true,
        }))
    }

    pub fn remove_draft(
        &self,
        case_ref: &str,
        draft_name: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_file_name("draft_name", draft_name)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let draft_path = case_path.join("drafts").join(draft_name);
        if !draft_path.is_file() {
            return Err(AppError::new(
                "draft_not_found",
                format!("draft not found: {draft_name}"),
            ));
        }
        let removed_push =
            crate::push_queue::remove_outbound_for_draft(&self.root, &case_uid, draft_name)?;
        remove_file(&draft_path)?;
        let mut draft_state = read_draft_state(&case_path)?;
        let state_removed = draft_state.drafts.remove(draft_name).is_some();
        write_draft_state(&case_path, &draft_state)?;
        let push_ids = removed_push
            .iter()
            .map(|item| item.push_id.clone())
            .collect::<Vec<_>>();
        let staged_eml_paths = removed_push
            .iter()
            .filter_map(|item| item.eml_path.clone())
            .collect::<Vec<_>>();
        self.append_audit_event(
            "draft_removed",
            vec![audit_target("case", &case_uid)],
            reason,
            json!({
                "case_uid": case_uid,
                "draft_name": draft_name,
                "draft_path": rel_path(&self.root, &draft_path),
                "push_ids": push_ids.clone(),
                "staged_eml_paths": staged_eml_paths.clone(),
                "state_removed": state_removed,
                "mail_sent": false,
            }),
        )?;
        Ok(json!({
            "code": "draft_removed",
            "case_uid": case_uid,
            "draft_name": draft_name,
            "draft_path": rel_path(&self.root, &draft_path),
            "draft_deleted": true,
            "state_removed": state_removed,
            "queued_removed": !push_ids.is_empty(),
            "removed_push_count": push_ids.len(),
            "push_ids": push_ids,
            "staged_eml_paths": staged_eml_paths,
            "mail_sent": false
        }))
    }

    pub fn compose_draft(&self, case_ref: &str, draft_name: &str) -> Result<Value> {
        self.require_workspace()?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        validate_file_name("draft_name", draft_name)?;
        let draft_path = case_path.join("drafts").join(draft_name);
        let draft_hash = draft_file_hash(&draft_path)?;
        let draft_state = read_draft_state(&case_path)?;
        let entry = draft_state.drafts.get(draft_name).ok_or_else(|| {
            AppError::new(
                "draft_validation_required",
                format!("draft must be validated before compose: {draft_name}"),
            )
            .with_hint(format!(
                "Run `afmail case draft validate {case_uid} {draft_name}` before composing."
            ))
            .with_details(json!({
                "case_uid": case_uid,
                "draft_name": draft_name,
                "suggested_commands": [
                    format!("afmail case draft validate {case_uid} {draft_name}"),
                    format!("afmail case compose {case_uid} {draft_name}")
                ]
            }))
        })?;
        let last_validated_hash = entry.last_validated_hash.as_deref().ok_or_else(|| {
            AppError::new(
                "draft_validation_required",
                format!("draft must be validated before compose: {draft_name}"),
            )
            .with_hint(format!(
                "Run `afmail case draft validate {case_uid} {draft_name}` before composing."
            ))
            .with_details(json!({
                "case_uid": case_uid,
                "draft_name": draft_name,
                "suggested_commands": [
                    format!("afmail case draft validate {case_uid} {draft_name}"),
                    format!("afmail case compose {case_uid} {draft_name}")
                ]
            }))
        })?;
        if last_validated_hash != draft_hash {
            return Err(AppError::new(
                "draft_changed_since_validation",
                format!("draft changed since validation: {draft_name}"),
            )
            .with_hint(format!(
                "Re-run `afmail case draft validate {case_uid} {draft_name}`, then compose again."
            ))
            .with_details(json!({
                "case_uid": case_uid,
                "draft_name": draft_name,
                "suggested_commands": [
                    format!("afmail case draft validate {case_uid} {draft_name}"),
                    format!("afmail case compose {case_uid} {draft_name}")
                ]
            })));
        }
        let config = crate::config::MailConfig::load(&self.root)?;
        let transaction = self.begin_transaction(
            "draft_compose",
            vec![
                rel_path(&self.root, &draft_path),
                rel_path(&self.root, &case_drafts_json_path(&case_path)),
                ".afmail/push".to_string(),
            ],
        )?;
        let mut queued = crate::push_queue::queue_outbound(
            &self.root,
            &case_path,
            &case_uid,
            draft_name,
            &draft_hash,
            &config,
        )?;
        let push_id = queued
            .get("push_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default();
        let now = now_rfc3339();
        let mut draft_state = read_draft_state(&case_path)?;
        let entry = draft_state
            .drafts
            .entry(draft_name.to_string())
            .or_default();
        entry.last_composed_hash = Some(draft_hash.clone());
        entry.last_composed_rfc3339 = Some(now.clone());
        if !push_id.is_empty() {
            entry.push_id = Some(push_id);
        }
        write_draft_state(&case_path, &draft_state)?;
        if let Some(object) = queued.as_object_mut() {
            object.insert("draft_hash".to_string(), json!(draft_hash));
            object.insert("last_composed_rfc3339".to_string(), json!(now));
        }
        transaction.commit()?;
        Ok(queued)
    }

    pub fn reply_to_message(
        &self,
        case_ref: &str,
        message_id: &str,
        reply_all: bool,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        let messages = read_case_messages(&case_messages_json_path(&case_path), &case_uid)?;
        if !messages.message_ids.iter().any(|id| id == message_id) {
            return Err(AppError::new(
                "invalid_request",
                format!("message does not belong to case: {message_id}"),
            ));
        }
        let message = self.read_message_by_id(message_id)?;
        let original_subject = message.subject.as_deref().unwrap_or("");
        let subject = if original_subject
            .trim_start()
            .to_lowercase()
            .starts_with("re:")
        {
            original_subject.to_string()
        } else {
            format!("Re: {original_subject}")
        };
        // Prefer Reply-To over From. With `reply_all`, also carry the original To
        // recipients into `to` and the original Cc into `cc`, excluding self.
        let config = MailConfig::load(&self.root)?;
        let own_email = config
            .smtp
            .from
            .as_deref()
            .or(config.imap.username.as_deref())
            .map(email_address);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        if let Some(own) = &own_email {
            seen.insert(own.clone());
        }
        let mut to: Vec<String> = Vec::new();
        let mut to_sources: Vec<&String> = if message.reply_to.is_empty() {
            message.from.iter().collect()
        } else {
            message.reply_to.iter().collect()
        };
        if reply_all {
            to_sources.extend(message.to.iter());
        }
        for addr in to_sources {
            let key = email_address(addr);
            if !key.is_empty() && seen.insert(key) {
                to.push(addr.clone());
            }
        }
        let mut cc: Vec<String> = Vec::new();
        if reply_all {
            for addr in &message.cc {
                let key = email_address(addr);
                if !key.is_empty() && seen.insert(key) {
                    cc.push(addr.clone());
                }
            }
        }
        let fm = DraftFrontmatter {
            kind: Some("draft".to_string()),
            case_uid: case_uid.to_string(),
            send_intent: Some("reply".to_string()),
            reply_to_message_id: Some(message_id.to_string()),
            subject: Some(subject),
            to,
            cc,
            attachments: Vec::new(),
        };
        let quoted = self.quoted_message_body(&message)?;
        let body = render_draft_reply_body(
            &self.root,
            config.template_language(),
            message.from.as_deref(),
            &quoted,
        )?;
        let draft_name = format!("reply-{message_id}.md");
        let draft_path = case_path.join("drafts").join(&draft_name);
        if draft_path.exists() {
            return Err(AppError::new(
                "draft_exists",
                format!("reply draft already exists: {draft_name}"),
            ));
        }
        create_dir_all(&case_path.join("drafts"))?;
        write_string_new(&draft_path, &render_frontmatter(&fm, &body)?)?;
        Ok(json!({
            "code": "draft_created",
            "case_uid": case_uid,
            "message_id": message_id,
            "draft_name": draft_name,
            "draft_path": rel_path(&self.root, &draft_path)
        }))
    }

    pub fn create_draft(
        &self,
        case_ref: &str,
        to: &[String],
        cc: &[String],
        subject: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let (case_uid, case_path) = self.resolve_active_case(case_ref)?;
        if to.is_empty() {
            return Err(AppError::new(
                "invalid_request",
                "draft requires at least one --to recipient",
            ));
        }
        let fm = DraftFrontmatter {
            kind: Some("draft".to_string()),
            case_uid: case_uid.to_string(),
            send_intent: Some("new".to_string()),
            reply_to_message_id: None,
            subject: subject.map(ToString::to_string),
            to: to.to_vec(),
            cc: cc.to_vec(),
            attachments: Vec::new(),
        };
        let slug = subject
            .map(crate::mail::slugify)
            .filter(|slug| !slug.is_empty())
            .unwrap_or_else(|| "message".to_string());
        let drafts_dir = case_path.join("drafts");
        create_dir_all(&drafts_dir)?;
        let mut draft_name = format!("new-{slug}.md");
        let mut counter = 1;
        while drafts_dir.join(&draft_name).exists() {
            counter += 1;
            draft_name = format!("new-{slug}-{counter}.md");
        }
        let draft_path = drafts_dir.join(&draft_name);
        let language = self.template_language()?;
        let body = render_draft_new_body(&self.root, language)?;
        write_string_new(&draft_path, &render_frontmatter(&fm, &body)?)?;
        Ok(json!({
            "code": "draft_created",
            "case_uid": case_uid,
            "draft_name": draft_name,
            "draft_path": rel_path(&self.root, &draft_path)
        }))
    }

    fn quoted_message_body(&self, message: &MessageFile) -> Result<String> {
        let quoted = message
            .body_text
            .lines()
            .map(|line| {
                if line.is_empty() {
                    ">".to_string()
                } else {
                    format!("> {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(quoted)
    }

    pub fn fetch_message_attachment(
        &self,
        message_id: &str,
        part_id: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let dest = self
            .root
            .join(format!(".afmail/messages/{message_id}.files"));
        match part_id {
            Some(part_id) => {
                let saved = self.fetch_attachment_to(message_id, part_id, &dest)?;
                self.refresh_read_views_after_message_change(message_id)?;
                Ok(saved_attachment_value(
                    &self.root,
                    "attachment_saved",
                    message_id,
                    &saved,
                ))
            }
            None => {
                let message = self.read_message_by_id(message_id)?;
                let mut items = Vec::new();
                for attachment in &message.attachments {
                    let saved = self.fetch_attachment_to(message_id, &attachment.part_id, &dest)?;
                    items.push(saved_attachment_value(
                        &self.root,
                        "attachment_saved",
                        message_id,
                        &saved,
                    ));
                }
                self.refresh_read_views_after_message_change(message_id)?;
                Ok(json!({
                    "code": "attachments_saved",
                    "message_id": message_id,
                    "count": items.len(),
                    "items": items,
                }))
            }
        }
    }

    fn validate_draft_inner(
        &self,
        case_uid: &str,
        draft_name: &str,
        case_path: &Path,
    ) -> Result<DraftValidation> {
        validate_file_name("draft_name", draft_name)?;
        let draft_path = case_path.join("drafts").join(draft_name);
        let draft_bytes = fs::read(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
        let draft_hash = sha256_fingerprint(&draft_bytes);
        let draft = std::str::from_utf8(&draft_bytes)
            .map_err(|e| AppError::new("draft_invalid", format!("draft is not UTF-8: {e}")))?;
        let (fm, _) = read_doc::<DraftFrontmatter>(draft).map_err(|e| {
            AppError::new("draft_invalid", format!("invalid draft frontmatter: {e}"))
        })?;
        if fm.kind.as_deref() != Some("draft") {
            return Err(AppError::new("draft_invalid", "draft kind must be draft"));
        }
        if fm.case_uid != case_uid {
            return Err(AppError::new(
                "draft_invalid",
                "draft case_uid does not match case",
            ));
        }
        if fm.subject.is_none() {
            return Err(AppError::new("draft_invalid", "draft subject is required"));
        }
        if fm.to.is_empty() {
            return Err(AppError::new("draft_invalid", "draft to is required"));
        }
        if let Some(reply_id) = fm.reply_to_message_id.as_ref() {
            let messages = read_case_messages(&case_messages_json_path(case_path), case_uid)?;
            if !messages.message_ids.contains(reply_id) {
                return Err(AppError::new(
                    "draft_invalid",
                    format!("reply_to_message_id does not belong to case: {reply_id}"),
                ));
            }
        }
        for attachment in &fm.attachments {
            let attachment_path = draft_attachment_path(case_path, attachment)?;
            if !attachment_path.is_file() {
                return Err(AppError::new(
                    "draft_invalid",
                    format!("draft attachment does not exist: {attachment}"),
                ));
            }
        }
        Ok(DraftValidation { draft_hash })
    }

    fn fetch_attachment_to(
        &self,
        message_id: &str,
        part_id: &str,
        dest_dir: &Path,
    ) -> Result<SavedAttachment> {
        validate_id("message_id", message_id)?;
        let mut message = self.read_message_by_id(message_id)?;
        let Some(pos) = message
            .attachments
            .iter()
            .position(|a| a.part_id == part_id)
        else {
            return Err(AppError::new(
                "attachment_not_found",
                format!("attachment not found: {message_id} part {part_id}"),
            ));
        };
        let attachment = message.attachments[pos].clone();
        create_dir_all(dest_dir)?;
        if attachment.fetched {
            if let Some(file_path) = attachment.file_path.as_deref() {
                let existing = self.root.join(file_path);
                if existing.is_file() {
                    let size_bytes = fs::metadata(&existing)
                        .map_err(|e| AppError::io("stat attachment", &e))?
                        .len();
                    return Ok(SavedAttachment {
                        part_id: attachment.part_id,
                        filename: attachment.filename,
                        saved_filename: path_file_name(&existing),
                        content_type: attachment.content_type,
                        path: existing,
                        size_bytes,
                    });
                }
            }
        }
        let saved_filename = safe_attachment_filename(&attachment.filename, part_id);
        let dest = unique_dest_path(dest_dir, &saved_filename);
        if let Some(source_path) = attachment.source_path.clone() {
            fs::copy(self.root.join(source_path), &dest)
                .map_err(|e| AppError::io("copy attachment", &e))?;
        } else {
            let eml_path = message
                .eml_path
                .clone()
                .unwrap_or_else(|| format!(".afmail/messages/{message_id}.eml"));
            let raw =
                fs::read(self.root.join(eml_path)).map_err(|e| AppError::io("read eml", &e))?;
            let bytes = crate::mail::attachment_bytes(&raw, part_id)?;
            fs::write(&dest, bytes).map_err(|e| AppError::io("write attachment", &e))?;
        }
        let size_bytes = fs::metadata(&dest)
            .map_err(|e| AppError::io("stat attachment", &e))?
            .len();
        message.attachments[pos].fetched = true;
        message.attachments[pos].file_path = Some(rel_path(&self.root, &dest));
        self.write_message_materialized_cache(&message)?;
        Ok(SavedAttachment {
            part_id: attachment.part_id,
            filename: attachment.filename,
            saved_filename: path_file_name(&dest),
            content_type: attachment.content_type,
            path: dest,
            size_bytes,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct DraftValidation {
    draft_hash: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct DraftStateFile {
    schema_name: String,
    schema_version: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    drafts: BTreeMap<String, DraftStateEntry>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct DraftStateEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    last_validated_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_validated_rfc3339: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_composed_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_composed_rfc3339: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    push_id: Option<String>,
}

#[derive(Debug)]
pub(super) struct SavedAttachment {
    part_id: String,
    filename: String,
    saved_filename: String,
    content_type: String,
    path: PathBuf,
    size_bytes: u64,
}

pub(super) fn saved_attachment_value(
    root: &Path,
    code: &str,
    message_id: &str,
    saved: &SavedAttachment,
) -> Value {
    json!({
        "code": code,
        "message_id": message_id,
        "part_id": saved.part_id.as_str(),
        "filename": saved.filename.as_str(),
        "saved_filename": saved.saved_filename.as_str(),
        "content_type": saved.content_type.as_str(),
        "storage": "message_cache",
        "file_path": rel_path(root, &saved.path),
        "size_bytes": saved.size_bytes,
    })
}

pub(super) fn saved_filename_for_attachment(attachment: &AttachmentRef) -> String {
    attachment
        .file_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| safe_attachment_filename(&attachment.filename, &attachment.part_id))
}

pub(super) fn safe_attachment_filename(filename: &str, part_id: &str) -> String {
    let fallback = format!("part-{part_id}");
    let candidate = filename.trim();
    if candidate.is_empty() {
        return fallback;
    }
    let sanitized = sanitize_with_options(
        candidate,
        SanitizeFilenameOptions {
            windows: true,
            truncate: true,
            replacement: "_",
        },
    );
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        fallback
    } else {
        sanitized.to_string()
    }
}

pub(super) fn is_image_content_type(content_type: &str) -> bool {
    content_type
        .split_once(';')
        .map(|(mime, _)| mime)
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
        .starts_with("image/")
}

pub(super) fn attachment_markdown_path(
    root: Option<&Path>,
    output_dir: Option<&Path>,
    file_path: &str,
) -> String {
    let Some(root) = root else {
        return file_path.to_string();
    };
    let Some(output_dir) = output_dir else {
        return file_path.to_string();
    };
    let Ok(from) = output_dir.strip_prefix(root) else {
        return file_path.to_string();
    };
    let up_count = from
        .components()
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .count();
    let mut parts = Vec::new();
    parts.extend(std::iter::repeat_n("..", up_count));
    parts.extend(file_path.split('/').filter(|part| !part.is_empty()));
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

pub(super) fn render_draft_new_body(root: &Path, language: TemplateLanguage) -> Result<String> {
    render_template(
        root,
        language,
        TemplateKey::DraftNew,
        &json!({"language": language.as_str()}),
    )
}

pub(super) fn render_draft_reply_body(
    root: &Path,
    language: TemplateLanguage,
    sender: Option<&str>,
    quoted: &str,
) -> Result<String> {
    render_template(
        root,
        language,
        TemplateKey::DraftReply,
        &json!({
            "language": language.as_str(),
            "sender": sender.unwrap_or(""),
            "quoted": quoted,
        }),
    )
}

pub(super) fn read_draft_state(case_path: &Path) -> Result<DraftStateFile> {
    let path = case_drafts_json_path(case_path);
    if !path.exists() {
        return Ok(DraftStateFile {
            schema_name: "draft_state".to_string(),
            schema_version: 1,
            drafts: BTreeMap::new(),
        });
    }
    let data = read_to_string(&path, "read draft state")?;
    let state: DraftStateFile =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse draft state", &e))?;
    if state.schema_name != "draft_state" || state.schema_version != 1 {
        return Err(AppError::new(
            "draft_state_invalid",
            format!("invalid draft state schema: {}", path_to_string(&path)),
        ));
    }
    Ok(state)
}

pub(super) fn write_draft_state(case_path: &Path, state: &DraftStateFile) -> Result<()> {
    let mut normalized = state.clone();
    normalized.schema_name = "draft_state".to_string();
    normalized.schema_version = 1;
    write_json_pretty(&case_drafts_json_path(case_path), &normalized)
}

pub(super) fn draft_file_hash(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|e| AppError::io("read draft", &e))?;
    Ok(sha256_fingerprint(&bytes))
}

pub(super) fn resolve_cli_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .map_err(|e| AppError::io("current dir", &e))?
        .join(path))
}

pub(super) fn draft_attachment_path(case_path: &Path, attachment: &str) -> Result<PathBuf> {
    let path = Path::new(attachment);
    if attachment.trim().is_empty() || path.is_absolute() {
        return Err(AppError::new(
            "draft_invalid",
            format!("invalid draft attachment path: {attachment}"),
        ));
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => safe.push(part),
            _ => {
                return Err(AppError::new(
                    "draft_invalid",
                    format!("invalid draft attachment path: {attachment}"),
                ))
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err(AppError::new(
            "draft_invalid",
            format!("invalid draft attachment path: {attachment}"),
        ));
    }
    Ok(case_path.join(safe))
}
