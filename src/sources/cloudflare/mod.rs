mod consumer;
mod handler_process;
use super::cloudflare_journal as journal;
mod queue;
pub(crate) mod r2;
mod scaffold;
pub(crate) use consumer::consume;
pub(crate) use scaffold::scaffold;

use super::imap::PullOptions;
use crate::config::MailConfig;
use crate::error::{AppError, Result};
use crate::ingest::{
    ExistingIndex, IngestMessage, IngestOutcome, IngestRenderContext, MAX_MESSAGE_BYTES,
    ingest_message, load_existing_remote_index,
};
use crate::progress::{ProgressCallback, emit};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

pub(crate) fn remote_test(config: &MailConfig) -> Result<Value> {
    r2::R2Client::new(&config.cloudflare)?.list(&config.cloudflare.r2.prefix, None, 1)?;
    Ok(json!({"status":"succeeded", "bucket":config.cloudflare.r2.bucket}))
}
/// A per-object problem: the key and error code only, never message content.
fn object_warning(key: &str, error: &AppError) -> Value {
    json!({"code":error.error_code,"message":error.message,"key":key})
}
pub(crate) fn pull(
    root: &Path,
    config: &MailConfig,
    options: PullOptions,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    pull_objects(root, config, options, progress, None)
}
fn pull_objects(
    root: &Path,
    config: &MailConfig,
    options: PullOptions,
    progress: Option<&mut ProgressCallback<'_>>,
    selected: Option<Vec<r2::Object>>,
) -> Result<Value> {
    let started = Instant::now();
    let mut progress = progress;
    let source = &config.cloudflare;
    let client = r2::R2Client::new(source)?;
    emit(
        &mut progress,
        "pull_cloudflare_list_start",
        json!({"bucket":source.r2.bucket,"prefix":source.r2.prefix}),
    );
    let mut objects = Vec::new();
    if let Some(selected) = selected {
        objects = selected;
    } else {
        let mut continuation = None;
        let mut tokens = HashSet::new();
        loop {
            let page = client.list(&source.r2.prefix, continuation.as_deref(), 1000)?;
            objects.extend(page.objects);
            continuation = page.continuation;
            let Some(token) = &continuation else {
                break;
            };
            if !tokens.insert(token.clone()) {
                return Err(AppError::new(
                    "cloudflare_r2_failed",
                    "R2 list returned a repeated continuation token",
                ));
            }
        }
    }
    objects.sort_by(|left, right| left.key.cmp(&right.key));
    objects.dedup_by(|left, right| left.key == right.key);
    emit(
        &mut progress,
        "pull_cloudflare_list_done",
        json!({"listed_object_count":objects.len()}),
    );
    let mut remaining_count = options.max_new_messages;
    let mut remaining_bytes = options.max_total_bytes;
    let mut index = if options.dry_run {
        ExistingIndex::default()
    } else {
        load_existing_remote_index(root)?
    };
    let context = if options.dry_run {
        None
    } else {
        Some(IngestRenderContext::new(root, config)?)
    };
    let mut journal = if !options.dry_run && journal::enabled(config) {
        Some(journal::Journal::load(root)?)
    } else {
        None
    };
    let mut imported = 0;
    let mut merged = 0;
    let mut deleted = 0;
    let mut moved = 0;
    let mut candidates = 0;
    let mut stored_bytes = 0u64;
    let mut warnings = Vec::new();
    let mut skipped = Vec::new();
    let mut ids = Vec::new();
    for object in &objects {
        let reason = if !object.key.starts_with(&source.r2.prefix) {
            Some("outside_prefix")
        } else if object.size_bytes > MAX_MESSAGE_BYTES {
            Some("message_too_large")
        } else if remaining_count == 0 {
            Some("message_count_budget_exceeded")
        } else if object.size_bytes > remaining_bytes {
            Some("total_bytes_budget_exceeded")
        } else {
            None
        };
        if let Some(reason) = reason {
            skipped.push(json!({"key":object.key,"reason":reason,"size_bytes":object.size_bytes}));
            continue;
        }
        candidates += 1;
        if options.dry_run {
            remaining_count -= 1;
            remaining_bytes -= object.size_bytes;
            continue;
        }
        emit(
            &mut progress,
            "pull_cloudflare_object_start",
            json!({"key":object.key,"size_bytes":object.size_bytes}),
        );
        let downloaded = match client.get(&object.key) {
            Ok(value) => value,
            Err(error) if error.error_code == "message_too_large" => {
                skipped.push(json!({"key":object.key,"reason":"message_too_large","size_bytes":object.size_bytes}));
                continue;
            }
            // A listed object can vanish before its download (another consumer
            // took it); that is about this object, not the whole scan.
            Err(error)
                if error
                    .details
                    .as_ref()
                    .is_some_and(|value| value["http_status"] == 404) =>
            {
                warnings.push(object_warning(&object.key, &error));
                continue;
            }
            Err(error) => return Err(error),
        };
        let actual_bytes = downloaded.raw.len() as u64;
        if actual_bytes > remaining_bytes {
            skipped.push(json!({"key":object.key,"reason":"total_bytes_budget_exceeded","size_bytes":actual_bytes}));
            continue;
        }
        let Some(context) = context.as_ref() else {
            return Err(AppError::new("internal_error", "missing ingest context"));
        };
        let quota_managed = downloaded.quota_managed;
        let content_key = crate::ingest::message_content_key(&downloaded.raw);
        let object_id = journal::object_id(config, &object.key);
        let archive_key = source.r2.imported_prefix.as_ref().map(|prefix| {
            format!(
                "{prefix}{}",
                object
                    .key
                    .strip_prefix(&source.r2.prefix)
                    .unwrap_or_default()
            )
        });
        let prepared = match &mut journal {
            Some(journal) => journal.prepare(root, config, &index, &downloaded.raw)?,
            None => false,
        };
        let outcome = match ingest_message(
            root,
            &mut index,
            context,
            config,
            IngestMessage {
                raw_eml: downloaded.raw,
                delivery: Some(downloaded.delivery),
                remote_location: None,
                import_as: source.import_as,
                direction: source.direction,
            },
        ) {
            Ok(outcome) => outcome,
            // Local storage failures affect every object; stop the scan.
            Err(error) if error.error_code == "store_error" => return Err(error),
            // The object stays in R2 for a later pull; its intent is withdrawn so
            // no task waits on mail that was never imported.
            Err(error) => {
                if prepared && let Some(journal) = &mut journal {
                    journal.withdraw(root, &content_key)?;
                }
                warnings.push(object_warning(&object.key, &error));
                emit(
                    &mut progress,
                    "pull_cloudflare_object_failed",
                    json!({"key":object.key,"code":error.error_code}),
                );
                continue;
            }
        };
        let message_id = outcome.message_id().to_string();
        if let Some(journal) = &mut journal {
            journal.imported(
                root,
                object_id.clone(),
                journal::Receipt {
                    message_id: message_id.clone(),
                    imported_epoch_s: chrono::Utc::now().timestamp(),
                    content_key,
                    quota_managed,
                    archive_key: archive_key.clone(),
                    cleanup_complete: false,
                },
            )?;
        }
        if let Some(warning) = outcome.duplicate_candidate_warning() {
            warnings.push(warning);
        }
        match outcome {
            IngestOutcome::Stored { .. } => {
                imported += 1;
                remaining_count -= 1;
                remaining_bytes -= actual_bytes;
                stored_bytes += actual_bytes;
                ids.push(message_id.clone());
            }
            IngestOutcome::Merged { .. } => merged += 1,
        }
        let cleanup = if let Some(prefix) = &source.r2.imported_prefix {
            let suffix = object.key.strip_prefix(&source.r2.prefix).ok_or_else(|| {
                AppError::new(
                    "cloudflare_r2_failed",
                    "object key is outside the configured prefix",
                )
            })?;
            client
                .acknowledge(
                    &object.key,
                    Some(&format!("{prefix}{suffix}")),
                    quota_managed,
                )
                .map(|()| moved += 1)
        } else {
            client
                .acknowledge(&object.key, None, quota_managed)
                .map(|()| deleted += 1)
        };
        match cleanup {
            Ok(()) => {
                if let Some(journal) = &mut journal {
                    journal.cleaned(root, &object_id)?;
                }
            }
            Err(error) => warnings.push(
                json!({"code":error.error_code,"message":error.message,"details":error.details}),
            ),
        }
        emit(
            &mut progress,
            "pull_cloudflare_object_done",
            json!({"key":object.key,"message_id":message_id,"imported_count":imported,"merged_count":merged}),
        );
    }
    if !options.dry_run {
        crate::store::Workspace::at(root).render_refresh()?;
    }
    Ok(json!({
        "dry_run":options.dry_run,"new_message_count":imported,"new_message_ids":ids,
        "triage_created_count": if source.import_as == crate::config::PullImportAs::Triage { imported } else { 0 },
        "spam_message_count": if source.import_as == crate::config::PullImportAs::Spam { imported } else { 0 },
        "trashed_message_count": if source.import_as == crate::config::PullImportAs::Trashed { imported } else { 0 },
        "stored_new_message_bytes":stored_bytes,"candidate_count":candidates,
        "remaining_message_budget":remaining_count,"remaining_bytes_budget":remaining_bytes,
        "budget_limited":skipped.iter().any(|item| item["reason"].as_str().is_some_and(|reason| reason.ends_with("budget_exceeded"))),
        "candidate_declared_bytes":if options.dry_run { options.max_total_bytes - remaining_bytes } else { 0 },"candidate_count_is_upper_bound":true,
        "cloudflare":{"bucket":source.r2.bucket,"prefix":source.r2.prefix,"listed_object_count":objects.len(),"imported_count":imported,"merged_count":merged,"deleted_object_count":deleted,"moved_object_count":moved,"skipped_object_count":skipped.len(),"skipped_objects":skipped,"warnings":warnings,"duration_ms":started.elapsed().as_millis() as u64}
    }))
}
