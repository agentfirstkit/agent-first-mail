use super::*;

impl Workspace {
    pub(crate) fn restore_remote_deleted_message(
        &self,
        message_id: &str,
        status: MessageStatus,
    ) -> Result<bool> {
        let mut message = self.read_message_by_id(message_id)?;
        if message.workspace.status != MessageStatus::DeletedRemote.as_str() {
            return Ok(false);
        }
        message.workspace.status = status.as_str().to_string();
        self.clear_message_disposition(MessageStatus::DeletedRemote, message_id)?;
        if matches!(status, MessageStatus::Spam | MessageStatus::Trashed) {
            self.set_message_disposition(status, message_id, None, &now_rfc3339())?;
        }
        self.write_message_artifacts(&message)?;
        Ok(true)
    }

    pub fn pull(&self, ids: &[String]) -> Result<Value> {
        self.pull_with_progress(ids, crate::imap_pull::PullOptions::default(), None)
    }

    pub(crate) fn pull_with_progress(
        &self,
        ids: &[String],
        options: crate::imap_pull::PullOptions,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let mut progress = progress;
        let mail_config = crate::config::MailConfig::load(&self.root)?;
        // Validate mailbox ids before requiring network credentials so typos fail clearly.
        mail_config.selected_pull_ids(ids)?;
        let imap_base = mail_config.require_imap()?;
        let targets = crate::imap_pull::resolve_pull_targets(&mail_config, &imap_base, ids)?;
        crate::progress::emit(
            &mut progress,
            "pull_resolve_targets",
            json!({
                "requested_mailbox_ids": ids,
                "mailbox_ids": targets.iter().map(|target| target.id.clone()).collect::<Vec<_>>(),
                "mailbox_names": targets.iter().map(|target| target.mailbox.clone()).collect::<Vec<_>>(),
                "mailbox_count": targets.len(),
            }),
        );
        let imap = mail_config.require_imap_with_mailboxes(
            targets
                .iter()
                .map(|target| target.mailbox.clone())
                .collect(),
        )?;
        if options.dry_run {
            return crate::imap_pull::pull_workspace(
                &self.root,
                &mail_config,
                &imap,
                &targets,
                options,
                progress.as_deref_mut(),
            );
        }
        let mut result = crate::imap_pull::pull_workspace(
            &self.root,
            &mail_config,
            &imap,
            &targets,
            options,
            progress.as_deref_mut(),
        )?;
        let all_targets_used_incremental = crate::imap_pull::pull_used_incremental_cursors(&result);
        crate::progress::emit(
            &mut progress,
            "pull_reconcile_start",
            json!({
                "mailbox_names": imap.mailboxes.clone(),
                "mailbox_count": imap.mailboxes.len(),
            }),
        );
        // Headers and bodies are pulled incrementally, but the location check
        // runs on every pull: "no new messages" says nothing about whether an
        // old message was moved or deleted remotely, and push recovery for a
        // MOVE whose target UID was never returned depends on the source
        // location being reconciled as missing. The incremental flag still
        // decides the periodic full header rescan below.
        let reconciliation = self.reconcile_remote_missing(&imap)?;
        crate::imap_pull::commit_pull_cursors(&self.root, &result, !all_targets_used_incremental)?;
        crate::imap_pull::remove_internal_pull_fields(&mut result);
        let mut reconcile_progress = reconciliation.clone();
        if let Some(map) = reconcile_progress.as_object_mut() {
            map.insert("mailbox_count".to_string(), json!(imap.mailboxes.len()));
        }
        crate::progress::emit(&mut progress, "pull_reconcile_done", reconcile_progress);
        merge_reconciliation_into_pull(&mut result, &reconciliation);
        crate::progress::emit(&mut progress, "pull_render_start", json!({}));
        crate::progress::emit(
            &mut progress,
            "pull_render_prepare_start",
            json!({"render_stage": "prepare"}),
        );
        let prepare_started = Instant::now();
        let static_inputs = RenderStaticInputs::load(self, mail_config.clone())?;
        let preparation = self.prepare_render_context(static_inputs)?;
        crate::progress::emit(
            &mut progress,
            "pull_render_prepare_done",
            render_step_done_fields(
                "prepare",
                prepare_started,
                json!({
                    "message_cache_rebuilt_count": preparation.materialization.rebuilt_count,
                    "text_cache_removed_count": preparation.materialization.removed_text_cache_count,
                    "validated_eml_count": preparation.materialization.validated_eml_count,
                    "materialized_cache_written_count": preparation.materialization.materialized_cache_written_count,
                }),
            ),
        );
        let mut render_cache = self.read_render_cache()?;

        crate::progress::emit(
            &mut progress,
            "pull_render_triage_start",
            json!({"render_stage": "triage"}),
        );
        let render_step_started = Instant::now();
        let triage = self.refresh_triage_views_cached_with_progress(
            &mut render_cache,
            progress.as_deref_mut(),
            "pull_render_triage_progress",
            &preparation.context,
        )?;
        crate::progress::emit(
            &mut progress,
            "pull_render_triage_done",
            render_step_done_fields("triage", render_step_started, triage.clone()),
        );

        crate::progress::emit(
            &mut progress,
            "pull_render_dispositions_start",
            json!({"render_stage": "dispositions"}),
        );
        let render_step_started = Instant::now();
        let dispositions =
            self.refresh_disposition_views_cached(&mut render_cache, &preparation.context)?;
        crate::progress::emit(
            &mut progress,
            "pull_render_dispositions_done",
            render_step_done_fields("dispositions", render_step_started, dispositions.clone()),
        );
        merge_triage_refresh_into_pull(&mut result, &triage);
        merge_triage_refresh_into_pull(&mut result, &dispositions);

        crate::progress::emit(
            &mut progress,
            "pull_render_cases_start",
            json!({"render_stage": "cases"}),
        );
        let render_step_started = Instant::now();
        let case_refresh = self.refresh_all_case_message_views_cached(
            &mut render_cache,
            &preparation.context,
            progress.as_deref_mut(),
        )?;
        crate::progress::emit(
            &mut progress,
            "pull_render_cases_done",
            render_step_done_fields(
                "cases",
                render_step_started,
                case_refresh_progress_fields(&case_refresh),
            ),
        );

        crate::progress::emit(
            &mut progress,
            "pull_render_notifications_start",
            json!({"render_stage": "notifications"}),
        );
        let render_step_started = Instant::now();
        let notification_refresh = self.refresh_notification_indexes_cached(
            &mut render_cache,
            &preparation.context,
            progress.as_deref_mut(),
        )?;
        crate::progress::emit(
            &mut progress,
            "pull_render_notifications_done",
            render_step_done_fields(
                "notifications",
                render_step_started,
                notification_refresh_progress_fields(&notification_refresh),
            ),
        );

        crate::progress::emit(
            &mut progress,
            "pull_render_cache_write_start",
            json!({"render_stage": "cache_write"}),
        );
        let render_step_started = Instant::now();
        self.write_render_cache_if_changed(&mut render_cache)?;
        crate::progress::emit(
            &mut progress,
            "pull_render_cache_write_done",
            render_step_done_fields("cache_write", render_step_started, json!({})),
        );
        let mut render_done = triage.clone();
        if let (Some(render_map), Some(disposition_map)) =
            (render_done.as_object_mut(), dispositions.as_object())
        {
            for key in [
                "spam_count",
                "spam_written_count",
                "stale_spam_removed_count",
                "trash_count",
                "trash_written_count",
                "stale_trash_removed_count",
                "deleted_count",
                "deleted_written_count",
                "stale_deleted_removed_count",
            ] {
                if let Some(value) = disposition_map.get(key) {
                    render_map.insert(key.to_string(), value.clone());
                }
            }
        }
        if let Some(map) = render_done.as_object_mut() {
            map.insert(
                "case_view_count".to_string(),
                json!(case_refresh.case_index_count + case_refresh.case_index_unchanged_count),
            );
            map.insert(
                "case_view_written_count".to_string(),
                json!(case_refresh.case_index_count + case_refresh.case_message_count),
            );
            map.insert(
                "case_view_unchanged_count".to_string(),
                json!(
                    case_refresh.case_index_unchanged_count
                        + case_refresh.case_message_unchanged_count
                ),
            );
            map.insert(
                "notification_count".to_string(),
                json!(
                    notification_refresh.notification_index_count
                        + notification_refresh.notification_index_unchanged_count
                ),
            );
            map.insert(
                "notification_view_written_count".to_string(),
                json!(
                    notification_refresh.notification_index_count
                        + notification_refresh.notification_message_count
                ),
            );
            map.insert(
                "notification_view_unchanged_count".to_string(),
                json!(
                    notification_refresh.notification_index_unchanged_count
                        + notification_refresh.notification_message_unchanged_count
                ),
            );
        }
        if let Some(map) = result.as_object_mut() {
            map.insert(
                "case_view_count".to_string(),
                json!(case_refresh.case_index_count + case_refresh.case_index_unchanged_count),
            );
            map.insert(
                "case_view_written_count".to_string(),
                json!(case_refresh.case_index_count + case_refresh.case_message_count),
            );
            map.insert(
                "case_view_unchanged_count".to_string(),
                json!(
                    case_refresh.case_index_unchanged_count
                        + case_refresh.case_message_unchanged_count
                ),
            );
            map.insert(
                "notification_count".to_string(),
                json!(
                    notification_refresh.notification_index_count
                        + notification_refresh.notification_index_unchanged_count
                ),
            );
            map.insert(
                "notification_view_written_count".to_string(),
                json!(
                    notification_refresh.notification_index_count
                        + notification_refresh.notification_message_count
                ),
            );
            map.insert(
                "notification_view_unchanged_count".to_string(),
                json!(
                    notification_refresh.notification_index_unchanged_count
                        + notification_refresh.notification_message_unchanged_count
                ),
            );
        }
        crate::progress::emit(&mut progress, "pull_render_done", render_done);
        Ok(result)
    }

    fn reconcile_remote_missing(&self, config: &crate::config::ImapConfig) -> Result<Value> {
        let started = Instant::now();
        let snapshots = crate::imap_pull::fetch_uid_snapshots(config)?;
        let snapshot_by_folder = snapshots
            .iter()
            .map(|snapshot| (snapshot.mailbox.clone(), snapshot))
            .collect::<BTreeMap<_, _>>();
        let selected_folders = config.mailboxes.iter().cloned().collect::<BTreeSet<_>>();

        let mut checked_location_count = 0usize;
        let mut missing_location_count = 0usize;
        let mut deleted_remote_message_ids = Vec::new();
        let mut tombstoned_message_ids = Vec::new();
        let mut kept_message_ids = Vec::new();
        for path in message_json_paths(&self.root)? {
            let mut message = read_message(&path)?;
            let active_locations = active_remote_locations(&message, &selected_folders);
            if active_locations.is_empty() {
                continue;
            }
            checked_location_count += active_locations.len();
            let missing_locations = active_locations
                .into_iter()
                .filter(|location| remote_location_missing(location, &snapshot_by_folder))
                .collect::<Vec<_>>();
            if missing_locations.is_empty() {
                continue;
            }
            missing_location_count += missing_locations.len();
            mark_remote_locations_missing(&mut message, &missing_locations);
            let still_has_active_remote = has_any_active_remote_location(&message);
            if still_has_active_remote {
                self.persist_message_metadata(&message)?;
                self.write_message_materialized_cache(&message)?;
                kept_message_ids.push(message.message_id.clone());
                continue;
            }

            let id = message.message_id.clone();
            if self.message_id_is_referenced(&id)? {
                self.persist_message_metadata(&message)?;
                self.write_message_materialized_cache(&message)?;
                tombstoned_message_ids.push(id);
            } else {
                message.workspace.status = MessageStatus::DeletedRemote.as_str().to_string();
                message.workspace.notification_uid = None;
                message.workspace.notification_rfc3339 = None;
                message.workspace.origin = None;
                message.workspace.remote_sync = None;
                message.workspace.push = None;
                self.set_message_disposition(
                    MessageStatus::DeletedRemote,
                    &id,
                    None,
                    &now_rfc3339(),
                )?;
                self.write_message_artifacts(&message)?;
                deleted_remote_message_ids.push(id);
            }
        }

        Ok(json!({
            "checked_location_count": checked_location_count,
            "missing_location_count": missing_location_count,
            "deleted_remote_message_count": deleted_remote_message_ids.len(),
            "deleted_remote_message_ids": deleted_remote_message_ids,
            "tombstoned_message_count": tombstoned_message_ids.len(),
            "tombstoned_message_ids": tombstoned_message_ids,
            "kept_message_count": kept_message_ids.len(),
            "kept_message_ids": kept_message_ids,
            "duration_ms": started.elapsed().as_millis() as u64
        }))
    }
}

fn render_step_done_fields(stage: &str, started: Instant, mut fields: Value) -> Value {
    if let Value::Object(map) = &mut fields {
        map.insert("render_stage".to_string(), json!(stage));
        map.insert(
            "elapsed_ms".to_string(),
            json!(started.elapsed().as_millis() as u64),
        );
        return fields;
    }
    json!({
        "render_stage": stage,
        "elapsed_ms": started.elapsed().as_millis() as u64,
    })
}

fn case_refresh_progress_fields(refresh: &CaseViewRefresh) -> Value {
    json!({
        "case_view_count": refresh.case_index_count + refresh.case_index_unchanged_count,
        "case_view_written_count": refresh.case_index_count + refresh.case_message_count,
        "case_view_unchanged_count": refresh.case_index_unchanged_count + refresh.case_message_unchanged_count,
        "case_index_written_count": refresh.case_index_count,
        "case_index_unchanged_count": refresh.case_index_unchanged_count,
        "case_message_written_count": refresh.case_message_count,
        "case_message_unchanged_count": refresh.case_message_unchanged_count,
    })
}

fn notification_refresh_progress_fields(refresh: &NotificationViewRefresh) -> Value {
    json!({
        "notification_count": refresh.notification_index_count + refresh.notification_index_unchanged_count,
        "notification_view_written_count": refresh.notification_index_count + refresh.notification_message_count,
        "notification_view_unchanged_count": refresh.notification_index_unchanged_count + refresh.notification_message_unchanged_count,
        "notification_index_written_count": refresh.notification_index_count,
        "notification_index_unchanged_count": refresh.notification_index_unchanged_count,
        "notification_message_written_count": refresh.notification_message_count,
        "notification_message_unchanged_count": refresh.notification_message_unchanged_count,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LocalRemoteLocation {
    mailbox: String,
    uid_validity: u64,
    uid: u64,
}

pub(super) fn active_remote_locations(
    message: &MessageFile,
    selected_folders: &BTreeSet<String>,
) -> Vec<LocalRemoteLocation> {
    let mut out = Vec::new();
    if let Some(remote) = &message.remote {
        for location in &remote.locations {
            if location.missing_rfc3339.is_some()
                || !selected_folders.contains(&location.mailbox_name)
            {
                continue;
            }
            if let (Some(uid_validity), Some(uid)) = (location.uid_validity, location.uid) {
                push_unique_location(
                    &mut out,
                    LocalRemoteLocation {
                        mailbox: location.mailbox_name.clone(),
                        uid_validity,
                        uid,
                    },
                );
            }
        }
    }
    out
}

pub(super) fn has_any_active_remote_location(message: &MessageFile) -> bool {
    message.remote.as_ref().is_some_and(|remote| {
        remote.locations.iter().any(|location| {
            location.missing_rfc3339.is_none()
                && location.uid_validity.is_some()
                && location.uid.is_some()
        })
    })
}

pub(super) fn message_remote_missing_since_rfc3339(message: &MessageFile) -> Option<String> {
    let mut values = Vec::new();
    if let Some(remote) = &message.remote {
        for location in &remote.locations {
            if let Some(missing) = &location.missing_rfc3339 {
                values.push(missing.clone());
            }
        }
    }
    values.sort();
    values.into_iter().next()
}

pub(super) fn message_remote_missing(message: &MessageFile) -> bool {
    message_remote_missing_since_rfc3339(message).is_some()
        && !has_any_active_remote_location(message)
}

pub(super) fn message_remote_effect_pending(message: &MessageFile) -> bool {
    message
        .workspace
        .push
        .as_ref()
        .is_some_and(|push| !push.pending.is_empty())
}

pub(super) fn push_unique_location(
    locations: &mut Vec<LocalRemoteLocation>,
    location: LocalRemoteLocation,
) {
    if !locations.iter().any(|existing| existing == &location) {
        locations.push(location);
    }
}

pub(super) fn remote_location_missing(
    location: &LocalRemoteLocation,
    snapshots: &BTreeMap<String, &crate::imap_pull::FolderUidSnapshot>,
) -> bool {
    let Some(snapshot) = snapshots.get(&location.mailbox) else {
        return false;
    };
    snapshot.uid_validity != location.uid_validity || !snapshot.uids.contains(&location.uid)
}

pub(super) fn mark_remote_locations_missing(
    message: &mut MessageFile,
    missing: &[LocalRemoteLocation],
) {
    let now = now_rfc3339();
    let remote = message
        .remote
        .get_or_insert_with(|| crate::types::RemoteState {
            locations: Vec::new(),
        });
    for missing_location in missing {
        if let Some(location) = remote.locations.iter_mut().find(|location| {
            location.mailbox_name == missing_location.mailbox
                && location.uid_validity == Some(missing_location.uid_validity)
                && location.uid == Some(missing_location.uid)
        }) {
            if location.missing_rfc3339.is_none() {
                location.missing_rfc3339 = Some(now.clone());
            }
        } else {
            remote.locations.push(crate::types::RemoteLocation {
                mailbox_id: None,
                mailbox_name: missing_location.mailbox.clone(),
                uid_validity: Some(missing_location.uid_validity),
                uid: Some(missing_location.uid),
                flags: Vec::new(),
                observed_rfc3339: now.clone(),
                missing_rfc3339: Some(now.clone()),
            });
        }
    }
}

#[derive(Debug)]
pub(super) struct ArchiveQueue {
    pub(super) eligible_message_ids: Vec<String>,
    pub(super) location_count: usize,
    pub(super) queued_location_count: usize,
    pub(super) items: Vec<crate::types::PushItem>,
}

#[derive(Debug)]
pub(super) struct ArchiveEligibility {
    eligible: bool,
    blockers: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct MailboxIdLocation {
    pub(super) mailbox_id: Option<String>,
    pub(super) push: PushLocation,
}

pub(super) fn resolve_location_mailbox_id(
    config: &crate::config::MailConfig,
    location: &MailboxIdLocation,
) -> Result<Option<String>> {
    if let Some(id) = &location.mailbox_id
        && config.mailboxes.contains_key(id)
    {
        return Ok(Some(id.clone()));
    }
    let matches = config.matching_mailbox_ids_offline(&location.push.mailbox_name);
    match matches.as_slice() {
        [id] => Ok(Some(id.clone())),
        [] => Ok(None),
        _ => Err(AppError::new(
            "imap_mailbox_ambiguous",
            format!(
                "remote mailbox {} matches multiple mailbox ids: {}",
                location.push.mailbox_name,
                matches.join(", ")
            ),
        )),
    }
}

fn mailbox_id_matches_special_use(
    config: &crate::config::MailConfig,
    mailbox_id: &str,
    kind: SpecialUseKind,
) -> bool {
    mailbox_id == kind.as_str()
        || config
            .mailbox(mailbox_id)
            .ok()
            .is_some_and(|mailbox| mailbox.contains_special_use(kind))
}

pub(super) fn add_queue_fields(value: &mut Value, location_count: usize, item: Option<&PushItem>) {
    if let Value::Object(map) = value {
        map.insert("location_count".to_string(), json!(location_count));
        map.insert(
            "queued_location_count".to_string(),
            json!(if item.is_some() { location_count } else { 0 }),
        );
        map.insert("queued".to_string(), json!(item.is_some()));
        map.insert(
            "push_id".to_string(),
            json!(item.map(|item| item.push_id.clone())),
        );
    }
}

impl Workspace {
    pub(crate) fn plan_message_push(
        &self,
        message_id: &str,
        action: MessagePushAction,
        allowed_push_path: Option<&Path>,
        config: &crate::config::MailConfig,
    ) -> Result<Option<MessageActionPush>> {
        validate_id("message_id", message_id)?;
        let message = self.read_message_by_id(message_id)?;
        let status = MessageStatus::parse(&message.workspace.status)?;
        let message_ids = vec![message_id.to_string()];
        let valid_status = match action {
            MessagePushAction::Spam => status == MessageStatus::Spam,
            MessagePushAction::Trash => status == MessageStatus::Trashed,
            MessagePushAction::Unspam | MessagePushAction::Untrash => {
                status == MessageStatus::Triage
            }
            MessagePushAction::CaseAdd => status == MessageStatus::Case,
            MessagePushAction::Notification => {
                matches!(
                    status,
                    MessageStatus::Notification | MessageStatus::Archived
                )
            }
        };
        if !valid_status {
            return Err(AppError::new(
                "push_state_mismatch",
                format!(
                    "queued action {} does not match canonical message status {} for {message_id}",
                    action.kind(),
                    status.as_str()
                ),
            )
            .with_details(json!({
                "message_id": message_id,
                "action": action.kind(),
                "status": status.as_str(),
            })));
        }
        match action {
            MessagePushAction::Spam => {
                let locations = self.message_remote_locations_except_mailbox_kinds(
                    &message_ids,
                    &[SpecialUseKind::Junk],
                )?;
                Ok(message_action_plan(
                    MessagePushAction::Spam,
                    message_ids,
                    locations,
                    config.actions.message.spam.steps.clone(),
                ))
            }
            MessagePushAction::Trash => {
                let locations = self.message_remote_locations_except_mailbox_kinds(
                    &message_ids,
                    &[SpecialUseKind::Trash],
                )?;
                Ok(message_action_plan(
                    MessagePushAction::Trash,
                    message_ids,
                    locations,
                    config.actions.message.trash.steps.clone(),
                ))
            }
            MessagePushAction::Unspam => {
                let locations = self.message_remote_locations_for_mailbox_kinds(
                    &message_ids,
                    &[SpecialUseKind::Junk],
                )?;
                Ok(message_action_plan(
                    MessagePushAction::Unspam,
                    message_ids,
                    locations,
                    config.actions.message.unspam.steps.clone(),
                ))
            }
            MessagePushAction::Untrash => {
                let locations = self.message_remote_locations_for_mailbox_kinds(
                    &message_ids,
                    &[SpecialUseKind::Trash],
                )?;
                Ok(message_action_plan(
                    MessagePushAction::Untrash,
                    message_ids,
                    locations,
                    config.actions.message.untrash.steps.clone(),
                ))
            }
            MessagePushAction::CaseAdd => {
                let locations = self.message_remote_locations_any(&message_ids)?;
                Ok(message_action_plan(
                    MessagePushAction::CaseAdd,
                    message_ids,
                    locations,
                    config.actions.case.add.steps.clone(),
                ))
            }
            MessagePushAction::Notification => {
                self.plan_archive_message_push(message_id, allowed_push_path, config)
            }
        }
    }

    fn plan_archive_message_push(
        &self,
        message_id: &str,
        allowed_push_path: Option<&Path>,
        config: &crate::config::MailConfig,
    ) -> Result<Option<MessageActionPush>> {
        let message_ids = vec![message_id.to_string()];
        let archiving_case_uid = self.archiving_case_uid_for_message(message_id)?;
        let eligibility = self.archive_eligibility_inner(
            message_id,
            allowed_push_path,
            archiving_case_uid.as_deref(),
        )?;
        if !eligibility.eligible {
            return Ok(None);
        }
        let locations = self.message_remote_locations_with_mailbox_ids(&message_ids)?;
        let (selected_locations, selected_steps) =
            resolve_archive_action(config, &locations, message_id)?;
        Ok(message_action_plan(
            MessagePushAction::Notification,
            message_ids,
            selected_locations,
            selected_steps,
        ))
    }

    pub(super) fn queue_archive_for_archived_messages(
        &self,
        message_ids: &[String],
        allowed_push_path: Option<&Path>,
        archiving_case_uid: Option<&str>,
    ) -> Result<ArchiveQueue> {
        let mut eligible_message_ids = Vec::new();
        for message_id in message_ids {
            let eligibility =
                self.archive_eligibility_inner(message_id, allowed_push_path, archiving_case_uid)?;
            self.write_archive_sync_state(message_id, eligibility.eligible)?;
            if eligibility.eligible {
                eligible_message_ids.push(message_id.clone());
            }
        }
        if eligible_message_ids.is_empty() {
            return Ok(ArchiveQueue {
                eligible_message_ids,
                location_count: 0,
                queued_location_count: 0,
                items: Vec::new(),
            });
        }
        let config = crate::config::MailConfig::load(&self.root)?;
        let locations = self.message_remote_locations_with_mailbox_ids(&eligible_message_ids)?;
        let mut plans = Vec::new();
        for message_id in &eligible_message_ids {
            let message_locations = locations
                .iter()
                .filter(|location| location.push.message_id == *message_id)
                .cloned()
                .collect::<Vec<_>>();
            let (queue_locations, steps) =
                resolve_archive_action(&config, &message_locations, message_id)?;
            if !queue_locations.is_empty() && !steps.is_empty() {
                plans.push((message_id.clone(), queue_locations, steps));
            }
        }

        for message_id in &eligible_message_ids {
            let removed = crate::push_queue::remove_pending_message_pushes(
                &self.root,
                message_id,
                MessagePushAction::CaseAdd.kind(),
            )?;
            let removed_push_ids = removed
                .iter()
                .map(|item| item.push_id.clone())
                .collect::<Vec<_>>();
            self.clear_message_pending_pushes(message_id, &removed_push_ids, false)?;
        }

        let queued_location_count = plans
            .iter()
            .map(|(_, queue_locations, _)| queue_locations.len())
            .sum();
        let mut items = Vec::new();
        for (message_id, queue_locations, steps) in plans {
            if let Some(item) = crate::push_queue::queue_action_steps(
                &self.root,
                "message.notification",
                std::slice::from_ref(&message_id),
                &queue_locations,
                &steps,
                None,
            )? {
                self.record_pending_push_item(&item)?;
                items.push(item);
            }
        }
        Ok(ArchiveQueue {
            eligible_message_ids,
            location_count: locations.len(),
            queued_location_count,
            items,
        })
    }

    pub(super) fn message_remote_locations_with_mailbox_ids(
        &self,
        message_ids: &[String],
    ) -> Result<Vec<MailboxIdLocation>> {
        let mut locations = Vec::new();
        for message_id in message_ids {
            validate_id("message_id", message_id)?;
            let message = self.read_message_by_id(message_id)?;
            if let Some(remote) = message.remote {
                for location in remote.locations {
                    if location.missing_rfc3339.is_some() {
                        continue;
                    }
                    if let (Some(uid_validity), Some(uid)) = (location.uid_validity, location.uid) {
                        locations.push(MailboxIdLocation {
                            mailbox_id: location.mailbox_id,
                            push: PushLocation {
                                message_id: message_id.clone(),
                                mailbox_name: location.mailbox_name,
                                uid_validity,
                                uid,
                            },
                        });
                    }
                }
            }
        }
        Ok(locations)
    }

    pub(super) fn message_remote_locations_for_mailbox_kinds(
        &self,
        message_ids: &[String],
        kinds: &[SpecialUseKind],
    ) -> Result<Vec<PushLocation>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let config = crate::config::MailConfig::load(&self.root)?;
        let mut locations = Vec::new();
        for location in self.message_remote_locations_with_mailbox_ids(message_ids)? {
            let Some(source_id) = resolve_location_mailbox_id(&config, &location)? else {
                continue;
            };
            if kinds
                .iter()
                .any(|kind| mailbox_id_matches_special_use(&config, &source_id, *kind))
            {
                locations.push(location.push);
            }
        }
        Ok(locations)
    }

    pub(super) fn message_remote_locations_except_mailbox_kinds(
        &self,
        message_ids: &[String],
        kinds: &[SpecialUseKind],
    ) -> Result<Vec<PushLocation>> {
        if kinds.is_empty() {
            return self.message_remote_locations(message_ids);
        }
        let config = crate::config::MailConfig::load(&self.root)?;
        let mut locations = Vec::new();
        for location in self.message_remote_locations_with_mailbox_ids(message_ids)? {
            let Some(source_id) = resolve_location_mailbox_id(&config, &location)? else {
                locations.push(location.push);
                continue;
            };
            if !kinds
                .iter()
                .any(|kind| mailbox_id_matches_special_use(&config, &source_id, *kind))
            {
                locations.push(location.push);
            }
        }
        Ok(locations)
    }

    pub(crate) fn ensure_archive_eligible(
        &self,
        message_ids: &[String],
        allowed_push_path: Option<&Path>,
    ) -> Result<()> {
        let mut blockers = Vec::new();
        for message_id in message_ids {
            let eligibility = self.archive_eligibility(message_id, allowed_push_path)?;
            self.write_archive_sync_state(message_id, eligibility.eligible)?;
            if !eligibility.eligible {
                blockers.extend(eligibility.blockers);
            }
        }
        if blockers.is_empty() {
            Ok(())
        } else {
            Err(AppError::new(
                "message_referenced",
                format!(
                    "message id cannot be archived remotely while blocked by {}",
                    blockers.join(", ")
                ),
            )
            .with_hint(
                "Resolve the listed local references before pushing the remote archive move.",
            )
            .with_details(json!({
                "blockers": blockers,
                "suggested_commands": [
                    "afmail status",
                    "afmail push list",
                    "afmail case show CASE_REF",
                    "afmail notification show NOTIFICATION_REF"
                ]
            })))
        }
    }

    pub(super) fn write_archive_sync_state(&self, message_id: &str, eligible: bool) -> Result<()> {
        let mut msg = self.read_message_by_id(message_id)?;
        msg.workspace.remote_sync = Some(RemoteSyncState {
            archive_eligible: eligible,
            checked_rfc3339: now_rfc3339(),
        });
        self.write_message_materialized_cache(&msg)
    }

    pub(super) fn archive_eligibility(
        &self,
        message_id: &str,
        allowed_push_path: Option<&Path>,
    ) -> Result<ArchiveEligibility> {
        self.archive_eligibility_inner(message_id, allowed_push_path, None)
    }

    pub(super) fn preflight_archive_eligibility(
        &self,
        message_ids: &[String],
        archiving_case_uid: Option<&str>,
    ) -> Result<()> {
        let mut eligible_message_ids = Vec::new();
        for message_id in message_ids {
            let eligibility =
                self.archive_eligibility_inner(message_id, None, archiving_case_uid)?;
            if eligibility.eligible {
                eligible_message_ids.push(message_id.clone());
            }
        }
        self.preflight_archive_actions(&eligible_message_ids)
    }

    pub(super) fn preflight_archive_actions(&self, message_ids: &[String]) -> Result<()> {
        let config = crate::config::MailConfig::load(&self.root)?;
        for message_id in message_ids {
            let locations =
                self.message_remote_locations_with_mailbox_ids(std::slice::from_ref(message_id))?;
            resolve_archive_action(&config, &locations, message_id)?;
        }
        Ok(())
    }

    fn archive_eligibility_inner(
        &self,
        message_id: &str,
        allowed_push_path: Option<&Path>,
        archiving_case_uid: Option<&str>,
    ) -> Result<ArchiveEligibility> {
        validate_id("message_id", message_id)?;
        let message = self.read_message_by_id(message_id)?;
        let cases = CaseIndex::build(self)?;
        let mut blockers = Vec::new();
        let has_archive_state = message.workspace.notification_uid.is_some()
            || message.workspace.status == "archived"
            || cases.has_archived_reference(message_id);
        if !has_archive_state {
            blockers.push(format!("messages/{message_id}.json:notification_uid"));
        }
        let ids = [message_id.to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        self.collect_active_case_references(&ids, &mut blockers)?;
        self.collect_draft_references_except_case(&ids, archiving_case_uid, &mut blockers)?;
        self.collect_push_references_except_case(
            &ids,
            allowed_push_path,
            archiving_case_uid,
            &mut blockers,
        )?;
        Ok(ArchiveEligibility {
            eligible: blockers.is_empty(),
            blockers,
        })
    }

    fn archiving_case_uid_for_message(&self, message_id: &str) -> Result<Option<String>> {
        for entry in self.archived_case_entries()? {
            let case = read_case_file(&entry.path)?;
            if case.status == "archiving" && case.contains_message(message_id) {
                return Ok(Some(entry.case_uid));
            }
        }
        Ok(None)
    }

    pub(crate) fn message_remote_locations(
        &self,
        message_ids: &[String],
    ) -> Result<Vec<PushLocation>> {
        self.message_remote_locations_inner(message_ids, true)
    }

    pub(crate) fn message_remote_locations_any(
        &self,
        message_ids: &[String],
    ) -> Result<Vec<PushLocation>> {
        self.message_remote_locations_inner(message_ids, false)
    }

    pub(super) fn message_remote_locations_inner(
        &self,
        message_ids: &[String],
        inbound_only: bool,
    ) -> Result<Vec<PushLocation>> {
        let mut locations = Vec::new();
        for message_id in message_ids {
            validate_id("message_id", message_id)?;
            let message = self.read_message_by_id(message_id)?;
            if inbound_only && message.direction.as_deref() != Some("inbound") {
                continue;
            }
            if let Some(remote) = message.remote {
                for location in remote.locations {
                    if location.missing_rfc3339.is_some() {
                        continue;
                    }
                    if let (Some(uid_validity), Some(uid)) = (location.uid_validity, location.uid) {
                        locations.push(PushLocation {
                            message_id: message_id.clone(),
                            mailbox_name: location.mailbox_name,
                            uid_validity,
                            uid,
                        });
                    }
                }
            }
        }
        Ok(locations)
    }

    pub(crate) fn add_remote_flags(
        &self,
        locations: &[PushLocation],
        flags: &[String],
    ) -> Result<()> {
        self.update_remote_flags(locations, flags, true)
    }

    pub(super) fn update_remote_flags(
        &self,
        locations: &[PushLocation],
        flags: &[String],
        add: bool,
    ) -> Result<()> {
        let flags = canonical_flags(flags.iter().cloned());
        for location in locations {
            validate_id("message_id", &location.message_id)?;
            let mut message = self.read_message_by_id(&location.message_id)?;
            let state = message.remote.get_or_insert_with(|| RemoteState {
                locations: Vec::new(),
            });
            let changed = if let Some(remote_location) =
                state.locations.iter_mut().find(|remote_location| {
                    remote_location.mailbox_name == location.mailbox_name
                        && remote_location.uid_validity == Some(location.uid_validity)
                        && remote_location.uid == Some(location.uid)
                }) {
                if add {
                    merge_flags(&mut remote_location.flags, &flags)
                } else {
                    remove_flags(&mut remote_location.flags, &flags)
                }
            } else if add {
                state.locations.push(RemoteLocation {
                    mailbox_id: None,
                    mailbox_name: location.mailbox_name.clone(),
                    uid_validity: Some(location.uid_validity),
                    uid: Some(location.uid),
                    flags: flags.clone(),
                    observed_rfc3339: now_rfc3339(),
                    missing_rfc3339: None,
                });
                true
            } else {
                false
            };
            if changed {
                self.persist_message_metadata(&message)?;
                self.write_message_materialized_cache(&message)?;
            }
        }
        Ok(())
    }

    pub(super) fn message_id_is_referenced(&self, message_id: &str) -> Result<bool> {
        match self.ensure_message_ids_unreferenced(&[message_id.to_string()], None) {
            Ok(()) => Ok(false),
            Err(err) if err.error_code == "message_referenced" => Ok(true),
            Err(err) => Err(err),
        }
    }

    pub(crate) fn ensure_message_ids_unreferenced(
        &self,
        message_ids: &[String],
        allowed_push_path: Option<&Path>,
    ) -> Result<()> {
        self.ensure_message_ids_unreferenced_except_message_pushes(
            message_ids,
            allowed_push_path,
            &[],
        )
    }

    pub(crate) fn ensure_message_ids_unreferenced_except_message_pushes(
        &self,
        message_ids: &[String],
        allowed_push_path: Option<&Path>,
        allowed_push_actions: &[crate::types::MessagePushAction],
    ) -> Result<()> {
        let ids = message_ids.iter().cloned().collect::<BTreeSet<_>>();
        let mut references = Vec::new();
        self.collect_case_references(&ids, &mut references)?;
        self.collect_push_references_except_message_pushes(
            &ids,
            allowed_push_path,
            allowed_push_actions,
            &mut references,
        )?;
        self.collect_notification_references(&ids, &mut references)?;
        if references.is_empty() {
            Ok(())
        } else {
            Err(AppError::new(
                "message_referenced",
                format!(
                    "message id cannot be moved while referenced by {}",
                    references.join(", ")
                ),
            )
            .with_hint(
                "Remove or move the listed case/archived-case/push references before moving the message.",
            )
            .with_details(json!({
                "references": references,
                "suggested_commands": [
                    "afmail status",
                    "afmail push list",
                    "afmail case show CASE_REF",
                    "afmail notification show NOTIFICATION_REF"
                ]
            })))
        }
    }

    pub(super) fn collect_case_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        self.collect_any_case_message_references(ids, references)?;
        self.collect_draft_references(ids, references)
    }

    pub(super) fn collect_any_case_message_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        for (_, case_path) in self.all_case_entries()? {
            let data_path = case_json_path(&case_path);
            if data_path.exists() {
                let data = read_to_string(&data_path, "read case metadata")?;
                if json_contains_any_id(
                    &serde_json::from_str::<Value>(&data)
                        .map_err(|e| AppError::json("parse case metadata", &e))?,
                    ids,
                ) {
                    references.push(rel_path(&self.root, &data_path));
                }
            }
        }
        Ok(())
    }

    pub(super) fn collect_active_case_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        for (_, case_path) in self.case_entries()? {
            let data_path = case_json_path(&case_path);
            if !data_path.exists() {
                continue;
            }
            let data = read_to_string(&data_path, "read case metadata")?;
            let value = serde_json::from_str::<Value>(&data)
                .map_err(|e| AppError::json("parse case metadata", &e))?;
            if json_contains_any_id(&value, ids) {
                references.push(rel_path(&self.root, &data_path));
            }
        }
        Ok(())
    }

    pub(super) fn collect_draft_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        self.collect_draft_references_except_case(ids, None, references)
    }

    fn collect_draft_references_except_case(
        &self,
        ids: &BTreeSet<String>,
        ignored_case_uid: Option<&str>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        for (case_uid, case_path) in self.all_case_entries()? {
            if ignored_case_uid.is_some_and(|ignored| ignored == case_uid) {
                continue;
            }
            let drafts_dir = case_path.join("drafts");
            if drafts_dir.exists() {
                for entry in read_dir(&drafts_dir, "read drafts directory")? {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("md") {
                        continue;
                    }
                    let text = read_to_string(&path, "read draft")?;
                    let (fm, _) = read_doc::<DraftFrontmatter>(&text)?;
                    if fm
                        .reply_to_message_id
                        .as_ref()
                        .is_some_and(|id| ids.contains(id))
                    {
                        references.push(rel_path(&self.root, &path));
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_push_references_except_case(
        &self,
        ids: &BTreeSet<String>,
        allowed_push_path: Option<&Path>,
        ignored_case_uid: Option<&str>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        let push_dir = self
            .root
            .join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
        if !push_dir.exists() {
            return Ok(());
        }
        for entry in read_dir(&push_dir, "read push queue")? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json")
                || allowed_push_path.is_some_and(|allowed| allowed == path)
            {
                continue;
            }
            let item = PushItem::parse_json(&read_to_string(&path, "read push item")?)?;
            if item
                .message()
                .is_some_and(|message| message.action == MessagePushAction::CaseAdd)
            {
                continue;
            }
            if ignored_case_uid
                .is_some_and(|ignored| item.draft().is_some_and(|draft| draft.case_uid == ignored))
            {
                continue;
            }
            if item.message_ids().iter().any(|id| ids.contains(id)) {
                references.push(rel_path(&self.root, &path));
            }
        }
        Ok(())
    }

    pub(super) fn collect_push_references_except_message_pushes(
        &self,
        ids: &BTreeSet<String>,
        allowed_push_path: Option<&Path>,
        allowed_push_actions: &[crate::types::MessagePushAction],
        references: &mut Vec<String>,
    ) -> Result<()> {
        let push_dir = self
            .root
            .join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
        if !push_dir.exists() {
            return Ok(());
        }
        for entry in read_dir(&push_dir, "read push queue")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json")
                || allowed_push_path.is_some_and(|allowed| allowed == path)
            {
                continue;
            }
            let data = read_to_string(&path, "read push item")?;
            let item = PushItem::parse_json(&data)?;
            if message_push_action_is_allowed(&item, ids, allowed_push_actions) {
                continue;
            }
            let value = serde_json::from_str::<Value>(&data)
                .map_err(|e| AppError::json("parse push item", &e))?;
            if json_contains_any_id(&value, ids) {
                references.push(rel_path(&self.root, &path));
            }
        }
        Ok(())
    }

    pub(super) fn collect_notification_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        for message_id in ids {
            let path = self.message_path(message_id);
            let Ok(message) = self.read_message_by_id(message_id) else {
                continue;
            };
            if message.workspace.notification_uid.is_some() {
                references.push(format!("{}:notification_uid", rel_path(&self.root, &path)));
            }
        }
        Ok(())
    }
}

fn message_action_plan(
    action: MessagePushAction,
    message_ids: Vec<String>,
    locations: Vec<PushLocation>,
    steps: Vec<crate::config::ActionStep>,
) -> Option<MessageActionPush> {
    if locations.is_empty() || steps.is_empty() {
        return None;
    }
    Some(MessageActionPush {
        action,
        message_ids,
        locations,
        steps,
        reply_to_message_id: None,
    })
}

fn resolve_archive_action(
    config: &crate::config::MailConfig,
    locations: &[MailboxIdLocation],
    message_id: &str,
) -> Result<(Vec<PushLocation>, Vec<crate::config::ActionStep>)> {
    let mut selected_rule: Option<(String, Vec<crate::config::ActionStep>)> = None;
    let mut selected_locations = Vec::new();
    for location in locations {
        let Some(source_id) = resolve_location_mailbox_id(config, location)? else {
            continue;
        };
        let Some(rule) = config
            .actions
            .message
            .notification
            .by_source_mailbox_id
            .get(&source_id)
        else {
            continue;
        };
        if rule.steps.is_empty() {
            continue;
        }
        if let Some((selected_source_id, selected_steps)) = &selected_rule {
            if selected_steps != &rule.steps {
                return Err(AppError::new(
                    "archive_action_conflict",
                    format!(
                        "message {message_id} has remote locations whose archive actions differ"
                    ),
                )
                .with_hint(
                    "Configure the source mailboxes with the same notification archive steps, or archive the locations manually before retrying.",
                )
                .with_details(json!({
                    "message_id": message_id,
                    "source_mailbox_ids": [selected_source_id, &source_id],
                    "remote_calls_started": false,
                })));
            }
        } else {
            selected_rule = Some((source_id, rule.steps.clone()));
        }
        selected_locations.push(location.push.clone());
    }
    Ok((
        selected_locations,
        selected_rule.map(|(_, steps)| steps).unwrap_or_default(),
    ))
}

fn message_push_action_is_allowed(
    item: &PushItem,
    ids: &BTreeSet<String>,
    allowed_push_actions: &[crate::types::MessagePushAction],
) -> bool {
    if allowed_push_actions.is_empty() {
        return false;
    }
    if item.has_started_effects() {
        return false;
    }
    item.message().is_some_and(|payload| {
        allowed_push_actions.contains(&payload.action) && ids.contains(&payload.message_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageAuthentication;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn uidvalidity_mismatch_marks_remote_location_missing() {
        let snapshot = crate::imap_pull::FolderUidSnapshot {
            mailbox: "INBOX".to_string(),
            uid_validity: 99,
            uids: BTreeSet::from([7u64]),
        };
        let mut snapshots = BTreeMap::new();
        snapshots.insert("INBOX".to_string(), &snapshot);

        let location = LocalRemoteLocation {
            mailbox: "INBOX".to_string(),
            uid_validity: 44,
            uid: 7,
        };
        assert!(remote_location_missing(&location, &snapshots));

        let mut message = MessageFile {
            schema_name: "message".to_string(),
            schema_version: MESSAGE_SCHEMA_VERSION,
            message_id: "message_uidvalidity".to_string(),
            rfc822_message_id: Some("<message_uidvalidity@example.com>".to_string()),
            in_reply_to: None,
            references: Vec::new(),
            remote: Some(RemoteState {
                locations: vec![RemoteLocation {
                    mailbox_name: "INBOX".to_string(),
                    mailbox_id: Some("inbox".to_string()),
                    uid_validity: Some(44),
                    uid: Some(7),
                    flags: Vec::new(),
                    observed_rfc3339: "2026-05-21T10:00:00Z".to_string(),
                    missing_rfc3339: None,
                }],
            }),
            agent_suggestion: None,
            human_suggestion: None,
            case_relationship_suggestion: None,
            direction: Some("inbound".to_string()),
            subject: Some("UIDVALIDITY".to_string()),
            from: Some("alice@example.com".to_string()),
            to: vec!["me@example.com".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            sender: None,
            delivered_to: Vec::new(),
            x_original_to: Vec::new(),
            envelope_to: Vec::new(),
            list_id: None,
            mailing_list_headers: Vec::new(),
            authentication: MessageAuthentication::default(),
            received_rfc3339: Some("2026-05-21T10:00:00Z".to_string()),
            sent_rfc3339: None,
            body_text: "Body".to_string(),
            body_markdown: None,
            eml_path: Some(".afmail/messages/message_uidvalidity.eml".to_string()),
            attachments: Vec::new(),
            contact: None,
            identity: None,
            identity_email: None,
            identity_match: None,
            identity_candidates: Vec::new(),
            observed_recipient_emails: Vec::new(),
            workspace: WorkspaceState {
                status: "triage".to_string(),
                notification_uid: None,
                notification_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
        };

        mark_remote_locations_missing(&mut message, &[location]);
        assert!(message_remote_missing(&message));
        assert!(
            message
                .remote
                .as_ref()
                .and_then(|remote| remote.locations.first())
                .and_then(|remote| remote.missing_rfc3339.as_ref())
                .is_some()
        );
    }
}
