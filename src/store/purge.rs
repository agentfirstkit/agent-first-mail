use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PurgeDisposition {
    Spam,
    Trash,
    Deleted,
}

impl PurgeDisposition {
    fn status(self) -> MessageStatus {
        match self {
            Self::Spam => MessageStatus::Spam,
            Self::Trash => MessageStatus::Trashed,
            Self::Deleted => MessageStatus::DeletedRemote,
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::Trash => "trash",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Default)]
struct PurgeTotals {
    purged_message_ids: Vec<String>,
    purged_spam_count: usize,
    purged_trash_count: usize,
    purged_deleted_count: usize,
    skipped_referenced_message_ids: Vec<String>,
    skipped_recent_message_ids: Vec<String>,
}

impl PurgeTotals {
    fn record_purged(&mut self, target: PurgeDisposition, message_id: String) {
        match target {
            PurgeDisposition::Spam => self.purged_spam_count += 1,
            PurgeDisposition::Trash => self.purged_trash_count += 1,
            PurgeDisposition::Deleted => self.purged_deleted_count += 1,
        }
        self.purged_message_ids.push(message_id);
    }
}

impl Workspace {
    pub fn purge_spam(&self, older_than_days: u64) -> Result<Value> {
        self.purge_dispositions(&[PurgeDisposition::Spam], older_than_days)
    }

    pub fn purge_trash(&self, older_than_days: u64) -> Result<Value> {
        self.purge_dispositions(&[PurgeDisposition::Trash], older_than_days)
    }

    pub fn purge_deleted(&self, older_than_days: u64) -> Result<Value> {
        self.purge_dispositions(&[PurgeDisposition::Deleted], older_than_days)
    }

    pub fn purge_discards(&self, older_than_days: u64) -> Result<Value> {
        self.purge_dispositions(
            &[
                PurgeDisposition::Spam,
                PurgeDisposition::Trash,
                PurgeDisposition::Deleted,
            ],
            older_than_days,
        )
    }

    fn purge_dispositions(
        &self,
        targets: &[PurgeDisposition],
        older_than_days: u64,
    ) -> Result<Value> {
        self.require_workspace()?;
        let cutoff = Utc::now() - Duration::days(older_than_days as i64);
        let targets_by_status = targets
            .iter()
            .copied()
            .map(|target| (target.status(), target))
            .collect::<BTreeMap<_, _>>();
        let mut totals = PurgeTotals::default();

        for path in message_json_paths(&self.root)? {
            let message = read_message(&path)?;
            let status = MessageStatus::parse(&message.workspace.status)?;
            let Some(target) = targets_by_status.get(&status).copied() else {
                continue;
            };
            let age_time = self.message_purge_age_time(&message)?;
            if age_time > cutoff {
                totals
                    .skipped_recent_message_ids
                    .push(message.message_id.clone());
                continue;
            }
            if self.message_id_is_referenced(&message.message_id)? {
                totals
                    .skipped_referenced_message_ids
                    .push(message.message_id.clone());
                continue;
            }
            let message_id = message.message_id.clone();
            purge_message_artifacts(&self.root, &message_id)?;
            totals.record_purged(target, message_id);
        }

        let dispositions = self.refresh_disposition_views()?;
        let mut result = json!({
            "code": "purged",
            "target": purge_target_name(targets),
            "targets": targets.iter().map(|target| target.target()).collect::<Vec<_>>(),
            "older_than_days": older_than_days,
            "purged_count": totals.purged_message_ids.len(),
            "purged_message_ids": totals.purged_message_ids,
            "purged_spam_count": totals.purged_spam_count,
            "purged_trash_count": totals.purged_trash_count,
            "purged_deleted_count": totals.purged_deleted_count,
            "skipped_referenced_count": totals.skipped_referenced_message_ids.len(),
            "skipped_referenced_message_ids": totals.skipped_referenced_message_ids,
            "skipped_recent_count": totals.skipped_recent_message_ids.len(),
            "skipped_recent_message_ids": totals.skipped_recent_message_ids,
        });
        merge_disposition_refresh_into_purge(&mut result, &dispositions);
        Ok(result)
    }

    fn message_purge_age_time(&self, message: &MessageFile) -> Result<DateTime<Utc>> {
        let value = message_state_updated_rfc3339(&self.root, &message.message_id)?
            .or_else(|| message.received_rfc3339.clone())
            .or_else(|| message.sent_rfc3339.clone())
            .unwrap_or_else(now_rfc3339);
        Ok(DateTime::parse_from_rfc3339(&value)
            .map(|time| time.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()))
    }
}

fn purge_target_name(targets: &[PurgeDisposition]) -> &'static str {
    if targets == [PurgeDisposition::Spam] {
        "spam"
    } else if targets == [PurgeDisposition::Trash] {
        "trash"
    } else if targets == [PurgeDisposition::Deleted] {
        "deleted"
    } else {
        "discards"
    }
}

fn merge_disposition_refresh_into_purge(purge: &mut Value, dispositions: &Value) {
    let Some(purge_obj) = purge.as_object_mut() else {
        return;
    };
    let Some(disposition_obj) = dispositions.as_object() else {
        return;
    };
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
        if let Some(value) = disposition_obj.get(key) {
            purge_obj.insert(key.to_string(), value.clone());
        }
    }
}
