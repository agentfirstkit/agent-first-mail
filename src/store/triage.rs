use super::*;

impl Workspace {
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
                    let requires_case = !related_message_ids.is_empty();
                    items.push(json!({
                        "message_id": message_id,
                        "from": message.from.as_deref().unwrap_or(""),
                        "subject": message.subject.as_deref().unwrap_or(""),
                        "direction": message.direction.as_deref().unwrap_or(""),
                        "received_rfc3339": message.received_rfc3339.as_deref().unwrap_or(""),
                        "sent_rfc3339": message.sent_rfc3339.as_deref().unwrap_or(""),
                        "workspace_status": message.workspace.status.as_str(),
                        "attachment_count": message.attachments.len(),
                        "authentication_warning": message.authentication.is_warning(),
                        "authentication": &message.authentication,
                        "related_message_ids": related_message_ids,
                        "related_messages": related_messages,
                        "case_refs": cases
                            .as_ref()
                            .map(|index| index.case_refs_for_message(&message_id))
                            .unwrap_or_default(),
                        "requires_case": requires_case,
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
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, config.template_language());
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
            let view_path = self
                .root
                .join(layout::TRIAGE)
                .join(format!("{message_id}.md"));
            if let Some(cache) = cache.as_deref_mut()
                && view_path.is_file()
            {
                let (suggested_case_uids, suggested_reason) =
                    existing_triage_suggestion(&view_path)?;
                let input_hash = self.render_input_hash_cached(
                    cache,
                    "triage_view",
                    &view_path,
                    &[TemplateKey::TriageView, TemplateKey::MessageSection],
                    json!({
                        "message_hash": match render_context
                            .and_then(|context| context.snapshot(&message_id))
                        {
                            Some(snapshot) => snapshot.hash.clone(),
                            None => message_file_fingerprint(&path)?,
                        },
                        "quoted_message_id": render_context
                            .and_then(|context| context.quoted_message_id(&message_id)),
                        "related_messages": render_context
                            .map(|context| context.related_message_dependencies(&message_id))
                            .transpose()?
                            .unwrap_or_default(),
                        "suggested_case_uids": suggested_case_uids,
                        "suggested_reason": suggested_reason,
                    }),
                )?;
                if cache.entry_matches(
                    &rel_path(&self.root, &view_path),
                    "triage_view",
                    &input_hash,
                ) {
                    desired.insert(message_id);
                    unchanged_count += 1;
                    emit_triage_progress(
                        &mut progress,
                        progress_phase,
                        index + 1,
                        total_count,
                        desired.len(),
                        written_count,
                        unchanged_count,
                    );
                    continue;
                }
            }
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
                    Some(&path),
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
    language: TemplateLanguage,
    message: &MessageFile,
    conversation: &str,
    suggested_case_uids: Vec<String>,
    suggested_reason: Option<String>,
    related_messages: Vec<Value>,
) -> Result<String> {
    let mut renderer = MarkdownTemplateRenderer::new(root, language);
    render_triage_view_with_renderer(
        &mut renderer,
        language,
        message,
        conversation,
        suggested_case_uids,
        suggested_reason,
        related_messages,
    )
}

fn render_triage_view_with_renderer(
    renderer: &mut MarkdownTemplateRenderer<'_>,
    language: TemplateLanguage,
    message: &MessageFile,
    conversation: &str,
    suggested_case_uids: Vec<String>,
    suggested_reason: Option<String>,
    related_messages: Vec<Value>,
) -> Result<String> {
    let suggested_reason = suggested_reason.as_deref().unwrap_or("");
    let suggested_reason_yaml = if suggested_reason.is_empty() {
        String::new()
    } else {
        yaml_double_quote(suggested_reason)
    };
    let view_suggested_case_uids = suggested_case_uids.clone();
    renderer.render(
        TemplateKey::TriageView,
        &json!({
            "frontmatter": {
                "kind": "triage_view",
                "message_id": message.message_id.as_str(),
                "message_ids": [message.message_id.as_str()],
                "message_count": 1,
                "attachment_count": message.attachments.len(),
                "suggested_case_uids": &suggested_case_uids,
                "suggested_reason": suggested_reason,
                "suggested_reason_yaml": suggested_reason_yaml,
            },
            "message": message_template_value(message)?,
            "view": {
                "language": language.as_str(),
                "title": message.subject.as_deref().unwrap_or(""),
                "display_title": markdown_inline(message.subject.as_deref().unwrap_or("")),
                "attachment_count": message.attachments.len(),
                "suggested_case_uids": view_suggested_case_uids,
                "suggested_reason": suggested_reason,
                "related_messages": related_messages,
                "conversation": conversation.trim(),
            },
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

    pub(super) fn refresh_all_case_message_views_cached(
        &self,
        cache: &mut RenderCache,
        render_context: &RenderContext,
        mut progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<CaseViewRefresh> {
        let language = render_context.config().template_language();
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
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
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        self.refresh_case_message_views_with_renderer(case_path, &mut renderer)?;
        Ok(())
    }

    pub(super) fn refresh_case_message_views_with_renderer(
        &self,
        case_path: &Path,
        renderer: &mut MarkdownTemplateRenderer<'_>,
    ) -> Result<CaseViewRefresh> {
        self.refresh_case_message_views_inner(case_path, renderer, None, None)
    }

    pub(super) fn refresh_case_message_views_with_cache_context(
        &self,
        case_path: &Path,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        cache: &mut RenderCache,
        render_context: Option<&RenderContext>,
    ) -> Result<CaseViewRefresh> {
        self.refresh_case_message_views_inner(case_path, renderer, Some(cache), render_context)
    }

    fn refresh_case_message_views_inner(
        &self,
        case_path: &Path,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        mut cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<CaseViewRefresh> {
        if !case_json_path(case_path).is_file() {
            return Ok(CaseViewRefresh::default());
        }
        let case_fm = read_case_file(case_path)?;
        let case_uid = case_fm.collection_uid.clone();
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
            let outcome = if let Some(cache) = cache.as_deref_mut() {
                let input_hash = self.render_input_hash_cached(
                    cache,
                    "case_message",
                    &view_path,
                    &[TemplateKey::CaseMessage, TemplateKey::MessageSection],
                    json!({
                        "case_uid": case_uid.as_str(),
                        "case_name": case_fm.collection_name.as_str(),
                        "message_hash": render_context
                            .and_then(|context| context.snapshot(&message_id))
                            .map(|snapshot| snapshot.hash.clone())
                            .unwrap_or(blake3_json_fingerprint(&message)?),
                        "quoted_message_id": render_context
                            .and_then(|context| context.quoted_message_id(&message_id)),
                    }),
                )?;
                self.render_cached_string(cache, &view_path, "case_message", input_hash, || {
                    self.render_case_message_view(
                        &case_uid,
                        &case_fm.collection_name,
                        &message,
                        renderer,
                        view_path.parent(),
                        render_context,
                    )
                })?
            } else {
                let view = self.render_case_message_view(
                    &case_uid,
                    &case_fm.collection_name,
                    &message,
                    renderer,
                    view_path.parent(),
                    render_context,
                )?;
                write_string(&view_path, &view)?;
                FileWriteOutcome::Written
            };
            message_stats.record(outcome);
            messages.push(message);
        }
        self.remove_stale_case_message_views(&messages_dir, &desired)?;
        let case_index_path = case_path.join("case.md");
        let case_index_outcome = if let Some(cache) = cache {
            let input_hash = self.render_input_hash_cached(
                cache,
                "case_index",
                &case_index_path,
                &[TemplateKey::CaseDocument, TemplateKey::MessageSection],
                json!({
                    "case_hash": blake3_json_fingerprint(&case_fm)?,
                    "message_dependencies": match render_context {
                        Some(context) => messages
                            .iter()
                            .map(|message| context.message_section_dependency(&message.message_id))
                            .collect::<Result<Vec<_>>>()?,
                        None => messages
                            .iter()
                            .map(blake3_json_fingerprint)
                            .collect::<Result<Vec<_>>>()?
                            .into_iter()
                            .map(Value::String)
                            .collect::<Vec<_>>(),
                    },
                }),
            )?;
            self.render_cached_string(cache, &case_index_path, "case_index", input_hash, || {
                let conversation = self.render_case_conversation(
                    &messages,
                    config,
                    renderer,
                    case_path,
                    render_context,
                )?;
                self.render_case_document(&case_fm, &messages, &conversation, config, renderer)
            })?
        } else {
            let conversation = self.render_case_conversation(
                &messages,
                config,
                renderer,
                case_path,
                render_context,
            )?;
            let case_doc =
                self.render_case_document(&case_fm, &messages, &conversation, config, renderer)?;
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

    fn render_case_conversation(
        &self,
        messages: &[MessageFile],
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        case_path: &Path,
        render_context: Option<&RenderContext>,
    ) -> Result<String> {
        let mut conversation_messages = messages.to_vec();
        conversation_messages.sort_by(compare_message_time_asc);
        conversation_messages
            .iter()
            .map(|message| {
                let message_link = format!("views/messages/{}.md", message.message_id);
                self.message_conversation_with_link_with_renderer_context(
                    message,
                    config,
                    renderer,
                    Some(case_path),
                    &message_link,
                    render_context,
                )
            })
            .collect::<Result<Vec<_>>>()
            .map(|sections| sections.join("\n\n"))
    }

    pub(super) fn render_case_message_view(
        &self,
        case_uid: &str,
        case_name: &str,
        message: &MessageFile,
        renderer: &mut MarkdownTemplateRenderer<'_>,
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
        let title = message.subject.as_deref().unwrap_or("");
        let message_value = message_template_value(message)?;
        let conversation = self.message_conversation_with_renderer_context(
            message,
            config,
            renderer,
            output_dir,
            render_context,
        )?;
        let context = json!({
            "frontmatter": {
                "kind": "case_message",
                "case_uid": case_uid,
                "case_name": case_name,
                "message_id": message.message_id.as_str(),
            },
            "case": {
                "case_uid": case_uid,
                "case_name": case_name,
            },
            "message": message_value,
            "view": {
                "language": config.resolved_language_bcp47(),
                "title": title,
                "display_title": markdown_inline(title),
                "conversation": conversation.trim(),
            },
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
        let case_uid = case_fm.collection_uid.as_str();
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
                format!("views/messages/{}.md", message.message_id),
                title,
            )?;
            if let Value::Object(map) = &mut item
                && let Some(Value::Object(view)) = map.get_mut("view")
            {
                view.insert("fields".to_string(), json!(fields));
            }
            items.push(item);
        }
        let case_value = serde_json::to_value(&case_view)
            .map_err(|e| AppError::json("serialize case frontmatter", &e))?;
        let messages_value = sorted
            .iter()
            .map(message_template_value)
            .collect::<Result<Vec<_>>>()?;
        let context = json!({
            "frontmatter": {
                "kind": "case_index",
                "case_uid": case_uid,
                "case_name": case_fm.collection_name.as_str(),
                "message_count": items.len(),
            },
            "case": case_value,
            "items": items,
            "messages": messages_value,
            "view": {
                "language": config.resolved_language_bcp47(),
                "title": case_fm.collection_name.as_str(),
                "message_count": items.len(),
                "conversation": conversation.trim(),
            },
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
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, config.template_language());
        self.write_triage_view_inner(message, None, None, &config, &mut renderer, None, None)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_triage_view_inner(
        &self,
        message: &MessageFile,
        message_source_path: Option<&Path>,
        relation_index: Option<&MessageRelationIndex>,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        cache: Option<&mut RenderCache>,
        render_context: Option<&RenderContext>,
    ) -> Result<FileWriteOutcome> {
        let path = self
            .root
            .join(layout::TRIAGE)
            .join(format!("{}.md", message.message_id));
        let (suggested_case_uids, suggested_reason) = existing_triage_suggestion(&path)?;
        let related_message_ids = match relation_index {
            Some(index) => index.related_message_ids(&message.message_id).to_vec(),
            None => self.related_message_ids(&message.message_id)?,
        };
        if let Some(cache) = cache {
            let input_hash = self.render_input_hash_cached(
                cache,
                "triage_view",
                &path,
                &[TemplateKey::TriageView, TemplateKey::MessageSection],
                json!({
                    "message_hash": match render_context
                        .and_then(|context| context.snapshot(&message.message_id))
                    {
                        Some(snapshot) => snapshot.hash.clone(),
                        None => match message_source_path {
                            Some(path) => message_file_fingerprint(path)?,
                            None => blake3_json_fingerprint(message)?,
                        },
                    },
                    "quoted_message_id": render_context
                        .and_then(|context| context.quoted_message_id(&message.message_id)),
                    "related_messages": render_context
                        .map(|context| context.related_message_dependencies(&message.message_id))
                        .transpose()?
                        .unwrap_or_default(),
                    "suggested_case_uids": suggested_case_uids.clone(),
                    "suggested_reason": suggested_reason.clone(),
                }),
            )?;
            return self.render_cached_string(cache, &path, "triage_view", input_hash, || {
                let related_messages = self.related_message_rows(&related_message_ids)?;
                let conversation = self.message_conversation_with_renderer_context(
                    message,
                    config,
                    renderer,
                    path.parent(),
                    render_context,
                )?;
                render_triage_view_with_renderer(
                    renderer,
                    config.template_language(),
                    message,
                    &conversation,
                    suggested_case_uids,
                    suggested_reason,
                    related_messages,
                )
            });
        }

        let related_messages = self.related_message_rows(&related_message_ids)?;
        let conversation = self.message_conversation_with_renderer_context(
            message,
            config,
            renderer,
            path.parent(),
            render_context,
        )?;
        let rendered = render_triage_view_with_renderer(
            renderer,
            config.template_language(),
            message,
            &conversation,
            suggested_case_uids,
            suggested_reason,
            related_messages,
        )?;
        write_string(&path, &rendered)?;
        Ok(FileWriteOutcome::Written)
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
                "message": message_template_value(&related)?,
                "view": {
                    "direction": markdown_table_cell(related.direction.as_deref().unwrap_or("")),
                    "from": markdown_table_cell(related.from.as_deref().unwrap_or("")),
                    "display_contact": markdown_inline(related.from.as_deref().unwrap_or("")),
                    "subject": markdown_table_cell(related.subject.as_deref().unwrap_or("")),
                    "display_subject": markdown_inline(related.subject.as_deref().unwrap_or("")),
                    "time": markdown_table_cell(time),
                    "status": markdown_table_cell(&related.workspace.status),
                },
            }));
        }
        Ok(out)
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
                "from": related.from.as_deref().unwrap_or(""),
                "subject": related.subject.as_deref().unwrap_or(""),
                "direction": related.direction.as_deref().unwrap_or(""),
                "received_rfc3339": related.received_rfc3339.as_deref().unwrap_or(""),
                "sent_rfc3339": related.sent_rfc3339.as_deref().unwrap_or(""),
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

fn message_file_fingerprint(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).map_err(|err| AppError::io("read message metadata", &err))?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Ok(format!("file:{}:{}", metadata.len(), modified_ns))
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
