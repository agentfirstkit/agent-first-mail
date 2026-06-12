use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct DispositionViewSpec {
    status: MessageStatus,
    dir: &'static str,
    title_en: &'static str,
    title_zh: &'static str,
    label_en: &'static str,
    label_zh: &'static str,
    frontmatter_kind: &'static str,
}

const DISPOSITION_VIEW_SPECS: [DispositionViewSpec; 3] = [
    DispositionViewSpec {
        status: MessageStatus::Spam,
        dir: "spam",
        title_en: "Spam",
        title_zh: "垃圾邮件",
        label_en: "Spam",
        label_zh: "垃圾邮件",
        frontmatter_kind: "spam_message",
    },
    DispositionViewSpec {
        status: MessageStatus::Trashed,
        dir: "trash",
        title_en: "Trash",
        title_zh: "废弃邮件",
        label_en: "Trash",
        label_zh: "废弃邮件",
        frontmatter_kind: "trash_message",
    },
    DispositionViewSpec {
        status: MessageStatus::DeletedRemote,
        dir: "deleted",
        title_en: "Remote Deleted",
        title_zh: "远端已删除",
        label_en: "Remote deleted",
        label_zh: "远端已删除",
        frontmatter_kind: "deleted_message",
    },
];

impl DispositionViewSpec {
    fn title(self, language: TemplateLanguage) -> &'static str {
        match language {
            TemplateLanguage::EnUs => self.title_en,
            TemplateLanguage::ZhCn => self.title_zh,
        }
    }

    fn label(self, language: TemplateLanguage) -> &'static str {
        match language {
            TemplateLanguage::EnUs => self.label_en,
            TemplateLanguage::ZhCn => self.label_zh,
        }
    }
}

impl Workspace {
    pub(super) fn refresh_disposition_views(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = MailConfig::load(&self.root)?;
        let language = config.template_language();
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        let mut groups: BTreeMap<&'static str, Vec<MessageFile>> = BTreeMap::new();
        let mut desired: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
        let mut written: BTreeMap<&'static str, usize> = BTreeMap::new();

        for spec in DISPOSITION_VIEW_SPECS {
            create_dir_all(&self.root.join(spec.dir))?;
            groups.insert(spec.dir, Vec::new());
            desired.insert(spec.dir, BTreeSet::new());
            written.insert(spec.dir, 0);
        }

        for path in message_json_paths(&self.root)? {
            let message = read_message(&path)?;
            let status = MessageStatus::parse(&message.workspace.status)?;
            let Some(spec) = disposition_spec_for_status(status) else {
                continue;
            };
            desired
                .entry(spec.dir)
                .or_default()
                .insert(message.message_id.clone());
            self.write_disposition_message_view_with_renderer(
                spec,
                &message,
                &config,
                &mut renderer,
            )?;
            *written.entry(spec.dir).or_default() += 1;
            groups.entry(spec.dir).or_default().push(message);
        }

        let mut stale_spam_removed_count = 0usize;
        let mut stale_trash_removed_count = 0usize;
        let mut stale_deleted_removed_count = 0usize;
        let mut index_written_count = 0usize;
        for spec in DISPOSITION_VIEW_SPECS {
            let desired = desired.get(spec.dir).cloned().unwrap_or_default();
            let stale_count = self.remove_stale_disposition_message_views(spec, &desired)?;
            match spec.status {
                MessageStatus::Spam => stale_spam_removed_count = stale_count,
                MessageStatus::Trashed => stale_trash_removed_count = stale_count,
                MessageStatus::DeletedRemote => stale_deleted_removed_count = stale_count,
                _ => {}
            }
            let mut messages = groups.remove(spec.dir).unwrap_or_default();
            self.write_disposition_index_with_renderer(
                spec,
                &mut messages,
                &config,
                &mut renderer,
            )?;
            index_written_count += 1;
        }

        let spam_written_count = written.get("spam").copied().unwrap_or_default();
        let trash_written_count = written.get("trash").copied().unwrap_or_default();
        let deleted_written_count = written.get("deleted").copied().unwrap_or_default();
        Ok(json!({
            "code": "disposition_views_refreshed",
            "spam_count": spam_written_count,
            "spam_written_count": spam_written_count,
            "stale_spam_removed_count": stale_spam_removed_count,
            "trash_count": trash_written_count,
            "trash_written_count": trash_written_count,
            "stale_trash_removed_count": stale_trash_removed_count,
            "deleted_count": deleted_written_count,
            "deleted_written_count": deleted_written_count,
            "stale_deleted_removed_count": stale_deleted_removed_count,
            "index_written_count": index_written_count,
            "message_written_count": spam_written_count + trash_written_count + deleted_written_count,
        }))
    }

    pub(super) fn write_disposition_message_view_with_renderer(
        &self,
        spec: DispositionViewSpec,
        message: &MessageFile,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<()> {
        let path = self.disposition_message_view_path(spec, &message.message_id);
        let generated_rfc3339 = now_rfc3339();
        let conversation =
            self.message_conversation_with_renderer(message, config, renderer, path.parent())?;
        let context = json!({
            "frontmatter_kind": spec.frontmatter_kind,
            "language": config.resolved_language_bcp47(),
            "message_id": message.message_id.as_str(),
            "status": message.workspace.status.as_str(),
            "status_label": spec.label(config.template_language()),
            "title": message.subject.as_deref().unwrap_or(""),
            "generated_rfc3339": generated_rfc3339.as_str(),
            "conversation": conversation.trim(),
            "message": message_template_value(message)?,
        });
        let rendered = renderer.render(TemplateKey::StatusMessage, &context)?;
        write_string(&path, &rendered)
    }

    pub(super) fn write_disposition_index_with_renderer(
        &self,
        spec: DispositionViewSpec,
        messages: &mut [MessageFile],
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<()> {
        messages.sort_by(|a, b| compare_message_time_asc(b, a));
        let offset = config.resolved_timezone_offset();
        let language = config.template_language();
        let items = messages
            .iter()
            .map(|message| {
                let title = message
                    .subject
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(message.message_id.as_str())
                    .to_string();
                let mut item = thread_item_common(
                    message,
                    &offset,
                    language,
                    format!("{}.md", message.message_id),
                    title,
                )?;
                if let Value::Object(map) = &mut item {
                    map.insert("message".to_string(), message_template_value(message)?);
                }
                Ok(item)
            })
            .collect::<Result<Vec<_>>>()?;
        let generated_rfc3339 = now_rfc3339();
        let context = json!({
            "frontmatter_kind": format!("{}_index", spec.frontmatter_kind),
            "language": config.resolved_language_bcp47(),
            "status": spec.status.as_str(),
            "status_label": spec.label(language),
            "status_dir": spec.dir,
            "title": spec.title(language),
            "generated_rfc3339": generated_rfc3339.as_str(),
            "message_count": items.len(),
            "items": items,
        });
        let rendered = renderer.render(TemplateKey::StatusIndex, &context)?;
        write_string(&self.root.join(spec.dir).join("index.md"), &rendered)
    }

    pub(super) fn remove_stale_disposition_message_views(
        &self,
        spec: DispositionViewSpec,
        desired: &BTreeSet<String>,
    ) -> Result<usize> {
        let dir = self.root.join(spec.dir);
        if !dir.exists() {
            return Ok(0);
        }
        let mut removed = 0usize;
        for entry in read_dir(&dir, "read disposition view directory")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem == "index" {
                continue;
            }
            if !desired.contains(stem) {
                remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub(super) fn disposition_message_view_path(
        &self,
        spec: DispositionViewSpec,
        message_id: &str,
    ) -> PathBuf {
        self.root.join(spec.dir).join(format!("{message_id}.md"))
    }
}

pub(super) fn disposition_spec_for_status(status: MessageStatus) -> Option<DispositionViewSpec> {
    DISPOSITION_VIEW_SPECS
        .iter()
        .copied()
        .find(|spec| spec.status == status)
}
