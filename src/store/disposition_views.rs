use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct DispositionViewSpec {
    status: MessageStatus,
    dir: &'static str,
    data_file: &'static str,
    schema_name: &'static str,
    collection_uid: &'static str,
    // English title only — used for the collection's stored (data) title. The
    // human-facing localized title/label shown in the Markdown view is rendered
    // by the per-language template from `view.status`, not from here.
    title_en: &'static str,
    frontmatter_kind: &'static str,
}

const DISPOSITION_VIEW_SPECS: [DispositionViewSpec; 3] = [
    DispositionViewSpec {
        status: MessageStatus::Spam,
        dir: "spam",
        data_file: "spam.json",
        schema_name: "spam",
        collection_uid: "spam",
        title_en: "Spam",
        frontmatter_kind: "spam_message",
    },
    DispositionViewSpec {
        status: MessageStatus::Trashed,
        dir: "trash",
        data_file: "trash.json",
        schema_name: "trash",
        collection_uid: "trash",
        title_en: "Trash",
        frontmatter_kind: "trash_message",
    },
    DispositionViewSpec {
        status: MessageStatus::DeletedRemote,
        dir: "deleted",
        data_file: "deleted.json",
        schema_name: "deleted_remote",
        collection_uid: "deleted",
        title_en: "Remote Deleted",
        frontmatter_kind: "deleted_message",
    },
];

impl DispositionViewSpec {
    fn empty_collection(self) -> MessageCollection {
        let now = now_rfc3339();
        let mut collection =
            MessageCollection::new(self.schema_name, self.collection_uid, self.title_en);
        collection.status = self.status.as_str().to_string();
        collection.created_rfc3339 = Some(now.clone());
        collection.updated_rfc3339 = Some(now);
        collection
    }
}

impl Workspace {
    pub(super) fn read_disposition_messages(
        &self,
        spec: DispositionViewSpec,
    ) -> Result<MessageCollection> {
        let path = self.disposition_json_path(spec);
        if !path.exists() {
            return Ok(spec.empty_collection());
        }
        let data = read_to_string(&path, "read disposition messages")?;
        let messages: MessageCollection = serde_json::from_str(&data)
            .map_err(|e| AppError::json("parse disposition messages", &e))?;
        if messages.schema_name != spec.schema_name
            || messages.schema_version != MESSAGE_COLLECTION_SCHEMA_VERSION
            || messages.collection_uid != spec.collection_uid
        {
            return Err(AppError::new(
                "disposition_messages_invalid",
                format!(
                    "invalid disposition messages schema: {}",
                    rel_path(&self.root, &path)
                ),
            ));
        }
        Ok(messages)
    }

    pub(super) fn write_disposition_messages(
        &self,
        spec: DispositionViewSpec,
        data: &MessageCollection,
    ) -> Result<()> {
        let mut normalized = data.clone();
        normalized.normalize(spec.schema_name, spec.collection_uid, spec.title_en);
        normalized.status = spec.status.as_str().to_string();
        if normalized.created_rfc3339.is_none() {
            normalized.created_rfc3339 = Some(now_rfc3339());
        }
        if normalized.updated_rfc3339.is_none() {
            normalized.updated_rfc3339 = Some(now_rfc3339());
        }
        let path = self.disposition_json_path(spec);
        if normalized.items.is_empty() && !path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        write_json_pretty(&path, &normalized)
    }

    pub(crate) fn set_message_disposition(
        &self,
        status: MessageStatus,
        message_id: &str,
        summary: Option<&str>,
        added_rfc3339: &str,
    ) -> Result<()> {
        let spec = disposition_spec_for_status(status).ok_or_else(|| {
            AppError::new(
                "invalid_request",
                format!(
                    "message status is not a local disposition: {}",
                    status.as_str()
                ),
            )
        })?;
        self.clear_message_from_all_dispositions_except(message_id, Some(spec))?;
        let mut data = self.read_disposition_messages(spec)?;
        data.upsert_item(message_id, summary, added_rfc3339);
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_disposition_messages(spec, &data)
    }

    pub(super) fn clear_message_disposition(
        &self,
        status: MessageStatus,
        message_id: &str,
    ) -> Result<bool> {
        let Some(spec) = disposition_spec_for_status(status) else {
            return Ok(false);
        };
        self.clear_message_disposition_for_spec(spec, message_id)
    }

    pub(super) fn clear_message_from_all_dispositions(&self, message_id: &str) -> Result<()> {
        self.clear_message_from_all_dispositions_except(message_id, None)
    }

    fn clear_message_from_all_dispositions_except(
        &self,
        message_id: &str,
        keep: Option<DispositionViewSpec>,
    ) -> Result<()> {
        for spec in DISPOSITION_VIEW_SPECS {
            if keep.is_some_and(|keep| keep.dir == spec.dir) {
                continue;
            }
            self.clear_message_disposition_for_spec(spec, message_id)?;
        }
        Ok(())
    }

    fn clear_message_disposition_for_spec(
        &self,
        spec: DispositionViewSpec,
        message_id: &str,
    ) -> Result<bool> {
        let mut data = self.read_disposition_messages(spec)?;
        let before = data.items.len();
        data.items.retain(|item| item.message_id != message_id);
        if data.items.len() == before {
            return Ok(false);
        }
        data.message_count = data.items.len();
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_disposition_messages(spec, &data)?;
        Ok(true)
    }

    pub(super) fn disposition_state_for_message(
        &self,
        message_id: &str,
    ) -> Result<Option<(MessageStatus, String)>> {
        for spec in DISPOSITION_VIEW_SPECS {
            let data = self.read_disposition_messages(spec)?;
            if let Some(item) = data.items.iter().find(|item| item.message_id == message_id) {
                return Ok(Some((spec.status, item.added_rfc3339.clone())));
            }
        }
        Ok(None)
    }

    pub(super) fn disposition_state_by_message(&self) -> Result<BTreeMap<String, MessageStatus>> {
        let mut out = BTreeMap::new();
        for spec in DISPOSITION_VIEW_SPECS {
            let data = self.read_disposition_messages(spec)?;
            for item in data.items {
                out.insert(item.message_id, spec.status);
            }
        }
        Ok(out)
    }

    pub(super) fn refresh_disposition_views(&self) -> Result<Value> {
        self.refresh_disposition_views_inner(None)
    }

    pub(super) fn refresh_disposition_views_cached(
        &self,
        cache: &mut RenderCache,
    ) -> Result<Value> {
        self.refresh_disposition_views_inner(Some(cache))
    }

    fn refresh_disposition_views_inner(
        &self,
        mut cache: Option<&mut RenderCache>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let config = MailConfig::load(&self.root)?;
        let language = config.template_language();
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        let mut groups: BTreeMap<&'static str, Vec<MessageFile>> = BTreeMap::new();
        let mut desired: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
        let mut written: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut unchanged: BTreeMap<&'static str, usize> = BTreeMap::new();

        for spec in DISPOSITION_VIEW_SPECS {
            groups.insert(spec.dir, Vec::new());
            desired.insert(spec.dir, BTreeSet::new());
            written.insert(spec.dir, 0);
            unchanged.insert(spec.dir, 0);
        }

        for spec in DISPOSITION_VIEW_SPECS {
            let data = self.read_disposition_messages(spec)?;
            for item in data.items {
                let mut message = self.read_message_by_id(&item.message_id)?;
                message.workspace.status = spec.status.as_str().to_string();
                message.workspace.archive_uid = None;
                message.workspace.archived_rfc3339 = None;
                message.workspace.origin = None;
                self.apply_identity_match(&mut message, &config)?;
                desired
                    .entry(spec.dir)
                    .or_default()
                    .insert(message.message_id.clone());
                let outcome = self.write_disposition_message_view_inner(
                    spec,
                    &message,
                    &config,
                    &mut renderer,
                    cache.as_deref_mut(),
                )?;
                match outcome {
                    FileWriteOutcome::Written => *written.entry(spec.dir).or_default() += 1,
                    FileWriteOutcome::Unchanged => *unchanged.entry(spec.dir).or_default() += 1,
                }
                groups.entry(spec.dir).or_default().push(message);
            }
        }

        let mut stale_spam_removed_count = 0usize;
        let mut stale_trash_removed_count = 0usize;
        let mut stale_deleted_removed_count = 0usize;
        let mut index_written_count = 0usize;
        let mut index_unchanged_count = 0usize;
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
            if messages.is_empty() {
                continue;
            }
            let outcome = self.write_disposition_index_inner(
                spec,
                &mut messages,
                &config,
                &mut renderer,
                cache.as_deref_mut(),
            )?;
            match outcome {
                FileWriteOutcome::Written => index_written_count += 1,
                FileWriteOutcome::Unchanged => index_unchanged_count += 1,
            }
        }

        let spam_written_count = written.get("spam").copied().unwrap_or_default();
        let trash_written_count = written.get("trash").copied().unwrap_or_default();
        let deleted_written_count = written.get("deleted").copied().unwrap_or_default();
        let spam_unchanged_count = unchanged.get("spam").copied().unwrap_or_default();
        let trash_unchanged_count = unchanged.get("trash").copied().unwrap_or_default();
        let deleted_unchanged_count = unchanged.get("deleted").copied().unwrap_or_default();
        Ok(json!({
            "code": "disposition_views_refreshed",
            "spam_count": spam_written_count + spam_unchanged_count,
            "spam_written_count": spam_written_count,
            "spam_unchanged_count": spam_unchanged_count,
            "stale_spam_removed_count": stale_spam_removed_count,
            "trash_count": trash_written_count + trash_unchanged_count,
            "trash_written_count": trash_written_count,
            "trash_unchanged_count": trash_unchanged_count,
            "stale_trash_removed_count": stale_trash_removed_count,
            "deleted_count": deleted_written_count + deleted_unchanged_count,
            "deleted_written_count": deleted_written_count,
            "deleted_unchanged_count": deleted_unchanged_count,
            "stale_deleted_removed_count": stale_deleted_removed_count,
            "index_written_count": index_written_count,
            "index_unchanged_count": index_unchanged_count,
            "message_written_count": spam_written_count + trash_written_count + deleted_written_count,
            "message_unchanged_count": spam_unchanged_count + trash_unchanged_count + deleted_unchanged_count,
        }))
    }

    fn write_disposition_message_view_inner(
        &self,
        spec: DispositionViewSpec,
        message: &MessageFile,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        cache: Option<&mut RenderCache>,
    ) -> Result<FileWriteOutcome> {
        let path = self.disposition_message_view_path(spec, &message.message_id);
        let context = json!({
            "frontmatter": {
                "kind": spec.frontmatter_kind,
                "message_id": message.message_id.as_str(),
                "status": message.workspace.status.as_str(),
            },
            "message": message_template_value(message)?,
            "view": {
                "language": config.resolved_language_bcp47(),
                "status": message.workspace.status.as_str(),
                "title": message.subject.as_deref().unwrap_or(""),
            },
        });
        if let Some(cache) = cache {
            let input_hash = self.render_input_hash_cached(
                cache,
                spec.frontmatter_kind,
                &path,
                &[TemplateKey::StatusMessage, TemplateKey::MessageSection],
                json!({
                    "message_hash": blake3_json_fingerprint(message)?,
                    "context": context.clone(),
                }),
            )?;
            return self.render_cached_string(
                cache,
                &path,
                spec.frontmatter_kind,
                input_hash,
                || {
                    let conversation = self.message_conversation_with_renderer(
                        message,
                        config,
                        renderer,
                        path.parent(),
                    )?;
                    let mut context = context.clone();
                    if let Value::Object(root) = &mut context {
                        if let Some(Value::Object(view)) = root.get_mut("view") {
                            view.insert("conversation".to_string(), json!(conversation.trim()));
                        }
                    }
                    renderer.render(TemplateKey::StatusMessage, &context)
                },
            );
        }
        let conversation =
            self.message_conversation_with_renderer(message, config, renderer, path.parent())?;
        let mut context = context;
        if let Value::Object(root) = &mut context {
            if let Some(Value::Object(view)) = root.get_mut("view") {
                view.insert("conversation".to_string(), json!(conversation.trim()));
            }
        }
        let rendered = renderer.render(TemplateKey::StatusMessage, &context)?;
        write_string(&path, &rendered)?;
        Ok(FileWriteOutcome::Written)
    }

    fn write_disposition_index_inner(
        &self,
        spec: DispositionViewSpec,
        messages: &mut [MessageFile],
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        cache: Option<&mut RenderCache>,
    ) -> Result<FileWriteOutcome> {
        messages.sort_by(|a, b| compare_message_time_asc(b, a));
        let offset = config.resolved_timezone_offset();
        let items = messages
            .iter()
            .map(|message| {
                let title = message
                    .subject
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(message.message_id.as_str())
                    .to_string();
                let item = thread_item_common(
                    message,
                    &offset,
                    format!("{}.md", message.message_id),
                    title,
                )?;
                Ok(item)
            })
            .collect::<Result<Vec<_>>>()?;
        let context = json!({
            "frontmatter": {
                "kind": format!("{}_index", spec.frontmatter_kind),
                "status": spec.status.as_str(),
                "message_count": items.len(),
            },
            "items": items,
            "view": {
                "language": config.resolved_language_bcp47(),
                "status": spec.status.as_str(),
                "status_dir": spec.dir,
                "message_count": items.len(),
            },
        });
        let path = self.root.join(spec.dir).join("index.md");
        if let Some(cache) = cache {
            let input_hash = self.render_input_hash_cached(
                cache,
                &format!("{}_index", spec.frontmatter_kind),
                &path,
                &[TemplateKey::StatusIndex],
                json!({
                    "message_hashes": messages
                        .iter()
                        .map(blake3_json_fingerprint)
                        .collect::<Result<Vec<_>>>()?,
                    "context": context.clone(),
                }),
            )?;
            return self.render_cached_string(
                cache,
                &path,
                &format!("{}_index", spec.frontmatter_kind),
                input_hash,
                || renderer.render(TemplateKey::StatusIndex, &context),
            );
        }
        let rendered = renderer.render(TemplateKey::StatusIndex, &context)?;
        write_string(&path, &rendered)?;
        Ok(FileWriteOutcome::Written)
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

    pub(super) fn disposition_json_path(&self, spec: DispositionViewSpec) -> PathBuf {
        self.root.join(spec.dir).join("data").join(spec.data_file)
    }
}

pub(super) fn disposition_spec_for_status(status: MessageStatus) -> Option<DispositionViewSpec> {
    DISPOSITION_VIEW_SPECS
        .iter()
        .copied()
        .find(|spec| spec.status == status)
}
