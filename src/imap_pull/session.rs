use super::*;
use crate::imap_client::ImapClientSession;

const BODY_FETCH_BATCH_SIZE: usize = 25;

pub(super) fn fetch_mailboxes(imap: &ImapConfig) -> Result<Vec<MailboxInfo>> {
    let mut session = ImapClientSession::connect(imap)?;
    session.list_mailboxes()
}

pub(super) fn uid_store_flags_with_operation(
    config: &ImapConfig,
    source_folder: &str,
    uid: u64,
    flags: &[String],
    add: bool,
) -> Result<()> {
    if flags.is_empty() {
        return Ok(());
    }
    let mut session = ImapClientSession::connect(config)?;
    session.uid_store_flags(source_folder, uid, flags, add)
}

pub(super) fn fetch_selected_mailbox_envelopes<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    since_days: Option<u32>,
) -> Result<Vec<RemoteEnvelope>> {
    let mailbox_status = session
        .examine(mailbox)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    let uid_validity = mailbox_status.uid_validity.unwrap_or(0) as u64;
    let sequence = mailbox_uid_sequence(session, since_days)?;
    if sequence.is_empty() {
        return Ok(Vec::new());
    }
    let fetches = session
        .uid_fetch(&sequence, "(UID FLAGS BODY.PEEK[HEADER])")
        .map_err(|e| AppError::new("imap_fetch_failed", e.to_string()))?;
    let mut out = Vec::new();
    for fetch in fetches.iter() {
        let Some(uid) = fetch.uid else {
            continue;
        };
        // rust-imap exposes BODY[HEADER] via header(); BODY[HEADER.FIELDS ...]
        // is parsed as a section the public Fetch API does not return.
        let Some(header) = fetch.header().or_else(|| fetch.body()) else {
            continue;
        };
        out.push(RemoteEnvelope {
            mailbox: mailbox.to_string(),
            uid_validity,
            uid: uid as u64,
            flags: canonical_flags(fetch.flags().iter().map(ToString::to_string)),
            header: header.to_vec(),
        });
    }
    Ok(out)
}

pub(super) fn fetch_current_mailbox_uid_batch<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    uid_validity: u64,
    uids: &[u64],
) -> Result<Vec<RemoteMessage>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let uid_set = uids.iter().copied().collect::<BTreeSet<_>>();
    let sequence = uids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let fetches = session
        .uid_fetch(&sequence, "(UID FLAGS RFC822.SIZE BODY.PEEK[])")
        .map_err(|e| AppError::new("imap_fetch_failed", e.to_string()))?;
    let mut out = Vec::new();
    for fetch in fetches.iter() {
        let Some(uid) = fetch.uid else {
            continue;
        };
        let uid = uid as u64;
        if !uid_set.contains(&uid) {
            continue;
        }
        let Some(body) = fetch.body() else {
            continue;
        };
        out.push(RemoteMessage {
            mailbox: mailbox.to_string(),
            uid_validity,
            uid,
            flags: canonical_flags(fetch.flags().iter().map(ToString::to_string)),
            raw_eml: body.to_vec(),
        });
    }
    Ok(out)
}

pub(super) fn mailbox_uid_sequence<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    since_days: Option<u32>,
) -> Result<String> {
    match since_days {
        Some(days) => {
            let cutoff = (Utc::now() - ChronoDuration::days(i64::from(days)))
                .format("%d-%b-%Y")
                .to_string();
            let uids = session
                .uid_search(format!("SINCE {cutoff}"))
                .map_err(|e| AppError::new("imap_search_failed", e.to_string()))?;
            if uids.is_empty() {
                return Ok(String::new());
            }
            let mut uids = uids.into_iter().collect::<Vec<_>>();
            uids.sort_unstable();
            Ok(uids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","))
        }
        None => Ok("1:*".to_string()),
    }
}

pub(super) fn fetch_uid_snapshot_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
) -> Result<FolderUidSnapshot> {
    let mailbox_status = session
        .examine(mailbox)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    let uid_validity = mailbox_status.uid_validity.unwrap_or(0) as u64;
    let fetches = session
        .uid_fetch("1:*", "UID")
        .map_err(|e| AppError::new("imap_fetch_failed", e.to_string()))?;
    let uids = fetches
        .iter()
        .filter_map(|fetch| fetch.uid)
        .map(|uid| uid as u64)
        .collect::<BTreeSet<_>>();
    Ok(FolderUidSnapshot {
        mailbox: mailbox.to_string(),
        uid_validity,
        uids,
    })
}

pub(super) fn list_folders_json<T: std::io::Read + std::io::Write>(
    config: &MailConfig,
    session: &mut imap::Session<T>,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let mailboxes = list_mailboxes(session)?;
    let selected = selected_targets_by_folder(config, &mailboxes);
    let mailbox_values = mailboxes
        .iter()
        .map(|mailbox| {
            json!({
                "mailbox_name": mailbox.name,
                "delimiter": mailbox.delimiter,
                "attributes": mailbox.attributes,
                "special_use": mailbox.special_use.map(SpecialUseKind::as_str),
                "special_use_source": mailbox.special_use.map(|_| SpecialUseSource::Rfc6154Attribute.as_str()),
                "special_use_matches": special_use_matches_for_mailbox(config, mailbox),
                "selected_for": selected.get(&mailbox.name).cloned().unwrap_or_default()
            })
        })
        .collect();
    let targets = special_use_kinds()
        .iter()
        .map(|kind| {
            let target = resolve_special_use_from_mailboxes(config, *kind, &mailboxes);
            let exists = mailboxes
                .iter()
                .any(|mailbox| mailbox.name == target.mailbox_name);
            special_use_target_json(&target, exists)
        })
        .collect();
    Ok((mailbox_values, targets))
}

pub(super) fn pull_workspace_session<T: std::io::Read + std::io::Write>(
    root: &Path,
    mail_config: &MailConfig,
    targets: &[PullTarget],
    session: &mut imap::Session<T>,
    progress: &mut Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    let started = Instant::now();
    let offset = mail_config.resolved_timezone_offset();
    let mut existing = load_existing_remote_index(root)?;
    let mut new_message_count = 0usize;
    let mut triage_created_count = 0usize;
    let mut spam_message_count = 0usize;
    let mut trashed_message_count = 0usize;
    let mut updated_location_count = 0usize;
    let mut flags_updated_count = 0usize;
    for (target_index, target) in targets.iter().enumerate() {
        emit_mailbox_progress(
            progress,
            "pull_mailbox_headers_start",
            target,
            target_index,
            targets.len(),
            json!({}),
        );
        let envelopes = fetch_selected_mailbox_envelopes(session, &target.mailbox, None)?;
        let envelope_count = envelopes.len();
        let mut mailbox_existing_location_count = 0usize;
        let mut mailbox_updated_location_count = 0usize;
        let mut mailbox_flags_updated_count = 0usize;
        let mut new_uids = BTreeSet::new();
        let mut new_envelopes = BTreeMap::new();
        for envelope in envelopes {
            let key = ImapKey {
                mailbox: envelope.mailbox.clone(),
                uid_validity: envelope.uid_validity,
                uid: envelope.uid,
            };
            if existing.imap_keys.contains(&key) {
                if let Some(existing_message_id) = existing.imap_locations.get(&key) {
                    let update = update_remote_location_flags_from_envelope(
                        root,
                        existing_message_id,
                        &envelope,
                    )?;
                    if update.flags_updated {
                        flags_updated_count += 1;
                        mailbox_flags_updated_count += 1;
                    }
                    mailbox_existing_location_count += 1;
                }
                continue;
            }
            if let Some(rfc822_message_id) = rfc822_message_id(&envelope.header) {
                let rfc822_key = normalize_message_id(&rfc822_message_id);
                if let Some(existing_message_id) = existing.rfc822_ids.get(&rfc822_key) {
                    let update = add_remote_location_from_envelope(
                        root,
                        existing_message_id,
                        &envelope,
                        Some(target.id.as_str()),
                    )?;
                    existing.imap_keys.insert(key);
                    updated_location_count += 1;
                    mailbox_updated_location_count += 1;
                    if update.flags_updated {
                        flags_updated_count += 1;
                        mailbox_flags_updated_count += 1;
                    }
                    continue;
                }
            }
            new_uids.insert(envelope.uid);
            new_envelopes.insert(envelope.uid, envelope);
        }
        emit_mailbox_progress(
            progress,
            "pull_mailbox_headers_done",
            target,
            target_index,
            targets.len(),
            json!({
                "fetched_count": envelope_count,
                "existing_location_count": mailbox_existing_location_count,
                "updated_location_count": mailbox_updated_location_count,
                "flags_updated_count": mailbox_flags_updated_count,
                "new_candidate_count": new_uids.len(),
            }),
        );
        emit_mailbox_progress(
            progress,
            "pull_mailbox_bodies_start",
            target,
            target_index,
            targets.len(),
            json!({
                "uid_count": new_uids.len(),
                "batch_size": BODY_FETCH_BATCH_SIZE,
                "batch_count": new_uids.len().div_ceil(BODY_FETCH_BATCH_SIZE),
            }),
        );
        let uid_validity = new_envelopes
            .values()
            .next()
            .map(|envelope| envelope.uid_validity)
            .unwrap_or(0);
        let uid_batches = new_uids
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .chunks(BODY_FETCH_BATCH_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        let mut fetched_body_count = 0usize;
        let mut processed_uid_count = 0usize;
        let mut mailbox_new_message_count = 0usize;
        let mut mailbox_triage_created_count = 0usize;
        let mut mailbox_spam_message_count = 0usize;
        let mut mailbox_trashed_message_count = 0usize;
        for (batch_index, batch) in uid_batches.iter().enumerate() {
            emit_mailbox_progress(
                progress,
                "pull_mailbox_bodies_progress",
                target,
                target_index,
                targets.len(),
                json!({
                    "stage": "fetch_start",
                    "uid_count": new_uids.len(),
                    "processed_count": processed_uid_count,
                    "fetched_count": fetched_body_count,
                    "batch_index": batch_index + 1,
                    "batch_count": uid_batches.len(),
                    "batch_uid_count": batch.len(),
                    "batch_size": BODY_FETCH_BATCH_SIZE,
                }),
            );
            let messages =
                fetch_current_mailbox_uid_batch(session, &target.mailbox, uid_validity, batch)?;
            processed_uid_count += batch.len();
            fetched_body_count += messages.len();
            for remote in messages {
                let Some(envelope) = new_envelopes.get(&remote.uid) else {
                    continue;
                };
                let key = ImapKey {
                    mailbox: remote.mailbox.clone(),
                    uid_validity: remote.uid_validity,
                    uid: remote.uid,
                };
                let suggestion = existing.suggest_case(&envelope.header);
                let saved =
                    save_remote_message(root, remote, &suggestion, target, offset, mail_config)?;
                existing.imap_keys.insert(key);
                if let Some(rfc822_message_id) = saved.rfc822_message_id {
                    existing
                        .rfc822_ids
                        .insert(normalize_message_id(&rfc822_message_id), saved.message_id);
                }
                new_message_count += 1;
                mailbox_new_message_count += 1;
                match target.import_as {
                    PullImportAs::Triage => {
                        triage_created_count += 1;
                        mailbox_triage_created_count += 1;
                    }
                    PullImportAs::Spam => {
                        spam_message_count += 1;
                        mailbox_spam_message_count += 1;
                    }
                    PullImportAs::Trashed => {
                        trashed_message_count += 1;
                        mailbox_trashed_message_count += 1;
                    }
                }
            }
            emit_mailbox_progress(
                progress,
                "pull_mailbox_bodies_progress",
                target,
                target_index,
                targets.len(),
                json!({
                    "stage": "fetch_done",
                    "uid_count": new_uids.len(),
                    "processed_count": processed_uid_count,
                    "fetched_count": fetched_body_count,
                    "new_message_count": mailbox_new_message_count,
                    "triage_created_count": mailbox_triage_created_count,
                    "spam_message_count": mailbox_spam_message_count,
                    "trashed_message_count": mailbox_trashed_message_count,
                    "batch_index": batch_index + 1,
                    "batch_count": uid_batches.len(),
                    "batch_uid_count": batch.len(),
                    "batch_size": BODY_FETCH_BATCH_SIZE,
                }),
            );
        }
        emit_mailbox_progress(
            progress,
            "pull_mailbox_bodies_done",
            target,
            target_index,
            targets.len(),
            json!({
                "uid_count": new_uids.len(),
                "processed_count": processed_uid_count,
                "fetched_count": fetched_body_count,
                "new_message_count": mailbox_new_message_count,
                "triage_created_count": mailbox_triage_created_count,
                "spam_message_count": mailbox_spam_message_count,
                "trashed_message_count": mailbox_trashed_message_count,
                "batch_count": uid_batches.len(),
                "batch_size": BODY_FETCH_BATCH_SIZE,
            }),
        );
    }
    Ok(json!({
        "code": "pull_result",
        "new_message_count": new_message_count,
        "triage_created_count": triage_created_count,
        "archived_message_count": 0,
        "spam_message_count": spam_message_count,
        "trashed_message_count": trashed_message_count,
        "sent_message_count": 0,
        "draft_message_count": 0,
        "indexed_message_count": 0,
        "flagged_message_count": 0,
        "reclassified_message_count": 0,
        "updated_location_count": updated_location_count,
        "flags_updated_count": flags_updated_count,
        "mailbox_ids": targets.iter().map(|target| target.id.clone()).collect::<Vec<_>>(),
        "mailbox_names": targets.iter().map(|target| target.mailbox.clone()).collect::<Vec<_>>(),
        "mailbox_count": targets.len(),
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

fn emit_mailbox_progress(
    progress: &mut Option<&mut ProgressCallback<'_>>,
    phase: &str,
    target: &PullTarget,
    target_index: usize,
    mailbox_count: usize,
    fields: Value,
) {
    let mut map = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    map.insert("mailbox_id".to_string(), json!(target.id.as_str()));
    map.insert("mailbox_name".to_string(), json!(target.mailbox.as_str()));
    map.insert("index".to_string(), json!(target_index + 1));
    map.insert("mailbox_count".to_string(), json!(mailbox_count));
    crate::progress::emit(progress, phase, Value::Object(map));
}

pub(super) fn fetch_uid_snapshots_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailboxes: &[String],
) -> Result<Vec<FolderUidSnapshot>> {
    let mut snapshots = Vec::new();
    for mailbox in mailboxes {
        snapshots.push(fetch_uid_snapshot_session(session, mailbox)?);
    }
    Ok(snapshots)
}
