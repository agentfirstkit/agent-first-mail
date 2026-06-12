use super::*;

impl Workspace {
    /// List untriaged messages by stable locator. Resolve details with
    /// path_templates or `afmail message show`.
    pub fn triage_list(&self) -> Result<Value> {
        self.require_workspace()?;
        let triage_dir = self.root.join("triage");
        let mut items = Vec::new();
        if triage_dir.exists() {
            let mut paths: Vec<PathBuf> = read_dir(&triage_dir, "read triage directory")?
                .into_iter()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("md"))
                .collect();
            paths.sort();
            for path in paths {
                let message_id = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default();
                items.push(json!({"message_id": message_id}));
            }
        }
        let push_status = serde_json::to_value(crate::push_queue::push_status(&self.root)?)
            .map_err(|e| AppError::json("serialize push status", &e))?;
        Ok(json!({
            "code": "triage_list",
            "count": items.len(),
            "push_status": push_status,
            "path_templates": {
                "view_path": "triage/{message_id}.md",
                "json_path": "messages/{message_id}.json",
            },
            "items": items,
        }))
    }

    pub fn refresh_triage_views(&self) -> Result<Value> {
        self.require_workspace()?;
        create_dir_all(&self.root.join("triage"))?;
        let cases = CaseIndex::build(self)?;
        let mut desired = BTreeSet::new();
        let mut written_count = 0usize;
        for path in message_json_paths(&self.root)? {
            let mut message = read_message(&path)?;
            let status = self.derived_message_status(&message, &cases)?;
            if message.workspace.status != status {
                message.workspace.status = status;
                message.workspace.remote_sync = None;
                self.write_message_cache(&message)?;
            }
            if self.triage_candidate(&message, &cases)? {
                desired.insert(message.message_id.clone());
                self.write_triage_view(&message)?;
                written_count += 1;
            }
        }
        let stale_count = self.remove_stale_triage_views(&desired)?;
        Ok(json!({
            "code": "triage_refreshed",
            "triage_count": desired.len(),
            "triage_written_count": written_count,
            "stale_triage_removed_count": stale_count
        }))
    }
}

pub(crate) fn render_triage_view(
    root: &Path,
    language: TemplateLanguage,
    message: &MessageFile,
    conversation: &str,
    suggested_case_uids: Vec<String>,
    suggested_reason: Option<String>,
    related_messages: Vec<Value>,
) -> Result<String> {
    let generated_rfc3339 = now_rfc3339();
    render_template(
        root,
        language,
        TemplateKey::TriageView,
        &json!({
            "language": language.as_str(),
            "message_id": message.message_id.as_str(),
            "title": message.subject.as_deref().unwrap_or(""),
            "generated_rfc3339": generated_rfc3339,
            "attachment_count": message.attachments.len(),
            "suggested_case_uids": suggested_case_uids,
            "suggested_reason": suggested_reason.as_deref().unwrap_or(""),
            "suggested_reason_yaml": suggested_reason
                .as_deref()
                .map(yaml_double_quote)
                .unwrap_or_default(),
            "related_messages": related_messages,
            "conversation": conversation.trim(),
            "message": message_template_value(message)?,
        }),
    )
}

pub(super) fn yaml_double_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

impl Workspace {
    pub(super) fn refresh_all_case_message_views(&self) -> Result<usize> {
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        let mut count = 0usize;
        for (_, case_path) in self.all_case_entries()? {
            self.refresh_case_message_views_with_renderer(&case_path, &mut renderer)?;
            count += 1;
        }
        Ok(count)
    }

    pub(super) fn refresh_case_message_views(&self, case_path: &Path) -> Result<()> {
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        self.refresh_case_message_views_with_renderer(case_path, &mut renderer)?;
        Ok(())
    }

    pub(super) fn refresh_case_message_views_with_renderer(
        &self,
        case_path: &Path,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<CaseViewRefresh> {
        if !case_json_path(case_path).is_file() {
            return Ok(CaseViewRefresh::default());
        }
        let case_fm = read_case_file(case_path)?;
        let case_uid = case_fm.case_uid.clone();
        let case_messages = read_case_messages(&case_messages_json_path(case_path), &case_uid)?;
        let messages_dir = case_views_messages_dir(case_path);
        create_dir_all(&messages_dir)?;
        let config = MailConfig::load(&self.root)?;
        let mut desired = BTreeSet::new();
        let mut case_conversation = Vec::new();
        let mut messages = Vec::new();
        let mut message_count = 0usize;
        for message_id in &case_messages.message_ids {
            let message = self.read_message_by_id(message_id)?;
            desired.insert(message_id.clone());
            case_conversation.push(self.message_conversation_with_renderer(
                &message,
                &config,
                renderer,
                Some(case_path),
            )?);
            let view_path = case_message_view_path(case_path, message_id);
            let view = self.render_case_message_view(
                &case_uid,
                &case_fm.case_name,
                &message,
                renderer,
                view_path.parent(),
            )?;
            write_string(&view_path, &view)?;
            messages.push(message);
            message_count += 1;
        }
        self.remove_stale_case_message_views(&messages_dir, &desired)?;
        let case_doc = self.render_case_document(
            &case_fm,
            &messages,
            &case_conversation.join("\n\n"),
            &config,
            renderer,
        )?;
        write_string(&case_path.join("case.md"), &case_doc)?;
        Ok(CaseViewRefresh {
            case_index_count: 1,
            case_message_count: message_count,
        })
    }

    pub(super) fn render_case_message_view(
        &self,
        case_uid: &str,
        case_name: &str,
        message: &MessageFile,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        output_dir: Option<&Path>,
    ) -> Result<String> {
        let config = MailConfig::load(&self.root)?;
        let title = message.subject.as_deref().unwrap_or("");
        let generated_rfc3339 = now_rfc3339();
        let message_value = message_template_value(message)?;
        let conversation =
            self.message_conversation_with_renderer(message, &config, renderer, output_dir)?;
        let context = json!({
            "frontmatter": {
                "kind": "case_message",
                "case_uid": case_uid,
                "case_name": case_name,
                "message_id": message.message_id.as_str(),
                "generated_rfc3339": generated_rfc3339.as_str(),
            },
            "language": config.resolved_language_bcp47(),
            "case_uid": case_uid,
            "case_name": case_name,
            "message_id": message.message_id.as_str(),
            "title": title,
            "generated_rfc3339": generated_rfc3339.as_str(),
            "conversation": conversation.trim(),
            "message": message_value,
        });
        renderer.render(TemplateKey::CaseMessage, &context)
    }

    pub(super) fn render_case_document(
        &self,
        case_fm: &CaseFrontmatter,
        messages: &[MessageFile],
        conversation: &str,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<String> {
        let case_uid = case_fm.case_uid.as_str();
        let mut case_view = case_fm.clone();
        case_view.message_count = messages.len();
        case_view.attachment_count = messages
            .iter()
            .map(|message| message.attachments.len())
            .sum::<usize>();
        case_view.last_message_rfc3339 = messages
            .iter()
            .filter_map(message_time)
            .max_by(|a, b| compare_rfc3339_asc(a, b));
        let mut sorted = messages.to_vec();
        sorted.sort_by(compare_message_time_asc);
        let mut items = Vec::new();
        let offset = config.resolved_timezone_offset();
        for message in &sorted {
            let mut fields = Vec::new();
            let display_time = message_time_datetime(message, &offset).unwrap_or_default();
            let display_from = message
                .from
                .as_deref()
                .map(markdown_inline)
                .unwrap_or_default();
            let display_to = markdown_inline(&message.to.join(", "));
            let display_subject = message
                .subject
                .as_deref()
                .map(markdown_inline)
                .unwrap_or_default();
            let display_status = markdown_inline(&message.workspace.status);
            if !display_time.is_empty() {
                fields.push(json!({"kind": "time", "value": display_time.as_str()}));
            }
            if !display_from.is_empty() {
                fields.push(json!({"kind": "from", "value": display_from.as_str()}));
            }
            if !display_to.is_empty() {
                fields.push(json!({"kind": "to", "value": display_to.as_str()}));
            }
            if !display_subject.is_empty() {
                fields.push(json!({"kind": "subject", "value": display_subject.as_str()}));
            }
            if !display_status.is_empty() {
                fields.push(json!({"kind": "status", "value": display_status.as_str()}));
            }
            let title = message
                .subject
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(message.message_id.as_str())
                .to_string();
            let mut item = thread_item_common(
                message,
                &offset,
                config.template_language(),
                format!("views/messages/{}.md", message.message_id),
                title,
            )?;
            if let Value::Object(map) = &mut item {
                map.insert("fields".to_string(), json!(fields));
            }
            items.push(item);
        }
        let case_value = serde_json::to_value(&case_view)
            .map_err(|e| AppError::json("serialize case frontmatter", &e))?;
        let generated_rfc3339 = now_rfc3339();
        let messages_value = sorted
            .iter()
            .map(message_template_value)
            .collect::<Result<Vec<_>>>()?;
        let context = json!({
            "frontmatter": {
                "kind": "case_index",
                "case_uid": case_uid,
                "case_name": case_fm.case_name.as_str(),
                "generated_rfc3339": generated_rfc3339.as_str(),
                "message_count": items.len(),
            },
            "language": config.resolved_language_bcp47(),
            "case_uid": case_uid,
            "case_name": case_fm.case_name.as_str(),
            "title": case_fm.case_name.as_str(),
            "status": case_fm.status.as_str(),
            "message_count": items.len(),
            "generated_rfc3339": generated_rfc3339.as_str(),
            "case": case_value,
            "items": items,
            "messages": messages_value,
            "conversation": conversation.trim(),
        });
        renderer.render(TemplateKey::CaseDocument, &context)
    }

    pub(super) fn remove_stale_case_message_views(
        &self,
        messages_dir: &Path,
        desired: &BTreeSet<String>,
    ) -> Result<()> {
        for entry in read_dir(messages_dir, "read case message views")? {
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
        Ok(())
    }

    pub(super) fn derived_message_status(
        &self,
        message: &MessageFile,
        cases: &CaseIndex,
    ) -> Result<String> {
        let current = MessageStatus::parse(&message.workspace.status)?;
        if current.is_terminal_local() {
            return Ok(current.as_str().to_string());
        }
        if message.workspace.archive_uid.is_some() || current == MessageStatus::Archived {
            return Ok(MessageStatus::Archived.as_str().to_string());
        }
        if cases.has_active_reference(&message.message_id) {
            return Ok(MessageStatus::Case.as_str().to_string());
        }
        if message.workspace.origin.is_some() {
            return Ok(MessageStatus::Archived.as_str().to_string());
        }
        if cases.has_any_reference(&message.message_id) {
            return Ok(MessageStatus::Case.as_str().to_string());
        }
        Ok(MessageStatus::Triage.as_str().to_string())
    }

    pub(super) fn triage_candidate(
        &self,
        message: &MessageFile,
        cases: &CaseIndex,
    ) -> Result<bool> {
        let current = MessageStatus::parse(&message.workspace.status)?;
        if current.is_terminal_local() {
            return Ok(false);
        }
        if message.workspace.archive_uid.is_some() || current == MessageStatus::Archived {
            return Ok(false);
        }
        if message.workspace.origin.is_some() {
            return Ok(false);
        }
        Ok(!cases.has_any_reference(&message.message_id))
    }

    pub(super) fn write_triage_view(&self, message: &MessageFile) -> Result<()> {
        let path = self
            .root
            .join("triage")
            .join(format!("{}.md", message.message_id));
        let conversation = self.message_conversation_for_dir(message, path.parent())?;
        let (suggested_case_uids, suggested_reason) = existing_triage_suggestion(&path)?;
        let related_message_ids = self.related_message_ids(&message.message_id)?;
        let related_messages = self.related_message_rows(&related_message_ids)?;
        let config = MailConfig::load(&self.root)?;
        let rendered = render_triage_view(
            &self.root,
            config.template_language(),
            message,
            &conversation,
            suggested_case_uids,
            suggested_reason,
            related_messages,
        )?;
        write_string(&path, &rendered)
    }

    pub(super) fn related_message_rows(
        &self,
        related_message_ids: &[String],
    ) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for related_message_id in related_message_ids {
            let related = self.read_message_by_id(related_message_id)?;
            let time = related
                .received_rfc3339
                .as_deref()
                .or(related.sent_rfc3339.as_deref())
                .unwrap_or("");
            out.push(json!({
                "message_id": related.message_id,
                "direction": markdown_table_cell(related.direction.as_deref().unwrap_or("")),
                "from": markdown_table_cell(related.from.as_deref().unwrap_or("")),
                "subject": markdown_table_cell(related.subject.as_deref().unwrap_or("")),
                "time": markdown_table_cell(time),
                "status": markdown_table_cell(&related.workspace.status),
                "message": message_template_value(&related)?,
            }));
        }
        Ok(out)
    }

    pub(super) fn remove_stale_triage_views(&self, desired: &BTreeSet<String>) -> Result<usize> {
        let triage_dir = self.root.join("triage");
        if !triage_dir.exists() {
            return Ok(0);
        }
        let mut removed = 0usize;
        for entry in read_dir(&triage_dir, "read triage directory")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !desired.contains(stem) {
                remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}
