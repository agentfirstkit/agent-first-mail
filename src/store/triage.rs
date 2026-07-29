use super::*;

impl Workspace {
    pub(crate) fn triage_template_document(
        &self,
        message_id: &str,
    ) -> Result<TriageRenderDocument> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let config = MailConfig::load(&self.root)?;
        let message = self.read_message_by_id(message_id)?;
        let related_message_ids = self.related_message_ids(message_id)?;
        let related_messages =
            self.related_message_documents(&related_message_ids, &config, None)?;
        Ok(triage_render_document(
            self.message_render_document_from_parts(&message, &config)?,
            related_messages,
        ))
    }

    /// List untriaged messages by stable locator. Resolve details with
    /// path_templates or `afmail message show`.
    pub fn triage_list(&self, limit: Option<usize>, details: bool) -> Result<Value> {
        self.require_workspace()?;
        let triage_dir = self.root.join(layout::TRIAGE);
        let mut items = Vec::new();
        let mut total_count = 0usize;
        if triage_dir.exists() {
            let mut paths: Vec<PathBuf> = read_dir(&triage_dir, "read triage directory")?
                .into_iter()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("md"))
                .collect();
            paths.sort();
            total_count = paths.len();
            let limited = match limit {
                Some(limit) => take_last(paths, limit),
                None => paths,
            };
            let relation_index = if details {
                Some(MessageRelationIndex::build(self)?)
            } else {
                None
            };
            let cases = if details {
                Some(CaseIndex::build(self)?)
            } else {
                None
            };
            for path in limited {
                let message_id = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default();
                if details {
                    let message = self.read_message_by_id(&message_id)?;
                    let related_message_ids = relation_index
                        .as_ref()
                        .map(|index| index.related_message_ids(&message.message_id).to_vec())
                        .unwrap_or_default();
                    let related_messages =
                        self.related_message_meta_rows(&related_message_ids, cases.as_ref())?;
                    items.push(json!({
                        "message_id": message_id,
                        "from": message.from.as_deref(),
                        "subject": message.subject.as_deref(),
                        "direction": message.direction.as_deref(),
                        "received_rfc3339": message.received_rfc3339.as_deref(),
                        "sent_rfc3339": message.sent_rfc3339.as_deref(),
                        "workspace_status": message.workspace.status.as_str(),
                        "attachment_count": message.attachments.len(),
                        "authentication_warning": message.authentication.is_warning(),
                        "authentication": &message.authentication,
                        "related_messages": related_messages,
                        "case_refs": cases
                            .as_ref()
                            .map(|index| index.case_refs_for_message(&message_id))
                            .unwrap_or_default(),
                        "view_path": rel_path(&self.root, &path),
                        "json_path": format!("messages/{message_id}.json"),
                    }));
                } else {
                    items.push(json!({"message_id": message_id}));
                }
            }
        }
        let push_status = serde_json::to_value(crate::push_queue::push_status(&self.root)?)
            .map_err(|e| AppError::json("serialize push status", &e))?;
        let pt_view_path = [layout::TRIAGE, "/{message_id}.md"].concat();
        Ok(json!({
            "count": total_count,
            "returned_count": items.len(),
            "limited": limit.is_some_and(|limit| total_count > limit),
            "limit": limit,
            "details": details,
            "push_status": push_status,
            "path_templates": {
                "view_path": pt_view_path,
                "json_path": "messages/{message_id}.json",
            },
            "items": items,
        }))
    }

    pub fn refresh_triage_views(&self) -> Result<Value> {
        self.refresh_triage_views_inner(None, None, None, None)
    }

    pub(super) fn refresh_triage_views_cached_with_progress(
        &self,
        cache: &mut RenderCache,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
        progress_phase: &'static str,
        render_context: &RenderContext,
    ) -> Result<Value> {
        self.refresh_triage_views_inner(
            Some(cache),
            progress,
            Some(progress_phase),
            Some(render_context),
        )
    }

    fn refresh_triage_views_inner(
        &self,
        mut cache: Option<&mut RenderCache>,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
        progress_phase: Option<&'static str>,
        render_context: Option<&RenderContext>,
    ) -> Result<Value> {
        self.require_workspace()?;
        create_dir_all(&self.root.join(layout::TRIAGE))?;
        let contact_map = if render_context.is_none() {
            Some(self.contact_email_map()?)
        } else {
            None
        };
        let owned_config = if render_context.is_none() {
            Some(MailConfig::load(&self.root)?)
        } else {
            None
        };
        let config = render_context
            .map(RenderContext::config)
            .or(owned_config.as_ref())
            .ok_or_else(|| AppError::new("internal", "render config was not initialized"))?;
        let mut renderer = TemplateRenderer::new(&self.root, config.template_language());
        let mut cases = None;
        let mut relation_index = None;
        let mut overlay_index = None;
        let mut desired = BTreeSet::new();
        let mut written_count = 0usize;
        let mut unchanged_count = 0usize;
        let paths = message_json_paths(&self.root)?;
        let total_count = paths.len();
        let mut progress = progress;
        for (index, path) in paths.into_iter().enumerate() {
            let message_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            let mut message = match render_context {
                Some(context) => context.read_message(self, &message_id)?,
                None => read_message(&path)?,
            };
            let before_overlays = message.clone();
            let overlay_index = match (&overlay_index, render_context) {
                (_, Some(_)) => None,
                (Some(index), None) => Some(index),
                (None, None) => {
                    overlay_index = Some(self.materialized_overlay_index()?);
                    let Some(index) = overlay_index.as_ref() else {
                        return Err(AppError::new(
                            "internal",
                            "overlay index was not initialized",
                        ));
                    };
                    Some(index)
                }
            };
            if let Some(overlay_index) = overlay_index {
                self.apply_materialized_workspace_overlays_with_index(&mut message, overlay_index)?;
                self.apply_identity_match(&mut message, config)?;
                self.apply_contact_link(
                    &mut message,
                    contact_map.as_ref().ok_or_else(|| {
                        AppError::new("internal", "contact map was not initialized")
                    })?,
                );
            }
            message.workspace.remote_sync = None;
            if render_context.is_none() && message != before_overlays {
                self.write_message_materialized_cache(&message)?;
            }
            let cases = match &cases {
                Some(index) => index,
                None => {
                    cases = Some(CaseIndex::build(self)?);
                    let Some(index) = cases.as_ref() else {
                        return Err(AppError::new("internal", "case index was not initialized"));
                    };
                    index
                }
            };
            if self.triage_candidate(&message, cases)? {
                desired.insert(message.message_id.clone());
                let relation_index = match (render_context, &relation_index) {
                    (Some(context), _) => context.relation_index(),
                    (None, Some(index)) => index,
                    (None, None) => {
                        relation_index = Some(MessageRelationIndex::build(self)?);
                        let Some(index) = relation_index.as_ref() else {
                            return Err(AppError::new(
                                "internal",
                                "message relation index was not initialized",
                            ));
                        };
                        index
                    }
                };
                let outcome = self.write_triage_view_inner(
                    &message,
                    Some(relation_index),
                    config,
                    &mut renderer,
                    cache.as_deref_mut(),
                    render_context,
                )?;
                match outcome {
                    FileWriteOutcome::Written => written_count += 1,
                    FileWriteOutcome::Unchanged => unchanged_count += 1,
                }
            }
            let processed_count = index + 1;
            emit_triage_progress(
                &mut progress,
                progress_phase,
                processed_count,
                total_count,
                desired.len(),
                written_count,
                unchanged_count,
            );
        }
        let stale_count = self.remove_stale_triage_views(&desired)?;
        Ok(json!({
            "triage_count": desired.len(),
            "triage_written_count": written_count,
            "triage_unchanged_count": unchanged_count,
            "stale_triage_removed_count": stale_count
        }))
    }
}

pub(crate) fn render_triage_view(
    root: &Path,
    config: &MailConfig,
    message: &MessageFile,
    related_messages: Vec<MessageRenderDocument>,
) -> Result<String> {
    let mut renderer = TemplateRenderer::new(root, config.template_language());
    let message = build_message_render_document(
        message,
        config
            .default_identity()
            .ok()
            .map(|identity| identity.email.as_str()),
    );
    let document = triage_render_document(message, related_messages);
    render_triage_document(
        &mut renderer,
        config,
        &document,
        Some(&root.join(layout::TRIAGE)),
    )
}

fn render_triage_document(
    renderer: &mut TemplateRenderer<'_>,
    config: &MailConfig,
    document: &TriageRenderDocument,
    output_dir: Option<&Path>,
) -> Result<String> {
    renderer.render_with_options(
        TemplateKey::TriageView,
        document,
        TemplateRenderOptions {
            timezone_offset: Some(config.resolved_timezone_offset()),
            output_dir,
        },
    )
}

impl Workspace {
    pub(super) fn refresh_all_case_message_views(&self) -> Result<usize> {
        let language = self.template_language()?;
        let mut renderer = TemplateRenderer::new(&self.root, language);
        let mut count = 0usize;
        for (_, case_path) in self.all_case_entries()? {
            self.refresh_case_message_views_with_renderer(&case_path, &mut renderer)?;
            count += 1;
        }
        Ok(count)
    }

    pub(super) fn refresh_all_case_message_views_cached(
        &self,
        cache: &mut RenderCache,
        render_context: &RenderContext,
        mut progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<CaseViewRefresh> {
        let language = render_context.config().template_language();
        let mut renderer = TemplateRenderer::new(&self.root, language);
        let mut totals = CaseViewRefresh::default();
        let entries = self.all_case_entries()?;
        let total_count = entries.len();
        for (index, (_, case_path)) in entries.into_iter().enumerate() {
            let refresh = self.refresh_case_message_views_with_cache_context(
                &case_path,
                &mut renderer,
                cache,
                Some(render_context),
            )?;
            totals.case_index_count += refresh.case_index_count;
            totals.case_message_count += refresh.case_message_count;
            totals.case_index_unchanged_count += refresh.case_index_unchanged_count;
            totals.case_message_unchanged_count += refresh.case_message_unchanged_count;
            let processed_count = index + 1;
            if processed_count == total_count || processed_count.is_multiple_of(25) {
                crate::progress::emit(
                    &mut progress,
                    "pull_render_cases_progress",
                    json!({
                        "render_stage": "cases",
                        "processed_count": processed_count,
                        "total_count": total_count,
                        "case_view_written_count": totals.case_index_count + totals.case_message_count,
                        "case_view_unchanged_count": totals.case_index_unchanged_count + totals.case_message_unchanged_count,
                    }),
                );
            }
        }
        Ok(totals)
    }

    pub(super) fn refresh_case_message_views(&self, case_path: &Path) -> Result<()> {
        let language = self.template_language()?;
        let mut renderer = TemplateRenderer::new(&self.root, language);
        self.refresh_case_message_views_with_renderer(case_path, &mut renderer)?;
        Ok(())
    }

    pub(super) fn refresh_case_message_views_with_renderer(
        &self,
        case_path: &Path,
        renderer: &mut TemplateRenderer<'_>,
    ) -> Result<CaseViewRefresh> {
        self.refresh_case_message_views_inner(case_path, renderer, None, None)
    }

    pub(super) fn refresh_case_message_views_with_cache_context(
        &self,
        case_path: &Path,
        renderer: &mut TemplateRenderer<'_>,
        cache: &mut RenderCache,
        render_context: Option<&RenderContext>,
    ) -> Result<CaseViewRefresh> {
        self.refresh_case_message_views_inner(case_path, renderer, Some(cache), render_context)
    }

    fn refresh_case_message_views_inner(
        &self,
        case_path: &Path,
        renderer: &mut TemplateRenderer<'_>,
        mut cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<CaseViewRefresh> {
        if !case_json_path(case_path).is_file() {
            return Ok(CaseViewRefresh::default());
        }
        let case_fm = read_case_file(case_path)?;
        let messages_dir = case_views_messages_dir(case_path);
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
        let mut desired = BTreeSet::new();
        let mut messages = Vec::new();
        let mut message_stats = RenderWriteStats::default();
        for message_id in case_fm.message_ids() {
            let message = match render_context {
                Some(context) => context.read_message(self, &message_id)?,
                None => self.read_message_by_id(&message_id)?,
            };
            desired.insert(message_id.clone());
            let view_path = case_message_view_path(case_path, &message_id);
            let document = self.case_message_render_document(
                &case_fm,
                &message,
                case_path,
                config,
                render_context,
            )?;
            let outcome = if let Some(cache) = cache.as_deref_mut() {
                let input_hash =
                    self.render_input_hash_cached(cache, "case_message", &view_path, &document)?;
                self.render_cached_string(cache, &view_path, "case_message", input_hash, || {
                    renderer.render_with_options(
                        TemplateKey::CaseMessage,
                        &document,
                        TemplateRenderOptions {
                            timezone_offset: Some(config.resolved_timezone_offset()),
                            output_dir: view_path.parent(),
                        },
                    )
                })?
            } else {
                let view = renderer.render_with_options(
                    TemplateKey::CaseMessage,
                    &document,
                    TemplateRenderOptions {
                        timezone_offset: Some(config.resolved_timezone_offset()),
                        output_dir: view_path.parent(),
                    },
                )?;
                write_string(&view_path, &view)?;
                FileWriteOutcome::Written
            };
            message_stats.record(outcome);
            messages.push(message);
        }
        self.remove_stale_case_message_views(&messages_dir, &desired)?;
        let case_index_path = case_path.join("case.md");
        let document = self.case_render_document_from_parts(
            &case_fm,
            &messages,
            case_path,
            config,
            render_context,
        )?;
        let case_index_outcome = if let Some(cache) = cache {
            let input_hash =
                self.render_input_hash_cached(cache, "case_index", &case_index_path, &document)?;
            self.render_cached_string(cache, &case_index_path, "case_index", input_hash, || {
                renderer.render_with_options(
                    TemplateKey::CaseDocument,
                    &document,
                    TemplateRenderOptions {
                        timezone_offset: Some(config.resolved_timezone_offset()),
                        output_dir: Some(case_path),
                    },
                )
            })?
        } else {
            let case_doc = renderer.render_with_options(
                TemplateKey::CaseDocument,
                &document,
                TemplateRenderOptions {
                    timezone_offset: Some(config.resolved_timezone_offset()),
                    output_dir: Some(case_path),
                },
            )?;
            write_string(&case_index_path, &case_doc)?;
            FileWriteOutcome::Written
        };
        Ok(CaseViewRefresh {
            case_index_count: usize::from(case_index_outcome == FileWriteOutcome::Written),
            case_message_count: message_stats.written_count,
            case_index_unchanged_count: usize::from(
                case_index_outcome == FileWriteOutcome::Unchanged,
            ),
            case_message_unchanged_count: message_stats.unchanged_count,
        })
    }

    #[cfg(test)]
    pub(super) fn render_case_message_view(
        &self,
        case_fm: &CaseFrontmatter,
        message: &MessageFile,
        renderer: &mut TemplateRenderer<'_>,
        case_path: &Path,
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
        let document =
            self.case_message_render_document(case_fm, message, case_path, config, render_context)?;
        let output_dir = case_message_view_path(case_path, &document.message.message.message_id)
            .parent()
            .map(Path::to_path_buf);
        renderer.render_with_options(
            TemplateKey::CaseMessage,
            &document,
            TemplateRenderOptions {
                timezone_offset: Some(config.resolved_timezone_offset()),
                output_dir: output_dir.as_deref(),
            },
        )
    }

    fn case_message_render_document(
        &self,
        case_fm: &CaseFrontmatter,
        message: &MessageFile,
        case_path: &Path,
        config: &MailConfig,
        render_context: Option<&RenderContext>,
    ) -> Result<CollectionMessageRenderDocument> {
        let message = self.message_render_document_with_context(message, config, render_context)?;
        let location = self.case_location_for_path(case_path);
        Ok(collection_message_render_document(
            case_fm.clone(),
            message,
            Some(location),
        ))
    }

    #[cfg(test)]
    pub(super) fn render_case_document(
        &self,
        case_fm: &CaseFrontmatter,
        messages: &[MessageFile],
        case_path: &Path,
        config: &MailConfig,
        renderer: &mut TemplateRenderer<'_>,
        render_context: Option<&RenderContext>,
    ) -> Result<String> {
        let document = self.case_render_document_from_parts(
            case_fm,
            messages,
            case_path,
            config,
            render_context,
        )?;
        renderer.render_with_options(
            TemplateKey::CaseDocument,
            &document,
            TemplateRenderOptions {
                timezone_offset: Some(config.resolved_timezone_offset()),
                output_dir: Some(case_path),
            },
        )
    }

    fn case_render_document_from_parts(
        &self,
        case_fm: &CaseFrontmatter,
        messages: &[MessageFile],
        case_path: &Path,
        config: &MailConfig,
        render_context: Option<&RenderContext>,
    ) -> Result<CollectionRenderDocument> {
        let messages =
            self.message_render_documents_with_context(messages, config, render_context)?;
        Ok(collection_render_document(
            case_fm.clone(),
            messages,
            Some(self.case_location_for_path(case_path)),
        ))
    }

    fn case_location_for_path(&self, case_path: &Path) -> CollectionLocation {
        let group = case_path
            .parent()
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        case_collection_location(group)
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

    pub(super) fn triage_candidate(
        &self,
        message: &MessageFile,
        cases: &CaseIndex,
    ) -> Result<bool> {
        let current = MessageStatus::parse(&message.workspace.status)?;
        if current.is_terminal_local() {
            return Ok(false);
        }
        if message.workspace.notification_uid.is_some() || current == MessageStatus::Archived {
            return Ok(false);
        }
        if message.workspace.origin.is_some() {
            return Ok(false);
        }
        Ok(!cases.has_any_reference(&message.message_id))
    }

    pub(super) fn write_triage_view(&self, message: &MessageFile) -> Result<()> {
        let config = MailConfig::load(&self.root)?;
        let mut renderer = TemplateRenderer::new(&self.root, config.template_language());
        self.write_triage_view_inner(message, None, &config, &mut renderer, None, None)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_triage_view_inner(
        &self,
        message: &MessageFile,
        relation_index: Option<&MessageRelationIndex>,
        config: &MailConfig,
        renderer: &mut TemplateRenderer<'_>,
        cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<FileWriteOutcome> {
        let path = self
            .root
            .join(layout::TRIAGE)
            .join(format!("{}.md", message.message_id));
        let related_message_ids = match relation_index {
            Some(index) => index.related_message_ids(&message.message_id).to_vec(),
            None => self.related_message_ids(&message.message_id)?,
        };
        let related_messages =
            self.related_message_documents(&related_message_ids, config, render_context)?;
        let message = self.message_render_document_with_context(message, config, render_context)?;
        let document = triage_render_document(message, related_messages);
        if let Some(cache) = cache {
            let input_hash =
                self.render_input_hash_cached(cache, "triage_view", &path, &document)?;
            return self.render_cached_string(cache, &path, "triage_view", input_hash, || {
                render_triage_document(renderer, config, &document, path.parent())
            });
        }

        let rendered = render_triage_document(renderer, config, &document, path.parent())?;
        write_string(&path, &rendered)?;
        Ok(FileWriteOutcome::Written)
    }

    fn related_message_documents(
        &self,
        related_message_ids: &[String],
        config: &MailConfig,
        render_context: Option<&RenderContext>,
    ) -> Result<Vec<MessageRenderDocument>> {
        let messages = related_message_ids
            .iter()
            .map(|message_id| match render_context {
                Some(context) => context.read_message(self, message_id),
                None => self.read_message_by_id(message_id),
            })
            .collect::<Result<Vec<_>>>()?;
        self.message_render_documents_with_context(&messages, config, render_context)
    }

    pub(super) fn related_message_meta_rows(
        &self,
        related_message_ids: &[String],
        cases: Option<&CaseIndex>,
    ) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for related_message_id in related_message_ids {
            let related = self.read_message_by_id(related_message_id)?;
            out.push(json!({
                "message_id": related.message_id.as_str(),
                "from": related.from.as_deref(),
                "subject": related.subject.as_deref(),
                "direction": related.direction.as_deref(),
                "received_rfc3339": related.received_rfc3339.as_deref(),
                "sent_rfc3339": related.sent_rfc3339.as_deref(),
                "workspace_status": related.workspace.status.as_str(),
                "view_path": self.message_existing_view_path(&related)?,
                "case_refs": cases
                    .map(|index| index.case_refs_for_message(related_message_id))
                    .unwrap_or_default(),
            }));
        }
        Ok(out)
    }

    pub(super) fn remove_stale_triage_views(&self, desired: &BTreeSet<String>) -> Result<usize> {
        let triage_dir = self.root.join(layout::TRIAGE);
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

fn emit_triage_progress(
    progress: &mut Option<&mut crate::progress::ProgressCallback<'_>>,
    phase: Option<&'static str>,
    processed_count: usize,
    total_count: usize,
    triage_count: usize,
    written_count: usize,
    unchanged_count: usize,
) {
    let Some(phase) = phase else {
        return;
    };
    if processed_count != total_count && !processed_count.is_multiple_of(250) {
        return;
    }
    crate::progress::emit(
        progress,
        phase,
        json!({
            "render_stage": "triage",
            "processed_count": processed_count,
            "total_count": total_count,
            "triage_count": triage_count,
            "triage_written_count": written_count,
            "triage_unchanged_count": unchanged_count,
        }),
    );
}
