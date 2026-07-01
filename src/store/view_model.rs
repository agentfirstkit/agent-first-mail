//! Typed projection from canonical data to view-ready **facts**.
//!
//! This is the single place that turns canonical records (the JSON files plus
//! the human-authored `notes.md`) into the per-entity facts the AFUI snapshot
//! references. Each row mirrors the local Markdown render context shape:
//! canonical JSON records live under their domain namespace (`message`, `case`,
//! `notification`, `archive`, `push`, or `item`) and AFUI-only/read-model data lives under
//! `view`.
//!
//! Nothing here reads a generated `*.md` view; case/notification/archive summaries come from
//! `case.json` (`CaseFrontmatter` stores the counts) and `notification.json`
//! (`NotificationMessages` stores each item's summary). The markdown templates and
//! this projection share the same canonical sources, so the two never drift.

use super::*;

/// One untriaged inbox message.
#[derive(Clone, Debug, Serialize)]
pub struct InboxMessageRecord {
    pub message: Value,
    pub view: InboxMessageView,
}

#[derive(Clone, Debug, Serialize)]
pub struct InboxMessageView {
    pub title: String,
    pub from: String,
    pub to: Vec<String>,
    pub received_rfc3339: String,
    pub attachment_count: usize,
    pub status: String,
    pub body_text: String,
}

/// One active case (summary row; the conversation is not inlined).
#[derive(Clone, Debug, Serialize)]
pub struct CaseSummaryRecord {
    pub case: Value,
    pub view: CaseSummaryView,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseSummaryView {
    pub group: String,
    pub case_dir: String,
    pub updated_rfc3339: String,
    pub last_message_rfc3339: String,
    pub notes_text: String,
}

/// One archived case (closed; kept for lookup).
#[derive(Clone, Debug, Serialize)]
pub struct ArchiveCaseSummaryRecord {
    pub case: Value,
    pub view: ArchiveCaseSummaryView,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArchiveCaseSummaryView {
    pub group: String,
    pub case_dir: String,
    pub updated_rfc3339: String,
    pub notes_text: String,
}

/// One notification message inside a collection.
#[derive(Clone, Debug, Serialize)]
pub struct NotificationItemRecord {
    pub item: Value,
    pub message: Value,
    pub view: NotificationItemView,
}

#[derive(Clone, Debug, Serialize)]
pub struct NotificationItemView {
    pub summary: String,
    pub added_rfc3339: String,
}

/// One notification collection.
#[derive(Clone, Debug, Serialize)]
pub struct NotificationRecord {
    pub notification: Value,
    pub view: NotificationView,
    pub items: Vec<NotificationItemRecord>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NotificationView {
    pub notification_dir: String,
    pub updated_rfc3339: String,
    pub notes_text: String,
}

/// One queued remote mailbox effect.
#[derive(Clone, Debug, Serialize)]
pub struct PushSummaryRecord {
    pub push: Value,
    pub view: PushSummaryView,
}

#[derive(Clone, Debug, Serialize)]
pub struct PushSummaryView {
    pub display_kind: String,
    pub step_count: usize,
}

impl Workspace {
    /// Inbox messages with their facts resolved from the message cache.
    pub(crate) fn inbox_message_views(&self, triage: &Value) -> Result<Vec<InboxMessageRecord>> {
        let mut out = Vec::new();
        for item in value_array(triage, "items") {
            let Some(message_id) = item.get("message_id").and_then(Value::as_str) else {
                continue;
            };
            let message = self.read_message_by_id(message_id)?;
            let message_value =
                read_json_file(&self.message_path(message_id), "read message json")?;
            out.push(InboxMessageRecord {
                view: InboxMessageView {
                    title: non_empty(message.subject.as_deref())
                        .unwrap_or_else(|| message_id.to_string()),
                    from: message.from.clone().unwrap_or_default(),
                    to: message.to.clone(),
                    received_rfc3339: message.received_rfc3339.clone().unwrap_or_default(),
                    attachment_count: message.attachments.len(),
                    status: message.workspace.status.clone(),
                    body_text: message.body_text.clone(),
                },
                message: message_value,
            });
        }
        Ok(out)
    }

    /// Active cases as summary rows built from `case.json` + `notes.md`.
    pub(crate) fn case_summary_views(&self, cases: &Value) -> Result<Vec<CaseSummaryRecord>> {
        let mut out = Vec::new();
        for item in value_array(cases, "items") {
            let (Some(group), Some(case_dir)) = (
                item.get("group").and_then(Value::as_str),
                item.get("case_dir").and_then(Value::as_str),
            ) else {
                continue;
            };
            let path = self.root.join("cases").join(group).join(case_dir);
            let case = read_case_file(&path)?;
            let notes_text = read_optional_string(&path.join("notes.md"))?.unwrap_or_default();
            let case_value = read_json_file(&case_json_path(&path), "read case metadata")?;
            out.push(CaseSummaryRecord {
                case: case_value,
                view: CaseSummaryView {
                    group: group.to_string(),
                    case_dir: case_dir.to_string(),
                    updated_rfc3339: case.updated_rfc3339.unwrap_or_default(),
                    last_message_rfc3339: case.last_message_rfc3339.unwrap_or_default(),
                    notes_text,
                },
            });
        }
        Ok(out)
    }

    /// Archived cases as summary rows.
    pub(crate) fn archive_case_summary_views(
        &self,
        archive: &Value,
    ) -> Result<Vec<ArchiveCaseSummaryRecord>> {
        let mut out = Vec::new();
        for item in value_array(archive, "cases") {
            let (Some(group), Some(case_dir)) = (
                item.get("group").and_then(Value::as_str),
                item.get("case_dir").and_then(Value::as_str),
            ) else {
                continue;
            };
            let path = self.root.join("archive/cases").join(group).join(case_dir);
            let case = read_case_file(&path)?;
            let notes_text = read_optional_string(&path.join("notes.md"))?.unwrap_or_default();
            let updated_rfc3339 = case
                .updated_rfc3339
                .clone()
                .or_else(|| case.archived_rfc3339.clone())
                .unwrap_or_default();
            let case_value = read_json_file(&case_json_path(&path), "read archived case metadata")?;
            out.push(ArchiveCaseSummaryRecord {
                case: case_value,
                view: ArchiveCaseSummaryView {
                    group: group.to_string(),
                    case_dir: case_dir.to_string(),
                    updated_rfc3339,
                    notes_text,
                },
            });
        }
        Ok(out)
    }

    /// Notification collections built from `notification.json` + `notes.md`.
    pub(crate) fn notification_views(
        &self,
        notifications: &Value,
    ) -> Result<Vec<NotificationRecord>> {
        let mut out = Vec::new();
        for item in value_array(notifications, "items") {
            let Some(notification_uid) = item.get("notification_uid").and_then(Value::as_str)
            else {
                continue;
            };
            let data = self.read_notifications(notification_uid)?;
            let notification_value = read_json_file(
                &self.notification_json_path(notification_uid),
                "read notifications",
            )?;
            let dir = self.notification_dir(notification_uid);
            let notification_dir = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let notes_text = read_optional_string(&dir.join("notes.md"))?.unwrap_or_default();
            let updated_rfc3339 = data
                .items
                .iter()
                .map(|entry| entry.added_rfc3339.as_str())
                .max()
                .unwrap_or_default()
                .to_string();
            let raw_items = notification_value
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    AppError::new(
                        "notifications_invalid",
                        "notifications JSON must contain an items array",
                    )
                })?;
            let mut items = Vec::new();
            for (index, entry) in data.items.iter().enumerate() {
                self.read_message_by_id(&entry.message_id)?;
                let message_value =
                    read_json_file(&self.message_path(&entry.message_id), "read message json")?;
                let item_value = raw_items.get(index).cloned().ok_or_else(|| {
                    AppError::new(
                        "notifications_invalid",
                        "notifications JSON item count changed while building UI state",
                    )
                })?;
                items.push(NotificationItemRecord {
                    item: item_value,
                    message: message_value,
                    view: NotificationItemView {
                        summary: entry.summary.clone().unwrap_or_default(),
                        added_rfc3339: entry.added_rfc3339.clone(),
                    },
                });
            }
            out.push(NotificationRecord {
                notification: notification_value,
                view: NotificationView {
                    notification_dir,
                    updated_rfc3339,
                    notes_text,
                },
                items,
            });
        }
        Ok(out)
    }

    /// Queued mailbox effects, projected from the push list.
    pub(crate) fn push_summary_views(&self, push: &Value) -> Result<Vec<PushSummaryRecord>> {
        let mut out = Vec::new();
        for item in value_array(push, "items") {
            out.push(PushSummaryRecord {
                push: item.clone(),
                view: PushSummaryView {
                    display_kind: item
                        .get("display_kind")
                        .or_else(|| item.get("action"))
                        .or_else(|| item.get("kind"))
                        .and_then(Value::as_str)
                        .unwrap_or("push item")
                        .to_string(),
                    step_count: item
                        .get("step_states")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0),
                },
            });
        }
        Ok(out)
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn value_array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn read_json_file(path: &Path, context: &str) -> Result<Value> {
    let data = read_to_string(path, context)?;
    serde_json::from_str(&data).map_err(|e| AppError::json(context, &e))
}

fn read_optional_string(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::io("read view-model text", &err)),
    }
}
