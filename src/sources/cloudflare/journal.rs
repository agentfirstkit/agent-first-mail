//! Durable notification intent is written before ingestion, then made ready
//! before remote cleanup. A crash can replay either boundary without losing work.
#[cfg(feature = "cloudflare")]
use crate::config::MailConfig;
use crate::error::{AppError, Result};
#[cfg(feature = "cloudflare")]
use crate::ingest::{ExistingIndex, message_content_key, stable_message_id};
#[cfg(feature = "cloudflare")]
use crate::util::write_json_pretty;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Receipt {
    pub message_id: String,
    /// When the import was recorded; a cleaned receipt is kept only while its
    /// Queue event could still be delivered.
    pub imported_epoch_s: i64,
    pub content_key: String,
    pub quota_managed: bool,
    pub archive_key: Option<String>,
    pub cleanup_complete: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskStatus {
    Preparing,
    Pending,
    Completed,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Task {
    pub message_id: String,
    pub status: TaskStatus,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Batch {
    pub task_id: String,
    pub content_keys: Vec<String>,
    pub retry_after_epoch_s: i64,
    #[serde(default)]
    pub attempt_count: u64,
    #[serde(default)]
    pub last_attempt_rfc3339: Option<String>,
    #[serde(default)]
    pub last_error_code: Option<String>,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
}
#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Journal {
    pub receipts: BTreeMap<String, Receipt>,
    pub tasks: BTreeMap<String, Task>,
    pub active: Option<Batch>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_name: String,
    schema_version: u64,
    journal: Journal,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionMarker {
    schema_name: String,
    schema_version: u64,
    task_id: String,
}
const SCHEMA_VERSION: u64 = 2;
/// Cloudflare Queues deletes every message once it reaches the queue's retention
/// period, which the platform caps at 14 days (1,209,600 s) whatever the
/// operator configured, retries included. A day of margin covers clock skew and
/// leases taken just before expiry. Past this age no event can ask about an
/// object, so a receipt whose cleanup finished can no longer be consulted.
#[cfg(feature = "cloudflare")]
pub(crate) const RECEIPT_RETENTION_S: i64 = (14 + 1) * 86_400;
pub(crate) fn directory(root: &Path) -> PathBuf {
    root.join(".afmail/cloudflare-consumer")
}
#[cfg(feature = "cloudflare")]
pub(crate) fn enabled(config: &MailConfig) -> bool {
    config.cloudflare.queue.queue_id.is_some() || !config.cloudflare.consumer.command.is_empty()
}
#[cfg(feature = "cloudflare")]
pub(crate) fn object_id(config: &MailConfig, key: &str) -> String {
    let source = &config.cloudflare;
    let scope = format!(
        "{:?}\n{:?}\n{:?}\n{key}",
        source.account_id, source.r2.endpoint_url, source.r2.bucket
    );
    blake3::hash(scope.as_bytes()).to_hex().to_string()
}
impl Journal {
    pub fn load(root: &Path) -> Result<Self> {
        let bytes = match std::fs::read(directory(root).join("journal.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(AppError::io("read consumer journal", &error)),
        };
        let doc: Document = serde_json::from_slice(&bytes).map_err(|_| {
            AppError::new(
                "consumer_journal_invalid",
                "Cloudflare consumer journal is invalid; restore it from a trusted backup",
            )
        })?;
        if doc.schema_name != "cloudflare_consumer" || doc.schema_version != SCHEMA_VERSION {
            return Err(AppError::new(
                "consumer_journal_invalid",
                "unsupported Cloudflare consumer journal schema",
            ));
        }
        doc.journal.validate_active()?;
        Ok(doc.journal)
    }

    fn validate_active(&self) -> Result<()> {
        let Some(active) = &self.active else {
            return Ok(());
        };
        let unique: std::collections::BTreeSet<_> = active.content_keys.iter().collect();
        if !active.task_id.starts_with("mail_")
            || active.task_id.len() > 128
            || !active
                .task_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || active.content_keys.is_empty()
            || unique.len() != active.content_keys.len()
            || active.content_keys.iter().any(|key| {
                !self
                    .tasks
                    .get(key)
                    .is_some_and(|task| task.status == TaskStatus::Pending)
            })
        {
            return Err(AppError::new(
                "consumer_journal_invalid",
                "Active consumer batch must have a safe task id and distinct pending notification tasks",
            ));
        }
        Ok(())
    }
    /// Drop what nothing can consult any more: completed tasks (the workspace
    /// index already deduplicates their mail) and cleaned receipts older than
    /// any deliverable Queue event.
    #[cfg(feature = "cloudflare")]
    fn prune(&mut self, now_epoch_s: i64) {
        self.tasks
            .retain(|_, task| task.status != TaskStatus::Completed);
        self.receipts.retain(|_, receipt| {
            !receipt.cleanup_complete
                || now_epoch_s.saturating_sub(receipt.imported_epoch_s) < RECEIPT_RETENTION_S
        });
    }
    /// The only save point; pruning rides on it so the journal stays bounded
    /// without an extra write.
    #[cfg(feature = "cloudflare")]
    pub fn save(&mut self, root: &Path) -> Result<()> {
        self.prune(chrono::Utc::now().timestamp());
        #[derive(Serialize)]
        struct DocumentRef<'a> {
            schema_name: &'static str,
            schema_version: u64,
            journal: &'a Journal,
        }
        write_json_pretty(
            &directory(root).join("journal.json"),
            &DocumentRef {
                schema_name: "cloudflare_consumer",
                schema_version: SCHEMA_VERSION,
                journal: self,
            },
        )
    }
    #[cfg(feature = "cloudflare")]
    pub fn prepare(
        &mut self,
        root: &Path,
        config: &MailConfig,
        index: &ExistingIndex,
        raw: &[u8],
    ) -> Result<bool> {
        let content_key = message_content_key(raw);
        if !index.content_ids.contains_key(&content_key) && !self.tasks.contains_key(&content_key) {
            self.tasks.insert(
                content_key,
                Task {
                    message_id: stable_message_id(raw, config.resolved_timezone_offset()),
                    status: TaskStatus::Preparing,
                },
            );
            self.save(root)?;
            return Ok(true);
        }
        Ok(false)
    }
    /// Withdraw an intent this pull prepared when ingestion failed; the source
    /// object remains and a later pull prepares it again.
    #[cfg(feature = "cloudflare")]
    pub fn withdraw(&mut self, root: &Path, content_key: &str) -> Result<()> {
        if self
            .tasks
            .get(content_key)
            .is_some_and(|task| task.status == TaskStatus::Preparing)
        {
            self.tasks.remove(content_key);
            self.save(root)?;
        }
        Ok(())
    }
    #[cfg(feature = "cloudflare")]
    pub fn imported(&mut self, root: &Path, object_id: String, receipt: Receipt) -> Result<()> {
        if let Some(task) = self.tasks.get_mut(&receipt.content_key)
            && task.status == TaskStatus::Preparing
        {
            task.message_id = receipt.message_id.clone();
            task.status = TaskStatus::Pending;
        }
        self.receipts.insert(object_id, receipt);
        self.save(root)
    }
    #[cfg(feature = "cloudflare")]
    pub fn cleaned(&mut self, root: &Path, object_id: &str) -> Result<()> {
        if let Some(receipt) = self.receipts.get_mut(object_id) {
            receipt.cleanup_complete = true;
            self.save(root)?;
        }
        Ok(())
    }
    pub fn pending_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| task.status == TaskStatus::Pending)
            .count()
    }

    /// A read-only snapshot also recognizes the durable completion marker when
    /// workspace lock contention prevented the consumer from folding it yet.
    pub(crate) fn snapshot(root: &Path) -> Result<serde_json::Value> {
        let mut journal = Self::load(root)?;
        apply_completed(root, &mut journal)?;
        let active = journal.active.as_ref().map(|batch| {
            serde_json::json!({
                "task_id": batch.task_id,
                "message_ids": batch.content_keys.iter().filter_map(|key| journal.tasks.get(key).map(|task| &task.message_id)).collect::<Vec<_>>(),
                "attempt_count": batch.attempt_count,
                "last_attempt_rfc3339": batch.last_attempt_rfc3339,
                "retry_after_epoch_s": batch.retry_after_epoch_s,
                "last_error_code": batch.last_error_code,
                "last_exit_code": batch.last_exit_code,
            })
        });
        Ok(serde_json::json!({
            "pending_message_count": journal.pending_count(),
            "preparing_message_count": journal.tasks.values().filter(|task| task.status == TaskStatus::Preparing).count(),
            "completed_message_count": journal.tasks.values().filter(|task| task.status == TaskStatus::Completed).count(),
            "receipt_count": journal.receipts.len(),
            "cleanup_pending_count": journal.receipts.values().filter(|receipt| !receipt.cleanup_complete).count(),
            "active_batch": active,
        }))
    }
}

pub(crate) fn completed_marker(root: &Path, task_id: &str) -> std::path::PathBuf {
    directory(root)
        .join("batches")
        .join(format!("{task_id}.completed"))
}
/// Fold a durable completion marker into the in-memory journal. The marker is
/// written under the consumer lock alone, so a handler outcome survives workspace
/// lock contention; only a crash before the marker re-runs a handler.
pub(crate) fn apply_completed(root: &Path, journal: &mut Journal) -> Result<bool> {
    journal.validate_active()?;
    let Some(active) = journal.active.clone() else {
        return Ok(false);
    };
    let path = completed_marker(root, &active.task_id);
    let relative = format!(
        ".afmail/cloudflare-consumer/batches/{}.completed",
        active.task_id
    );
    let invalid = || {
        AppError::new("consumer_completion_invalid", "Consumer completion marker must be a bounded regular JSON document matching the active task")
        .with_hint("Restore the completion marker and its journal together from a trusted backup; do not discard notification state to retry work.")
        .with_details(serde_json::json!({"task_id": active.task_id, "path": relative}))
    };
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(AppError::io("inspect consumer completion marker", &error)
                .with_details(serde_json::json!({"path": relative})));
        }
    };
    const MAX_MARKER_BYTES: u64 = 4096;
    if !metadata.is_file() || metadata.len() > MAX_MARKER_BYTES {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_MARKER_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| {
            AppError::io("read consumer completion marker", &error)
                .with_details(serde_json::json!({"path": relative}))
        })?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(invalid());
    }
    let marker: CompletionMarker = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if marker.schema_name != "mail_received_task_completed"
        || marker.schema_version != 1
        || marker.task_id != active.task_id
    {
        return Err(invalid());
    }
    for key in &active.content_keys {
        if let Some(task) = journal.tasks.get_mut(key) {
            task.status = TaskStatus::Completed;
        }
    }
    journal.active = None;
    Ok(true)
}

#[cfg(all(test, feature = "cloudflare"))]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_live_work_and_receipts_that_can_still_be_consulted() {
        let now = 10 * RECEIPT_RETENTION_S;
        let receipt = |age: i64, cleanup_complete: bool| Receipt {
            message_id: "message".into(),
            imported_epoch_s: now - age,
            content_key: "content".into(),
            quota_managed: false,
            archive_key: None,
            cleanup_complete,
        };
        let task = |status| Task {
            message_id: "message".into(),
            status,
        };
        let mut journal = Journal::default();
        journal
            .receipts
            .insert("fresh".into(), receipt(RECEIPT_RETENTION_S - 1, true));
        journal
            .receipts
            .insert("expired".into(), receipt(RECEIPT_RETENTION_S, true));
        journal
            .receipts
            .insert("uncleaned".into(), receipt(RECEIPT_RETENTION_S * 3, false));
        journal
            .tasks
            .insert("preparing".into(), task(TaskStatus::Preparing));
        journal
            .tasks
            .insert("pending".into(), task(TaskStatus::Pending));
        journal
            .tasks
            .insert("completed".into(), task(TaskStatus::Completed));
        journal.prune(now);
        assert_eq!(
            journal.receipts.keys().collect::<Vec<_>>(),
            ["fresh", "uncleaned"]
        );
        assert_eq!(
            journal.tasks.keys().collect::<Vec<_>>(),
            ["pending", "preparing"]
        );
    }
}
