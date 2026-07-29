use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct DispositionViewSpec {
    pub(super) status: MessageStatus,
    pub(super) dir: &'static str,
    pub(super) data_file: &'static str,
    pub(super) schema_name: &'static str,
    pub(super) collection_uid: &'static str,
    frontmatter_kind: &'static str,
}

pub(super) const DISPOSITION_VIEW_SPECS: [DispositionViewSpec; 3] = [
    DispositionViewSpec {
        status: MessageStatus::Spam,
        dir: layout::SPAM,
        data_file: "spam.json",
        schema_name: "spam",
        collection_uid: "spam",
        frontmatter_kind: "spam_message",
    },
    DispositionViewSpec {
        status: MessageStatus::Trashed,
        dir: layout::TRASH,
        data_file: "trash.json",
        schema_name: "trash",
        collection_uid: "trash",
        frontmatter_kind: "trash_message",
    },
    DispositionViewSpec {
        status: MessageStatus::DeletedRemote,
        dir: layout::DELETED,
        data_file: "deleted.json",
        schema_name: "deleted_remote",
        collection_uid: "deleted",
        frontmatter_kind: "deleted_message",
    },
];

struct DispositionIndexInput<'a> {
    spec: DispositionViewSpec,
    collection: &'a MessageCollection,
    messages: &'a [MessageFile],
    config: &'a MailConfig,
    render_context: Option<&'a RenderContext>,
}

impl DispositionViewSpec {
    fn empty_collection(self) -> MessageCollection {
        let now = now_rfc3339();
        let mut collection =
            MessageCollection::new(self.schema_name, self.collection_uid, self.collection_uid);
        collection.status = self.status.as_str().to_string();
        collection.created_rfc3339 = Some(now.clone());
        collection.updated_rfc3339 = Some(now);
        collection
    }
}

pub(super) fn validate_disposition_collection(
    root: &Path,
    path: &Path,
    spec: DispositionViewSpec,
    messages: &MessageCollection,
) -> Result<()> {
    let duplicate_message_id = messages.duplicate_message_id();
    let content_issues = message_collection_content_issues(messages);
    let valid = messages.schema_name == spec.schema_name
        && messages.schema_version == MESSAGE_COLLECTION_SCHEMA_VERSION
        && messages.collection_uid == spec.collection_uid
        && messages.collection_name == spec.collection_uid
        && messages.status == spec.status.as_str()
        && messages.archived_rfc3339.is_none()
        && messages.agent_suggestion.is_none()
        && messages.human_suggestion.is_none()
        && content_issues.is_empty()
        && duplicate_message_id.is_none();
    if valid {
        return Ok(());
    }
    Err(AppError::new(
        "disposition_messages_invalid",
        format!("invalid disposition messages: {}", rel_path(root, path)),
    )
    .with_details(json!({
        "path": rel_path(root, path),
        "expected_schema_name": spec.schema_name,
        "schema_name": messages.schema_name,
        "expected_schema_version": MESSAGE_COLLECTION_SCHEMA_VERSION,
        "schema_version": messages.schema_version,
        "expected_collection_uid": spec.collection_uid,
        "collection_uid": messages.collection_uid,
        "expected_collection_name": spec.collection_uid,
        "collection_name": messages.collection_name,
        "expected_status": spec.status.as_str(),
        "status": messages.status,
        "archived_rfc3339": messages.archived_rfc3339,
        "has_agent_suggestion": messages.agent_suggestion.is_some(),
        "has_human_suggestion": messages.human_suggestion.is_some(),
        "duplicate_message_id": duplicate_message_id,
        "content_issues": content_issues,
    })))
}

fn validate_disposition_message(spec: DispositionViewSpec, message: &MessageFile) -> Result<()> {
    if message.workspace.status == spec.status.as_str()
        && message.workspace.notification_uid.is_none()
        && message.workspace.notification_rfc3339.is_none()
        && message.workspace.origin.is_none()
    {
        return Ok(());
    }
    Err(AppError::new(
        "disposition_state_invalid",
        format!(
            "message {} does not match canonical {} disposition state",
            message.message_id, spec.collection_uid
        ),
    )
    .with_details(json!({
        "message_id": message.message_id,
        "expected_status": spec.status.as_str(),
        "status": message.workspace.status,
        "notification_uid": message.workspace.notification_uid,
        "notification_rfc3339": message.workspace.notification_rfc3339,
        "origin": message.workspace.origin,
    })))
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
        validate_disposition_collection(&self.root, &path, spec, &messages)?;
        Ok(messages)
    }

    pub(super) fn write_disposition_messages(
        &self,
        spec: DispositionViewSpec,
        data: &MessageCollection,
    ) -> Result<()> {
        let path = self.disposition_json_path(spec);
        validate_disposition_collection(&self.root, &path, spec, data)?;
        if data.items.is_empty() && !path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        write_json_pretty(&path, data)
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
        data.updated_rfc3339 = Some(now_rfc3339());
        self.write_disposition_messages(spec, &data)?;
        Ok(true)
    }

    pub(super) fn disposition_state_for_message(
        &self,
        message_id: &str,
    ) -> Result<Option<(MessageStatus, String)>> {
        let mut state: Option<(MessageStatus, String)> = None;
        for spec in DISPOSITION_VIEW_SPECS {
            let data = self.read_disposition_messages(spec)?;
            if let Some(item) = data.items.iter().find(|item| item.message_id == message_id) {
                if let Some((existing_status, _)) = &state {
                    return Err(AppError::new(
                        "disposition_state_invalid",
                        format!(
                            "message {message_id} is present in both {} and {} dispositions",
                            existing_status.as_str(),
                            spec.status.as_str()
                        ),
                    ));
                }
                state = Some((spec.status, item.added_rfc3339.clone()));
            }
        }
        Ok(state)
    }

    pub(super) fn disposition_state_by_message(&self) -> Result<BTreeMap<String, MessageStatus>> {
        let mut out = BTreeMap::new();
        for spec in DISPOSITION_VIEW_SPECS {
            let data = self.read_disposition_messages(spec)?;
            for item in data.items {
                if let Some(existing_status) = out.insert(item.message_id.clone(), spec.status) {
                    return Err(AppError::new(
                        "disposition_state_invalid",
                        format!(
                            "message {} is present in both {} and {} dispositions",
                            item.message_id,
                            existing_status.as_str(),
                            spec.status.as_str()
                        ),
                    ));
                }
            }
        }
        Ok(out)
    }

    pub(super) fn refresh_disposition_views(&self) -> Result<Value> {
        self.refresh_disposition_views_inner(None, None)
    }

    pub(super) fn refresh_disposition_views_cached(
        &self,
        cache: &mut RenderCache,
        render_context: &RenderContext,
    ) -> Result<Value> {
        self.refresh_disposition_views_inner(Some(cache), Some(render_context))
    }

    fn refresh_disposition_views_inner(
        &self,
        mut cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let owned_config = if render_context.is_none() {
            Some(MailConfig::load(&self.root)?)
        } else {
            None
        };
        let config = render_context
            .map(RenderContext::config)
            .or(owned_config.as_ref())
            .ok_or_else(|| AppError::new("internal", "render config was not initialized"))?;
        let language = config.template_language();
        let mut renderer = TemplateRenderer::new(&self.root, language);
        let mut groups: BTreeMap<&'static str, Vec<MessageFile>> = BTreeMap::new();
        let mut collections: BTreeMap<&'static str, MessageCollection> = BTreeMap::new();
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
            collections.insert(spec.dir, data.clone());
            for item in &data.items {
                let mut message = match render_context {
                    Some(context) => context.read_message(self, &item.message_id)?,
                    None => self.read_message_by_id(&item.message_id)?,
                };
                validate_disposition_message(spec, &message)?;
                if render_context.is_none() {
                    self.apply_identity_match(&mut message, config)?;
                }
                desired
                    .entry(spec.dir)
                    .or_default()
                    .insert(message.message_id.clone());
                let outcome = self.write_disposition_message_view_inner(
                    spec,
                    &message,
                    config,
                    &mut renderer,
                    cache.as_deref_mut(),
                    render_context,
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
            let messages = groups.remove(spec.dir).unwrap_or_default();
            if messages.is_empty() {
                continue;
            }
            let outcome = self.write_disposition_index_inner(
                DispositionIndexInput {
                    spec,
                    collection: collections.get(spec.dir).ok_or_else(|| {
                        AppError::new("internal", "disposition collection was not initialized")
                    })?,
                    messages: &messages,
                    config,
                    render_context,
                },
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
        renderer: &mut TemplateRenderer<'_>,
        cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<FileWriteOutcome> {
        let path = self.disposition_message_view_path(spec, &message.message_id);
        let document =
            self.message_render_document_with_context(message, config, render_context)?;
        if let Some(cache) = cache {
            let input_hash =
                self.render_input_hash_cached(cache, spec.frontmatter_kind, &path, &document)?;
            return self.render_cached_string(
                cache,
                &path,
                spec.frontmatter_kind,
                input_hash,
                || {
                    renderer.render_with_options(
                        TemplateKey::StatusMessage,
                        &document,
                        TemplateRenderOptions {
                            timezone_offset: Some(config.resolved_timezone_offset()),
                            output_dir: path.parent(),
                        },
                    )
                },
            );
        }
        let rendered = renderer.render_with_options(
            TemplateKey::StatusMessage,
            &document,
            TemplateRenderOptions {
                timezone_offset: Some(config.resolved_timezone_offset()),
                output_dir: path.parent(),
            },
        )?;
        write_string(&path, &rendered)?;
        Ok(FileWriteOutcome::Written)
    }

    fn write_disposition_index_inner(
        &self,
        input: DispositionIndexInput<'_>,
        renderer: &mut TemplateRenderer<'_>,
        cache: Option<&mut RenderCache>,
    ) -> Result<FileWriteOutcome> {
        let documents = self.message_render_documents_with_context(
            input.messages,
            input.config,
            input.render_context,
        )?;
        let document = collection_render_document(input.collection.clone(), documents, None);
        let path = self.root.join(input.spec.dir).join("index.md");
        if let Some(cache) = cache {
            let input_hash = self.render_input_hash_cached(
                cache,
                &format!("{}_index", input.spec.frontmatter_kind),
                &path,
                &document,
            )?;
            return self.render_cached_string(
                cache,
                &path,
                &format!("{}_index", input.spec.frontmatter_kind),
                input_hash,
                || {
                    renderer.render_with_options(
                        TemplateKey::StatusIndex,
                        &document,
                        TemplateRenderOptions {
                            timezone_offset: Some(input.config.resolved_timezone_offset()),
                            output_dir: path.parent(),
                        },
                    )
                },
            );
        }
        let rendered = renderer.render_with_options(
            TemplateKey::StatusIndex,
            &document,
            TemplateRenderOptions {
                timezone_offset: Some(input.config.resolved_timezone_offset()),
                output_dir: path.parent(),
            },
        )?;
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
