use super::*;
use crate::imap_client::ImapClientSession;

const BODY_FETCH_BATCH_SIZE: usize = 25;
const PULL_RESULT_MESSAGE_ID_LIMIT: usize = 50;
const PULL_CURSOR_SCHEMA_NAME: &str = "pull_cursors";
const PULL_CURSOR_SCHEMA_VERSION: u32 = 1;
const FULL_RECONCILE_INTERVAL: u64 = 10;

/// Safety valve: refuse to download/parse/store a single message whose
/// server-reported `RFC822.SIZE` exceeds this. A hostile or misbehaving IMAP
/// server cannot drive an unbounded allocation (and the later full-file read +
/// mail-parser parse) from one pathological message; the pull fails with a clear
/// error naming the UID instead of OOM-ing. Generous so real mail is unaffected.
const MAX_MESSAGE_BYTES: u32 = 100 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PullCursorFile {
    schema_name: String,
    schema_version: u32,
    mailboxes: BTreeMap<String, PullMailboxCursor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PullMailboxCursor {
    mailbox_id: String,
    mailbox_name: String,
    uid_validity: u64,
    highest_uid: u64,
    pull_count_since_full_reconcile: u64,
    updated_rfc3339: String,
}

#[derive(Clone, Copy, Debug)]
struct PullCursorView {
    uid_validity: u64,
    highest_uid: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PullCursorUpdate {
    mailbox_id: String,
    mailbox_name: String,
    uid_validity: u64,
    highest_uid: u64,
}

struct MailboxEnvelopeFetch {
    uid_validity: u64,
    envelopes: Vec<RemoteEnvelope>,
    cursor_used: bool,
    uid_validity_reset: bool,
    previous_highest_uid: Option<u64>,
}

fn max_message_bytes() -> u32 {
    if cfg!(debug_assertions) {
        return std::env::var("AFMAIL_TEST_MAX_MESSAGE_BYTES")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0 && *value <= MAX_MESSAGE_BYTES)
            .unwrap_or(MAX_MESSAGE_BYTES);
    }
    MAX_MESSAGE_BYTES
}

fn pull_cursor_path(root: &Path) -> std::path::PathBuf {
    root.join(crate::layout::afmail_path(
        crate::layout::AFMAIL_PULL_CURSORS,
    ))
}

fn empty_pull_cursors() -> PullCursorFile {
    PullCursorFile {
        schema_name: PULL_CURSOR_SCHEMA_NAME.to_string(),
        schema_version: PULL_CURSOR_SCHEMA_VERSION,
        mailboxes: BTreeMap::new(),
    }
}

fn load_pull_cursors(root: &Path) -> PullCursorFile {
    let path = pull_cursor_path(root);
    let Ok(data) = fs::read_to_string(path) else {
        return empty_pull_cursors();
    };
    let Ok(cursors) = serde_json::from_str::<PullCursorFile>(&data) else {
        return empty_pull_cursors();
    };
    if cursors.schema_name != PULL_CURSOR_SCHEMA_NAME
        || cursors.schema_version != PULL_CURSOR_SCHEMA_VERSION
    {
        return empty_pull_cursors();
    }
    cursors
}

fn pull_cursor_for_target(cursors: &PullCursorFile, target: &PullTarget) -> Option<PullCursorView> {
    let cursor = cursors.mailboxes.get(&target.id)?;
    if cursor.mailbox_id != target.id || cursor.mailbox_name != target.mailbox {
        return None;
    }
    if cursor.pull_count_since_full_reconcile >= FULL_RECONCILE_INTERVAL {
        return None;
    }
    Some(PullCursorView {
        uid_validity: cursor.uid_validity,
        highest_uid: cursor.highest_uid,
    })
}

pub(super) fn commit_pull_cursor_updates(
    root: &Path,
    result: &Value,
    full_reconcile: bool,
) -> Result<()> {
    let Some(updates) = result
        .get("__afmail_pull_cursor_updates")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    if updates.is_empty() {
        return Ok(());
    }
    let mut cursors = load_pull_cursors(root);
    let now = crate::store::now_rfc3339();
    for update in updates {
        let update = serde_json::from_value::<PullCursorUpdate>(update.clone())
            .map_err(|error| AppError::json("parse pull cursor update", &error))?;
        let previous_count = cursors
            .mailboxes
            .get(&update.mailbox_id)
            .map(|cursor| cursor.pull_count_since_full_reconcile)
            .unwrap_or(0);
        cursors.mailboxes.insert(
            update.mailbox_id.clone(),
            PullMailboxCursor {
                mailbox_id: update.mailbox_id,
                mailbox_name: update.mailbox_name,
                uid_validity: update.uid_validity,
                highest_uid: update.highest_uid,
                pull_count_since_full_reconcile: if full_reconcile {
                    0
                } else {
                    previous_count.saturating_add(1)
                },
                updated_rfc3339: now.clone(),
            },
        );
    }
    crate::util::write_json_pretty(&pull_cursor_path(root), &cursors)
}

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
    session.uid_store_flags(source_folder, None, uid, flags, add)
}

pub(super) fn fetch_selected_mailbox_envelopes<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    since_days: Option<u32>,
) -> Result<Vec<RemoteEnvelope>> {
    Ok(fetch_selected_mailbox_envelopes_with_cursor(session, mailbox, since_days, None)?.envelopes)
}

fn fetch_selected_mailbox_envelopes_with_cursor<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    since_days: Option<u32>,
    cursor: Option<PullCursorView>,
) -> Result<MailboxEnvelopeFetch> {
    let mailbox_status = session
        .examine(mailbox)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    let uid_validity = mailbox_status.uid_validity.unwrap_or(0) as u64;
    let uid_validity_reset = cursor.is_some_and(|cursor| cursor.uid_validity != uid_validity);
    let valid_cursor = cursor.filter(|cursor| cursor.uid_validity == uid_validity);
    let previous_highest_uid = valid_cursor.map(|cursor| cursor.highest_uid);
    let effective_since_days = if uid_validity_reset { None } else { since_days };
    let sequence = mailbox_uid_sequence(session, effective_since_days, previous_highest_uid)?;
    if sequence.is_empty() {
        return Ok(MailboxEnvelopeFetch {
            uid_validity,
            envelopes: Vec::new(),
            cursor_used: valid_cursor.is_some(),
            uid_validity_reset,
            previous_highest_uid,
        });
    }
    let fetches = session
        .uid_fetch(&sequence, "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER])")
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
            size_bytes: fetch.size.map(u64::from),
            header: header.to_vec(),
        });
    }
    Ok(MailboxEnvelopeFetch {
        uid_validity,
        envelopes: out,
        cursor_used: valid_cursor.is_some(),
        uid_validity_reset,
        previous_highest_uid,
    })
}

struct BodyFetchBatch {
    messages: Vec<RemoteMessage>,
    skipped: Vec<Value>,
}

fn fetch_current_mailbox_uid_batch<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    uid_validity: u64,
    uids: &[u64],
    body_fetch_mode: BodyFetchMode,
) -> Result<BodyFetchBatch> {
    if uids.is_empty() {
        return Ok(BodyFetchBatch {
            messages: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let uid_set = uids.iter().copied().collect::<BTreeSet<_>>();
    let sequence = uids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let metadata = session
        .uid_fetch(&sequence, "(UID FLAGS RFC822.SIZE)")
        .map_err(|e| AppError::new("imap_fetch_failed", e.to_string()))?;
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let max_message_bytes = max_message_bytes();
    for fetch in metadata.iter() {
        let Some(uid) = fetch.uid else {
            continue;
        };
        let uid = uid as u64;
        if !uid_set.contains(&uid) {
            continue;
        }
        if let Some(size) = fetch.size
            && size > max_message_bytes
        {
            skipped.push(json!({
                "reason": "declared_size_exceeded",
                "mailbox_name": mailbox,
                "uid": uid,
                "declared_size_bytes": size,
                "max_message_bytes": max_message_bytes,
            }));
            continue;
        }
        let fetched_body = match body_fetch_mode {
            BodyFetchMode::BoundedPartial => {
                fetch_bounded_partial_body(session, uid, max_message_bytes)?
            }
            BodyFetchMode::CompatibilityFull => fetch_compatibility_full_body(session, uid)?,
        };
        let Some((response_flags, raw_eml)) = fetched_body else {
            skipped.push(json!({
                "reason": "body_missing",
                "mailbox_name": mailbox,
                "uid": uid,
            }));
            continue;
        };
        if raw_eml.len() > max_message_bytes as usize {
            skipped.push(json!({
                "reason": "actual_size_exceeded",
                "mailbox_name": mailbox,
                "uid": uid,
                "actual_body_bytes_at_least": raw_eml.len(),
                "max_message_bytes": max_message_bytes,
            }));
            continue;
        }
        out.push(RemoteMessage {
            mailbox: mailbox.to_string(),
            uid_validity,
            uid,
            flags: response_flags,
            raw_eml,
        });
    }
    Ok(BodyFetchBatch {
        messages: out,
        skipped,
    })
}

fn fetch_compatibility_full_body<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    uid: u64,
) -> Result<Option<(Vec<String>, Vec<u8>)>> {
    let fetches = session
        .uid_fetch(uid.to_string(), "(UID FLAGS BODY.PEEK[])")
        .map_err(|error| AppError::new("imap_fetch_failed", error.to_string()))?;
    let Some(fetch) = fetches
        .iter()
        .find(|fetch| fetch.uid.map(u64::from) == Some(uid))
    else {
        return Ok(None);
    };
    let Some(body) = fetch.body() else {
        return Ok(None);
    };
    Ok(Some((
        canonical_flags(fetch.flags().iter().map(ToString::to_string)),
        body.to_vec(),
    )))
}

fn fetch_bounded_partial_body<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    uid: u64,
    max_message_bytes: u32,
) -> Result<Option<(Vec<String>, Vec<u8>)>> {
    const CHUNK_BYTES: usize = 1024 * 1024;
    let response_limit = max_message_bytes as usize + 1;
    let mut raw = Vec::new();
    let mut flags = Vec::new();
    while raw.len() < response_limit {
        let request_bytes = CHUNK_BYTES.min(response_limit - raw.len());
        let fetches = session
            .uid_fetch(
                uid.to_string(),
                format!("(UID FLAGS BODY.PEEK[]<{}.{request_bytes}>)", raw.len()),
            )
            .map_err(|error| AppError::new("imap_fetch_failed", error.to_string()))?;
        let Some(fetch) = fetches
            .iter()
            .find(|fetch| fetch.uid.map(u64::from) == Some(uid))
        else {
            return Ok(None);
        };
        flags = canonical_flags(fetch.flags().iter().map(ToString::to_string));
        let Some(body) = fetch.body() else {
            return Ok(None);
        };
        raw.extend_from_slice(body);
        if body.len() < request_bytes {
            break;
        }
    }
    Ok(Some((flags, raw)))
}

pub(super) fn mailbox_uid_sequence<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    since_days: Option<u32>,
    after_uid: Option<u64>,
) -> Result<String> {
    if let Some(uid) = after_uid {
        if uid == u64::MAX {
            return Ok(String::new());
        }
        return Ok(format!("{}:*", uid + 1));
    }
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
    options: PullOptions,
    body_fetch_mode: BodyFetchMode,
    session: &mut imap::Session<T>,
    progress: &mut Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    let started = Instant::now();
    let offset = mail_config.resolved_timezone_offset();
    let mut existing = load_existing_remote_index(root)?;
    let ingest_render_context = PullIngestRenderContext::new(root, mail_config)?;
    let mut new_message_count = 0usize;
    let mut triage_created_count = 0usize;
    let mut spam_message_count = 0usize;
    let mut trashed_message_count = 0usize;
    let mut updated_location_count = 0usize;
    let mut flags_updated_count = 0usize;
    let mut new_message_ids = Vec::new();
    let mut triage_created_message_ids = Vec::new();
    let mut remaining_message_count = options.max_new_messages;
    let mut remaining_total_bytes = options.max_total_bytes;
    let mut budget_limited = false;
    let mut budget_skipped_count = 0usize;
    let mut skipped_messages = Vec::new();
    let mut stored_new_message_bytes = 0u64;
    let cursors = load_pull_cursors(root);
    let mut cursor_updates = Vec::new();
    let mut uid_validity_reset_mailbox_ids = Vec::new();
    let mut all_targets_used_incremental = !targets.is_empty();
    for (target_index, target) in targets.iter().enumerate() {
        emit_mailbox_progress(
            progress,
            "pull_mailbox_headers_start",
            target,
            target_index,
            targets.len(),
            json!({}),
        );
        let cursor = pull_cursor_for_target(&cursors, target);
        let envelope_fetch = fetch_selected_mailbox_envelopes_with_cursor(
            session,
            &target.mailbox,
            options.effective_since_days(),
            cursor,
        )?;
        let uid_validity = envelope_fetch.uid_validity;
        let cursor_used = envelope_fetch.cursor_used;
        if envelope_fetch.uid_validity_reset {
            uid_validity_reset_mailbox_ids.push(target.id.clone());
        }
        all_targets_used_incremental &= cursor_used;
        let previous_highest_uid = envelope_fetch.previous_highest_uid.unwrap_or(0);
        let envelopes = envelope_fetch.envelopes;
        let envelope_count = envelopes.len();
        let mut mailbox_existing_location_count = 0usize;
        let mut mailbox_flags_updated_count = 0usize;
        let mut mailbox_blocked_cursor_uid: Option<u64> = None;
        let mut mailbox_highest_observed_uid = previous_highest_uid;
        let mut new_uids = BTreeSet::new();
        let mut new_envelopes = BTreeMap::new();
        for envelope in envelopes {
            mailbox_highest_observed_uid = mailbox_highest_observed_uid.max(envelope.uid);
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
            let budget_size = envelope.size_bytes.unwrap_or(u64::from(MAX_MESSAGE_BYTES));
            if remaining_message_count == 0 || budget_size > remaining_total_bytes {
                budget_limited = true;
                budget_skipped_count += 1;
                mailbox_blocked_cursor_uid = Some(
                    mailbox_blocked_cursor_uid
                        .unwrap_or(envelope.uid)
                        .min(envelope.uid),
                );
                continue;
            }
            remaining_message_count -= 1;
            remaining_total_bytes -= budget_size;
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
                // No `updated_location_count` here: a relocation is recognized
                // from the body's bytes, which this stage has not fetched. It
                // reported a constant 0, so a caller watching progress saw
                // "nothing relocated" all the way to a non-zero final count.
                "fetched_count": envelope_count,
                "existing_location_count": mailbox_existing_location_count,
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
            let fetched = fetch_current_mailbox_uid_batch(
                session,
                &target.mailbox,
                uid_validity,
                batch,
                body_fetch_mode,
            )?;
            for skipped in &fetched.skipped {
                if let Some(uid) = skipped.get("uid").and_then(Value::as_u64) {
                    mailbox_blocked_cursor_uid =
                        Some(mailbox_blocked_cursor_uid.unwrap_or(uid).min(uid));
                }
            }
            skipped_messages.extend(fetched.skipped);
            processed_uid_count += batch.len();
            fetched_body_count += fetched.messages.len();
            for remote in fetched.messages {
                let Some(envelope) = new_envelopes.get(&remote.uid) else {
                    continue;
                };
                let actual_bytes = remote.raw_eml.len() as u64;
                let reserved_bytes = envelope.size_bytes.unwrap_or(u64::from(MAX_MESSAGE_BYTES));
                if actual_bytes > reserved_bytes {
                    let extra_bytes = actual_bytes - reserved_bytes;
                    if extra_bytes > remaining_total_bytes {
                        skipped_messages.push(json!({
                            "reason": "total_bytes_budget_exceeded",
                            "mailbox_name": remote.mailbox.as_str(),
                            "uid": remote.uid,
                            "declared_size_bytes": envelope.size_bytes,
                            "actual_size_bytes": actual_bytes,
                            "remaining_bytes_budget": remaining_total_bytes,
                        }));
                        budget_limited = true;
                        budget_skipped_count += 1;
                        mailbox_blocked_cursor_uid = Some(
                            mailbox_blocked_cursor_uid
                                .unwrap_or(remote.uid)
                                .min(remote.uid),
                        );
                        continue;
                    }
                    remaining_total_bytes -= extra_bytes;
                } else {
                    remaining_total_bytes += reserved_bytes - actual_bytes;
                }
                let key = ImapKey {
                    mailbox: remote.mailbox.clone(),
                    uid_validity: remote.uid_validity,
                    uid: remote.uid,
                };
                let content_key = message_content_key(&remote.raw_eml);
                if let Some(existing_message_id) = existing.content_ids.get(&content_key) {
                    // Identical bytes already stored: this is a second location
                    // for a message we have, not a new message. Refund what the
                    // header stage reserved for it — the dedup is only knowable
                    // after the body arrives, and without the refund a mailbox
                    // that mirrors another (Gmail's All Mail beside Inbox)
                    // spends the whole `--max-new-messages` / `--max-total-bytes`
                    // budget on copies it then discards.
                    remaining_message_count += 1;
                    remaining_total_bytes += actual_bytes;
                    let update = add_remote_location_parts(
                        root,
                        existing_message_id,
                        &remote.mailbox,
                        remote.uid_validity,
                        remote.uid,
                        &remote.flags,
                        Some(target.id.as_str()),
                    )?;
                    existing.imap_keys.insert(key.clone());
                    existing
                        .imap_locations
                        .insert(key, existing_message_id.clone());
                    updated_location_count += 1;
                    if update.flags_updated {
                        flags_updated_count += 1;
                    }
                    continue;
                }
                let suggestion = existing.suggest_case(&envelope.header);
                let saved = save_remote_message(
                    root,
                    remote,
                    &suggestion,
                    target,
                    offset,
                    mail_config,
                    &ingest_render_context,
                )?;
                existing.imap_keys.insert(key.clone());
                let saved_message_id = saved.message_id.clone();
                existing
                    .imap_locations
                    .insert(key, saved_message_id.clone());
                existing
                    .content_ids
                    .insert(content_key, saved_message_id.clone());
                new_message_ids.push(saved_message_id.clone());
                new_message_count += 1;
                stored_new_message_bytes += actual_bytes;
                mailbox_new_message_count += 1;
                match target.import_as {
                    PullImportAs::Triage => {
                        triage_created_message_ids.push(saved_message_id);
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
        let highest_uid = mailbox_blocked_cursor_uid
            .and_then(|uid| uid.checked_sub(1))
            .unwrap_or(mailbox_highest_observed_uid)
            .max(previous_highest_uid);
        cursor_updates.push(PullCursorUpdate {
            mailbox_id: target.id.clone(),
            mailbox_name: target.mailbox.clone(),
            uid_validity,
            highest_uid,
        });
    }
    let mut result = json!({
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
        "uid_validity_reset_count": uid_validity_reset_mailbox_ids.len(),
        "uid_validity_reset_mailbox_ids": uid_validity_reset_mailbox_ids,
        "mailbox_ids": targets.iter().map(|target| target.id.clone()).collect::<Vec<_>>(),
        "since_days": options.effective_since_days(),
        "all_history": options.all_history,
        "body_fetch_mode": match body_fetch_mode { BodyFetchMode::BoundedPartial => "bounded_partial", BodyFetchMode::CompatibilityFull => "compatibility_full" },
        "max_new_messages": options.max_new_messages,
        "max_total_bytes": options.max_total_bytes,
        "budget_limited": budget_limited,
        "budget_skipped_count": budget_skipped_count,
        "remaining_message_budget": remaining_message_count,
        "remaining_bytes_budget": remaining_total_bytes,
        "stored_new_message_bytes": stored_new_message_bytes,
        "skipped_message_count": skipped_messages.len(),
        "skipped_messages": skipped_messages,
        "mailbox_names": targets.iter().map(|target| target.mailbox.clone()).collect::<Vec<_>>(),
        "mailbox_count": targets.len(),
        "duration_ms": started.elapsed().as_millis() as u64
    });
    if let Value::Object(map) = &mut result {
        map.insert(
            "__afmail_pull_all_incremental".to_string(),
            json!(all_targets_used_incremental),
        );
        map.insert(
            "__afmail_pull_cursor_updates".to_string(),
            serde_json::to_value(&cursor_updates)
                .map_err(|error| AppError::json("serialize pull cursor updates", &error))?,
        );
    }
    add_limited_message_ids(&mut result, "new_message_ids", &new_message_ids);
    add_limited_message_ids(
        &mut result,
        "triage_created_message_ids",
        &triage_created_message_ids,
    );
    Ok(result)
}

pub(super) fn estimate_pull_workspace_session<T: std::io::Read + std::io::Write>(
    root: &Path,
    targets: &[PullTarget],
    options: PullOptions,
    session: &mut imap::Session<T>,
) -> Result<Value> {
    let existing = load_existing_remote_index(root)?;
    let mut remaining_message_count = options.max_new_messages;
    let mut remaining_total_bytes = options.max_total_bytes;
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    let mut remote_envelope_count = 0usize;
    for target in targets {
        let envelopes = fetch_selected_mailbox_envelopes(
            session,
            &target.mailbox,
            options.effective_since_days(),
        )?;
        remote_envelope_count += envelopes.len();
        for envelope in envelopes {
            let key = ImapKey {
                mailbox: envelope.mailbox.clone(),
                uid_validity: envelope.uid_validity,
                uid: envelope.uid,
            };
            // Location is all a dry run can check. Cross-mailbox duplicates are
            // recognized by the bytes of the body, which is exactly what a dry
            // run does not fetch — so a copy of a message already stored under
            // another mailbox counts here and drops out during the real pull.
            // That makes every count below an upper bound, which
            // `candidate_count_is_upper_bound` says out loud rather than
            // leaving a caller to discover it by diffing against the pull.
            let known_by_location = existing.imap_keys.contains(&key);
            if known_by_location {
                continue;
            }
            let budget_size = envelope.size_bytes.unwrap_or(u64::from(MAX_MESSAGE_BYTES));
            let item = json!({
                "mailbox_id": target.id,
                "mailbox_name": target.mailbox,
                "uid_validity": envelope.uid_validity,
                "uid": envelope.uid,
                "declared_size_bytes": envelope.size_bytes,
                "budget_size_bytes": budget_size,
            });
            if remaining_message_count == 0 || budget_size > remaining_total_bytes {
                skipped.push(item);
                continue;
            }
            remaining_message_count -= 1;
            remaining_total_bytes -= budget_size;
            candidates.push(item);
        }
    }
    Ok(json!({
        "dry_run": true,
        "remote_read": true,
        "local_changed": false,
        "mailbox_ids": targets.iter().map(|target| target.id.clone()).collect::<Vec<_>>(),
        "since_days": options.effective_since_days(),
        "all_history": options.all_history,
        "max_new_messages": options.max_new_messages,
        "max_total_bytes": options.max_total_bytes,
        "remote_envelope_count": remote_envelope_count,
        "candidate_count": candidates.len(),
        "candidate_count_is_upper_bound": true,
        "candidate_declared_bytes": candidates.iter().filter_map(|item| item["declared_size_bytes"].as_u64()).sum::<u64>(),
        "candidates": candidates,
        "budget_limited": !skipped.is_empty(),
        "budget_skipped_count": skipped.len(),
        "budget_skipped": skipped,
        "remaining_message_budget": remaining_message_count,
        "remaining_bytes_budget": remaining_total_bytes,
    }))
}

fn add_limited_message_ids(value: &mut Value, key: &str, message_ids: &[String]) {
    let Value::Object(map) = value else {
        return;
    };
    if message_ids.len() <= PULL_RESULT_MESSAGE_ID_LIMIT {
        map.insert(key.to_string(), json!(message_ids));
        return;
    }
    map.insert(format!("{key}_truncated"), json!(true));
    map.insert(format!("{key}_count"), json!(message_ids.len()));
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
