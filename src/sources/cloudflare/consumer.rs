//! One finite consumer tick; the supervisor schedules ticks without a model.
use super::{
    journal::{self, Batch, Journal, TaskStatus, apply_completed, completed_marker},
    pull_objects,
    queue::QueueClient,
    r2,
};
use crate::config::MailConfig;
use crate::error::{AppError, Result};
use crate::sources::imap::PullOptions;
use crate::store::Workspace;
use crate::workspace_lock::{LockMode, WorkspaceLock};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn consumer_lock(root: &Path) -> Result<File> {
    let directory = journal::directory(root);
    std::fs::create_dir_all(&directory)
        .map_err(|e| AppError::io("create consumer directory", &e))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("consumer.lock"))
        .map_err(|e| AppError::io("open consumer lock", &e))?;
    fs4::FileExt::try_lock(&file).map_err(|_| {
        AppError::retryable(
            "consumer_locked",
            "another Cloudflare consumer is running for this workspace",
        )
    })?;
    Ok(file)
}
/// Handler-side afmail commands hold the workspace lock only briefly, so the
/// consumer waits a bounded time instead of losing a handler outcome to them.
const OUTCOME_LOCK_WAIT: Duration = Duration::from_secs(30);
fn acquire_with_wait(root: &Path, mode: LockMode, wait: Duration) -> Result<WorkspaceLock> {
    let started = Instant::now();
    loop {
        if let Some(lock) = WorkspaceLock::try_acquire(root, mode)? {
            return Ok(lock);
        }
        if started.elapsed() >= wait {
            return WorkspaceLock::acquire(root, mode);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub(crate) fn consume(cwd: &Path, reconcile: bool) -> Result<Value> {
    if crate::sources::consumer_shutdown::requested() {
        return Err(crate::sources::consumer_shutdown::interrupted());
    }
    let ws = Workspace::discover(cwd)?;
    let root = ws.root();
    let _consumer_lock = consumer_lock(root)?;
    let config = MailConfig::load(root)?;
    if !config.cloudflare.is_configured() {
        return Err(AppError::new(
            "config_missing",
            "cloudflare.r2.bucket is required",
        ));
    }
    let queue = QueueClient::new(&config)?;
    let mut warnings = Vec::new();
    let mut acks = Vec::new();
    let mut retries = Vec::new();
    let mut imported_count = 0u64;
    let mut reconciled = false;
    let mut budget = options();
    let received_count;
    {
        // Lease events only once the workspace is ours: a lease taken before a
        // failed lock would be neither acknowledged nor retried, burning a
        // delivery attempt and hiding the event for the visibility timeout.
        let _lock = WorkspaceLock::acquire(root, LockMode::Exclusive)?;
        ws.ensure_no_incomplete_transactions()?;
        // Queue outages must not prevent R2 reconciliation or retries of local work.
        let messages = match queue.pull(&config) {
            Ok(messages) => messages,
            Err(error) => {
                warnings.push(error.to_error_value());
                vec![]
            }
        };
        received_count = messages.len();
        for message in messages {
            let result = (|| -> Result<()> {
                let event = message.event()?;
                if event.account != config.cloudflare.account_id.as_deref().unwrap_or_default()
                    || Some(event.bucket.as_str()) != config.cloudflare.r2.bucket.as_deref()
                    || !event.object.key.starts_with(&config.cloudflare.r2.prefix)
                    || !matches!(
                        event.action.as_str(),
                        "PutObject" | "CopyObject" | "CompleteMultipartUpload"
                    )
                {
                    return Err(AppError::new(
                        "cloudflare_event_rejected",
                        "Queue event does not match the configured account, bucket, pending prefix, and create actions",
                    ));
                }
                let object_id = journal::object_id(&config, &event.object.key);
                if recover_receipt(root, &config, &event.object.key, &object_id)? {
                    return Ok(());
                }
                let value = pull_objects(
                    root,
                    &config,
                    budget,
                    None,
                    Some(vec![r2::Object {
                        key: event.object.key,
                        size_bytes: event.object.size,
                    }]),
                )?;
                imported_count += value["new_message_count"].as_u64().unwrap_or(0);
                if let Some(items) = value["cloudflare"]["warnings"].as_array() {
                    warnings.extend(items.iter().cloned());
                }
                budget.max_new_messages =
                    value["remaining_message_budget"].as_u64().unwrap_or(0) as usize;
                budget.max_total_bytes = value["remaining_bytes_budget"].as_u64().unwrap_or(0);
                let journal = Journal::load(root)?;
                if !journal
                    .receipts
                    .get(&object_id)
                    .is_some_and(|receipt| receipt.cleanup_complete)
                {
                    return Err(AppError::retryable(
                        "cloudflare_import_pending",
                        "mail import or spool cleanup remains pending",
                    ));
                }
                Ok(())
            })();
            match result {
                Ok(()) => acks.push(message.lease_id),
                Err(error) => {
                    warnings.push(error.to_error_value());
                    retries.push(message.lease_id);
                }
            }
        }
        if reconcile {
            match super::pull(root, &config, budget, None) {
                Ok(value) => {
                    reconciled = true;
                    imported_count += value["new_message_count"].as_u64().unwrap_or(0);
                    if let Some(items) = value["cloudflare"]["warnings"].as_array() {
                        warnings.extend(items.iter().cloned());
                    }
                    if let Some(items) = value["cloudflare"]["skipped_objects"].as_array() {
                        warnings.extend(items.iter().cloned());
                    }
                }
                Err(error) => warnings.push(error.to_error_value()),
            }
        }
    }
    let settled = if acks.is_empty() && retries.is_empty() {
        true
    } else {
        match queue.settle(&acks, &retries, config.cloudflare.consumer.retry_delay_s) {
            Ok(()) => true,
            Err(error) => {
                warnings.push(error.to_error_value());
                false
            }
        }
    };
    // The workspace lock is released before invoking the handler: an agent
    // must be able to use normal afmail commands from inside that handler.
    if crate::sources::consumer_shutdown::requested() {
        return Err(crate::sources::consumer_shutdown::interrupted());
    }
    let dispatched = match dispatch(root, &config, &mut warnings) {
        Ok(value) => value,
        Err(error) if error.error_code == "consumer_interrupted" => return Err(error),
        Err(error) => {
            warnings.push(error.to_error_value());
            false
        }
    };
    // The journal is replaced atomically, so this read needs no workspace lock;
    // a handler's own afmail command must not turn a finished tick into an error.
    let pending = {
        let mut journal = Journal::load(root)?;
        apply_completed(root, &mut journal)?;
        journal.pending_count()
    };
    Ok(
        json!({"received_event_count":received_count, "imported_count":imported_count,
        "acknowledged_event_count":if settled { acks.len() } else { 0 }, "retried_event_count":if settled { retries.len() } else { 0 },
        "reconciled":reconciled, "dispatched":dispatched, "pending_message_count":pending,
        "warnings":warnings}),
    )
}
fn options() -> PullOptions {
    PullOptions::new(None, None, None, false, false)
}

fn recover_receipt(root: &Path, config: &MailConfig, key: &str, object_id: &str) -> Result<bool> {
    let mut journal = Journal::load(root)?;
    let Some(receipt) = journal.receipts.get(object_id).cloned() else {
        return Ok(false);
    };
    // A receipt is evidence of durable import; a missing object alone is not.
    if receipt.cleanup_complete {
        return Ok(true);
    }
    let message = Workspace::at(root).read_message_by_id(&receipt.message_id)?;
    if message.message_id != receipt.message_id {
        return Err(AppError::new(
            "consumer_journal_invalid",
            "receipt message identity mismatch",
        ));
    }
    let client = r2::R2Client::new(&config.cloudflare)?;
    // Managed acknowledgement is idempotent and also repairs quota accounting.
    if config.cloudflare.spool.control_url.is_some() {
        client.acknowledge(key, receipt.archive_key.as_deref(), receipt.quota_managed)?;
    } else {
        if receipt.quota_managed {
            return Err(AppError::new(
                "config_missing",
                "managed spool cleanup requires cloudflare.spool.control_url",
            ));
        }
        if let Some(target) = &receipt.archive_key {
            match client.get(target) {
                Ok(download)
                    if crate::ingest::message_content_key(&download.raw) == receipt.content_key => {
                }
                Ok(_) => {
                    return Err(AppError::new(
                        "cloudflare_archive_conflict",
                        "archive destination does not match the imported mail receipt",
                    ));
                }
                Err(error)
                    if error
                        .details
                        .as_ref()
                        .is_some_and(|value| value["http_status"] == 404) =>
                {
                    client.copy(key, target)?
                }
                Err(error) => return Err(error),
            }
        }
        client.delete(key)?;
    }
    journal.cleaned(root, object_id)?;
    Ok(true)
}

/// Persist a folded completion, then retire the marker. Requires the exclusive
/// workspace lock. Markers of batches that are no longer active are stale.
fn record_completed(root: &Path, journal: &mut Journal) -> Result<()> {
    let task_id = journal.active.as_ref().map(|batch| batch.task_id.clone());
    if apply_completed(root, journal)? {
        journal.save(root)?;
    }
    let batches = journal::directory(root).join("batches");
    let entries = match std::fs::read_dir(&batches) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::io("list consumer batches", &error)),
    };
    for entry in entries {
        let path = entry
            .map_err(|e| AppError::io("read consumer batch entry", &e))?
            .path();
        let still_active = journal.active.is_some()
            && task_id
                .as_deref()
                .is_some_and(|id| path == completed_marker(root, id));
        if path.extension().is_some_and(|ext| ext == "completed") && !still_active {
            std::fs::remove_file(&path)
                .map_err(|e| AppError::io("remove consumer completion", &e))?;
        }
    }
    Ok(())
}
/// Remove a task whose message can no longer be handed to the handler. A
/// purged or unreadable message must never block every later batch.
fn drop_unavailable(
    root: &Path,
    journal: &mut Journal,
    key: &str,
    warnings: &mut Vec<Value>,
) -> Result<Option<String>> {
    let Some(task) = journal.tasks.get(key) else {
        return Ok(None);
    };
    let message_id = task.message_id.clone();
    let raw_path = root.join(format!(".afmail/messages/{message_id}.eml"));
    let problem = if !raw_path.is_file() {
        Some(AppError::new(
            "consumer_message_missing",
            "a pending consumer task refers to a message that no longer exists; the task was dropped",
        ))
    } else {
        Workspace::at(root)
            .read_message_by_id(&message_id)
            .err()
            .map(|error| {
                AppError::new(
                    error.error_code,
                    "a pending consumer task refers to an unreadable message; the task was dropped",
                )
            })
    };
    let Some(problem) = problem else {
        return Ok(Some(message_id));
    };
    warnings.push(
        problem
            .with_details(json!({"message_id":message_id}))
            .to_error_value(),
    );
    journal.tasks.remove(key);
    Ok(None)
}

fn dispatch(root: &Path, config: &MailConfig, warnings: &mut Vec<Value>) -> Result<bool> {
    if config.cloudflare.consumer.command.is_empty() {
        return Ok(false);
    }
    let batch = {
        let _lock = WorkspaceLock::acquire(root, LockMode::Exclusive)?;
        let mut journal = Journal::load(root)?;
        record_completed(root, &mut journal)?;
        let before = journal.tasks.len();
        let mut items = Vec::new();
        if let Some(active) = journal.active.clone() {
            let mut keys = Vec::new();
            for key in &active.content_keys {
                if let Some(message_id) = drop_unavailable(root, &mut journal, key, warnings)? {
                    keys.push(key.clone());
                    items.push(message_id);
                }
            }
            journal.active = if keys.is_empty() {
                None
            } else {
                Some(Batch {
                    content_keys: keys,
                    ..active
                })
            };
        }
        if journal.active.is_none() {
            items.clear();
            let pending: Vec<_> = journal
                .tasks
                .iter()
                .filter(|(_, task)| task.status == TaskStatus::Pending)
                .map(|(key, _)| key.clone())
                .collect();
            let mut keys = Vec::new();
            for key in pending {
                if keys.len() >= config.cloudflare.queue.batch_size {
                    break;
                }
                if let Some(message_id) = drop_unavailable(root, &mut journal, &key, warnings)? {
                    keys.push(key);
                    items.push(message_id);
                }
            }
            if !keys.is_empty() {
                let identity = serde_json::to_vec(&keys)
                    .map_err(|e| AppError::json("serialize task identity", &e))?;
                journal.active = Some(Batch {
                    task_id: format!("mail_{}", blake3::hash(&identity).to_hex()),
                    content_keys: keys,
                    retry_after_epoch_s: 0,
                    attempt_count: 0,
                    last_attempt_rfc3339: None,
                    last_error_code: None,
                    last_exit_code: None,
                });
            }
        }
        let Some(batch) = journal.active.clone() else {
            if journal.tasks.len() != before {
                journal.save(root)?;
            }
            return Ok(false);
        };
        if batch.retry_after_epoch_s > chrono::Utc::now().timestamp() {
            if journal.tasks.len() != before {
                journal.save(root)?;
            }
            return Ok(false);
        }
        let items = items
            .iter()
            .map(|message_id| json!({"message_id":message_id, "raw_path":format!(".afmail/messages/{message_id}.eml")}))
            .collect::<Vec<_>>();
        // Persist the immutable input before recording an attempt. Only trusted
        // config supplies argv; mail headers and bodies never enter a command.
        crate::util::write_json_pretty(
            &journal::directory(root)
                .join("batches")
                .join(format!("{}.json", batch.task_id)),
            &json!({"schema_name":"mail_received_task", "schema_version":1, "task_id":batch.task_id, "messages":items}),
        )?;
        if let Some(active) = &mut journal.active {
            active.attempt_count = active.attempt_count.saturating_add(1);
            active.last_attempt_rfc3339 = Some(chrono::Utc::now().to_rfc3339());
            active.retry_after_epoch_s = chrono::Utc::now().timestamp()
                + config.cloudflare.consumer.command_timeout_s as i64
                + config.cloudflare.consumer.retry_delay_s as i64;
        }
        journal.save(root)?;
        batch
    };
    if let Err(error) = run_handler(root, config, &batch) {
        // The handler's error is the outcome; failing to shorten the retry delay
        // only leaves the longer delay recorded before the attempt.
        let recorded =
            acquire_with_wait(root, LockMode::Exclusive, OUTCOME_LOCK_WAIT).and_then(|_lock| {
                let mut journal = Journal::load(root)?;
                if let Some(active) = &mut journal.active
                    && active.task_id == batch.task_id
                {
                    active.last_error_code = Some(error.error_code.to_string());
                    active.last_exit_code = error
                        .details
                        .as_ref()
                        .and_then(|details| details["exit_code"].as_i64())
                        .and_then(|code| i32::try_from(code).ok());
                    active.retry_after_epoch_s = chrono::Utc::now().timestamp()
                        + config.cloudflare.consumer.retry_delay_s as i64;
                    journal.save(root)?;
                }
                Ok(())
            });
        if let Err(record_error) = recorded {
            warnings.push(record_error.to_error_value());
        }
        return Err(error);
    }
    crate::util::write_json_pretty(
        &completed_marker(root, &batch.task_id),
        &json!({"schema_name":"mail_received_task_completed", "schema_version":1, "task_id":batch.task_id}),
    )?;
    // Folding now is an optimisation; the marker already makes completion durable.
    if let Ok(_lock) = acquire_with_wait(root, LockMode::Exclusive, OUTCOME_LOCK_WAIT) {
        let mut journal = Journal::load(root)?;
        record_completed(root, &mut journal)?;
    }
    Ok(true)
}

fn run_handler(root: &Path, config: &MailConfig, batch: &Batch) -> Result<()> {
    let program = config.cloudflare.consumer.command.first().ok_or_else(|| {
        AppError::new(
            "config_missing",
            "cloudflare.consumer.command is required for dispatch",
        )
    })?;
    let path = journal::directory(root)
        .join("batches")
        .join(format!("{}.json", batch.task_id));
    let mut command = Command::new(program);
    command
        .args(&config.cloudflare.consumer.command[1..])
        .current_dir(root)
        .env("AFMAIL_TASK_PATH", &path)
        .env("AFMAIL_TASK_ID", &batch.task_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = super::handler_process::HandlerProcess::spawn(&mut command).map_err(|_| {
        AppError::retryable(
            "consumer_command_failed",
            "could not start the configured Cloudflare consumer command",
        )
    })?;
    let started = Instant::now();
    let success = loop {
        match child.try_wait().map_err(|e| {
            if e.kind() == std::io::ErrorKind::Interrupted {
                crate::sources::consumer_shutdown::interrupted()
            } else {
                AppError::io("wait for consumer command", &e)
            }
        })? {
            Some(status) => break status,
            None if started.elapsed()
                >= Duration::from_secs(config.cloudflare.consumer.command_timeout_s) =>
            {
                child
                    .stop()
                    .map_err(|e| AppError::io("stop timed-out consumer command", &e))?;
                return Err(AppError::retryable(
                    "consumer_command_timeout",
                    "Cloudflare consumer command timed out; its durable task remains pending",
                ));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    if !success.success() {
        return Err(AppError::retryable(
            "consumer_command_failed",
            "Cloudflare consumer command exited unsuccessfully; its durable task remains pending",
        )
        .with_details(json!({"exit_code": success.code()})));
    }
    Ok(())
}
