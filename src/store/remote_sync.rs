use super::*;

impl Workspace {
    pub fn pull(&self, ids: &[String]) -> Result<Value> {
        self.pull_with_progress(ids, None)
    }

    pub fn pull_with_progress(
        &self,
        ids: &[String],
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
        let mut result = crate::imap_pull::pull_workspace(
            &self.root,
            &mail_config,
            &imap,
            &targets,
            progress.as_deref_mut(),
        )?;
        crate::progress::emit(
            &mut progress,
            "pull_reconcile_start",
            json!({
                "mailbox_names": imap.mailboxes.clone(),
                "mailbox_count": imap.mailboxes.len(),
            }),
        );
        let reconciliation = self.reconcile_remote_missing(&imap)?;
        let mut reconcile_progress = reconciliation.clone();
        if let Some(map) = reconcile_progress.as_object_mut() {
            map.insert("mailbox_count".to_string(), json!(imap.mailboxes.len()));
        }
        crate::progress::emit(&mut progress, "pull_reconcile_done", reconcile_progress);
        merge_reconciliation_into_pull(&mut result, &reconciliation);
        crate::progress::emit(&mut progress, "pull_render_start", json!({}));
        let triage = self.refresh_triage_views()?;
        let dispositions = self.refresh_disposition_views()?;
        merge_triage_refresh_into_pull(&mut result, &triage);
        merge_triage_refresh_into_pull(&mut result, &dispositions);
        let case_view_count = self.refresh_all_case_message_views()?;
        let archive_index_count = self.archive_message_category_ids()?.len();
        self.refresh_archive_indexes()?;
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
            map.insert("case_view_count".to_string(), json!(case_view_count));
            map.insert(
                "archive_message_category_count".to_string(),
                json!(archive_index_count),
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
                self.persist_message_remote(&message)?;
                self.write_message_materialized_cache(&message)?;
                kept_message_ids.push(message.message_id.clone());
                continue;
            }

            let id = message.message_id.clone();
            if self.message_id_is_referenced(&id)? {
                self.persist_message_remote(&message)?;
                self.write_message_materialized_cache(&message)?;
                tombstoned_message_ids.push(id);
            } else {
                message.workspace.status = MessageStatus::DeletedRemote.as_str().to_string();
                message.workspace.archive_uid = None;
                message.workspace.archived_rfc3339 = None;
                message.workspace.origin = None;
                message.workspace.remote_sync = None;
                message.workspace.push = None;
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

pub(super) fn message_remote_flags(message: &MessageFile) -> Vec<String> {
    let mut flags = Vec::new();
    if let Some(remote) = &message.remote {
        for location in &remote.locations {
            if location.missing_rfc3339.is_none() {
                flags.extend(location.flags.iter().cloned());
            }
        }
    }
    canonical_flags(flags)
}

pub(super) fn message_mailbox_ids(message: &MessageFile, config: &MailConfig) -> Vec<String> {
    let mut ids = BTreeSet::new();
    if let Some(remote) = &message.remote {
        for location in &remote.locations {
            if location.missing_rfc3339.is_some() {
                continue;
            }
            if let Some(id) = &location.mailbox_id {
                ids.insert(id.clone());
                continue;
            }
            let matches = config.matching_mailbox_ids_offline(&location.mailbox_name);
            if matches.is_empty() {
                ids.insert(location.mailbox_name.clone());
            } else {
                ids.extend(matches);
            }
        }
    }
    ids.into_iter().collect()
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
    mailbox_id: Option<String>,
    push: PushLocation,
}

pub(super) fn resolve_location_mailbox_id(
    config: &crate::config::MailConfig,
    location: &MailboxIdLocation,
) -> Result<Option<String>> {
    if let Some(id) = &location.mailbox_id {
        if config.mailboxes.contains_key(id) {
            return Ok(Some(id.clone()));
        }
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

pub(super) fn add_queue_fields(value: &mut Value, location_count: usize, item: Option<&PushItem>) {
    if let Value::Object(map) = value {
        map.insert("location_count".to_string(), json!(location_count));
        map.insert("queued_location_count".to_string(), json!(location_count));
        map.insert("queued".to_string(), json!(item.is_some()));
        map.insert(
            "push_id".to_string(),
            json!(item.map(|item| item.push_id.clone())),
        );
    }
}

impl Workspace {
    pub(super) fn queue_archive_for_archived_messages(
        &self,
        message_ids: &[String],
        allowed_push_path: Option<&Path>,
    ) -> Result<ArchiveQueue> {
        let mut eligible_message_ids = Vec::new();
        for message_id in message_ids {
            let eligibility = self.archive_eligibility(message_id, allowed_push_path)?;
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
        let mut grouped: BTreeMap<String, Vec<PushLocation>> = BTreeMap::new();
        for location in &locations {
            let Some(source_id) = resolve_location_mailbox_id(&config, location)? else {
                continue;
            };
            let Some(rule) = config
                .actions
                .message_archive
                .by_source_mailbox_id
                .get(&source_id)
            else {
                continue;
            };
            if rule.steps.is_empty() {
                continue;
            }
            grouped
                .entry(source_id)
                .or_default()
                .push(location.push.clone());
        }
        let queued_location_count = grouped.values().map(Vec::len).sum();
        let mut items = Vec::new();
        for (source_id, queue_locations) in grouped {
            let mut queue_message_ids = Vec::new();
            for location in &queue_locations {
                merge_string(&mut queue_message_ids, &location.message_id);
            }
            let Some(rule) = config
                .actions
                .message_archive
                .by_source_mailbox_id
                .get(&source_id)
            else {
                continue;
            };
            if let Some(item) = crate::push_queue::queue_action_steps(
                &self.root,
                "message.archive",
                &queue_message_ids,
                &queue_locations,
                &rule.steps,
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
                    "afmail archive message show ARCHIVE_REF"
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
        validate_id("message_id", message_id)?;
        let message = self.read_message_by_id(message_id)?;
        let cases = CaseIndex::build(self)?;
        let mut blockers = Vec::new();
        let has_archive_state = message.workspace.archive_uid.is_some()
            || message.workspace.status == "archived"
            || cases.has_archived_reference(message_id);
        if !has_archive_state {
            blockers.push(format!("messages/{message_id}.json:archive_uid"));
        }
        let ids = [message_id.to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        self.collect_active_case_references(&ids, &mut blockers)?;
        self.collect_draft_references(&ids, &mut blockers)?;
        self.collect_push_references(&ids, allowed_push_path, &mut blockers)?;
        Ok(ArchiveEligibility {
            eligible: blockers.is_empty(),
            blockers,
        })
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
                self.persist_message_remote(&message)?;
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
        let ids = message_ids.iter().cloned().collect::<BTreeSet<_>>();
        let mut references = Vec::new();
        self.collect_case_references(&ids, &mut references)?;
        self.collect_push_references(&ids, allowed_push_path, &mut references)?;
        self.collect_direct_archive_references(&ids, &mut references)?;
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
                "Remove or move the listed case/archive/push references before moving the message.",
            )
            .with_details(json!({
                "references": references,
                "suggested_commands": [
                    "afmail status",
                    "afmail push list",
                    "afmail case show CASE_REF",
                    "afmail archive message show ARCHIVE_REF"
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
            let messages_path = case_messages_json_path(&case_path);
            if messages_path.exists() {
                let data = read_to_string(&messages_path, "read case messages")?;
                if json_contains_any_id(
                    &serde_json::from_str::<Value>(&data)
                        .map_err(|e| AppError::json("parse case messages", &e))?,
                    ids,
                ) {
                    references.push(rel_path(&self.root, &messages_path));
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
        for (_, case_path) in self.all_case_entries()? {
            let messages_path = case_messages_json_path(&case_path);
            if !messages_path.exists() {
                continue;
            }
            let data = read_to_string(&messages_path, "read case messages")?;
            let value = serde_json::from_str::<Value>(&data)
                .map_err(|e| AppError::json("parse case messages", &e))?;
            if !json_contains_any_id(&value, ids) {
                continue;
            }
            if case_status(&case_path)? != "archived" {
                references.push(rel_path(&self.root, &messages_path));
            }
        }
        Ok(())
    }

    pub(super) fn collect_draft_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        for (_, case_path) in self.all_case_entries()? {
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

    pub(super) fn collect_push_references(
        &self,
        ids: &BTreeSet<String>,
        allowed_push_path: Option<&Path>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        let push_dir = self.root.join(".afmail/push");
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
            PushItem::parse_json(&data)?;
            let value = serde_json::from_str::<Value>(&data)
                .map_err(|e| AppError::json("parse push item", &e))?;
            if json_contains_any_id(&value, ids) {
                references.push(rel_path(&self.root, &path));
            }
        }
        Ok(())
    }

    pub(super) fn collect_direct_archive_references(
        &self,
        ids: &BTreeSet<String>,
        references: &mut Vec<String>,
    ) -> Result<()> {
        for message_id in ids {
            let path = self.message_path(message_id);
            let Ok(message) = self.read_message_by_id(message_id) else {
                continue;
            };
            if message.workspace.archive_uid.is_some() {
                references.push(format!("{}:archive_uid", rel_path(&self.root, &path)));
            }
        }
        Ok(())
    }
}
