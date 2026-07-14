//! Typed, read-only facts for the AFUI mail reader and draft editor.
//!
//! The projection deliberately reuses the canonical message cache, case index,
//! draft frontmatter parser, validation logic, and attachment resolver. It never
//! reads generated Markdown views and never invents a second draft format.

use super::*;
use crate::types::{DraftStatus, DraftSyncIntent};

/// One locally cached message, ready for both the queue row and selected reader.
#[derive(Clone, Debug, Serialize)]
pub struct MailMessageRecord {
    pub message: Value,
    pub view: MailMessageView,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailMessageView {
    pub title: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub received_rfc3339: String,
    pub sent_rfc3339: String,
    pub direction: String,
    pub attachment_count: usize,
    pub workspace_status: String,
    pub unread: bool,
    pub body_text: String,
}

/// One draft from an active or archived local case.
#[derive(Clone, Debug, Serialize)]
pub struct DraftRecord {
    pub case_ref: String,
    pub case_name: String,
    pub group: String,
    pub case_archived: bool,
    pub draft_name: String,
    pub draft_path: String,
    pub reference: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    /// Agent/user-authored compose section only. Scaffolding, quoted history,
    /// and the identity footer stay in the draft file but are not form input.
    pub body: String,
    pub attachments: Vec<DraftAttachmentView>,
    pub status: DraftStatus,
    pub queued_action: Option<DraftSyncIntent>,
    pub push_id: Option<String>,
    pub remote_changed: bool,
    pub completeness: DraftCompleteness,
    pub validation: DraftValidationView,
    pub availability: DraftAvailabilityView,
    pub view: DraftDisplayView,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DraftAttachmentView {
    pub path: String,
    pub exists: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DraftValidationView {
    pub state: String,
    pub current_hash: String,
    pub recorded: bool,
    pub last_validated_hash: Option<String>,
    pub last_validated_rfc3339: Option<String>,
    pub error_code: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DraftAvailabilityView {
    pub editable: bool,
    pub attachments_available: bool,
    pub usable: bool,
    pub send_ready: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DraftDisplayView {
    pub title: String,
    pub case_label: String,
    pub to_text: String,
    pub cc_text: String,
}

impl Workspace {
    /// Every locally cached message in the store's canonical message-list order.
    pub(crate) fn mail_message_views(
        &self,
        message_list: &Value,
    ) -> Result<Vec<MailMessageRecord>> {
        let mut out = Vec::new();
        for item in value_array(message_list, "items") {
            let Some(message_id) = item.get("message_id").and_then(Value::as_str) else {
                continue;
            };
            let message = self.read_message_by_id(message_id)?;
            let message_value =
                read_json_file(&self.message_path(message_id), "read message json")?;
            let shown = self.message_show(message_id, false)?;
            out.push(MailMessageRecord {
                view: MailMessageView {
                    title: non_empty(message.subject.as_deref())
                        .unwrap_or_else(|| message_id.to_string()),
                    from: message.from.clone().unwrap_or_default(),
                    to: message.to.clone(),
                    cc: message.cc.clone(),
                    received_rfc3339: message.received_rfc3339.clone().unwrap_or_default(),
                    sent_rfc3339: message.sent_rfc3339.clone().unwrap_or_default(),
                    direction: message.direction.clone().unwrap_or_default(),
                    attachment_count: message.attachments.len(),
                    workspace_status: message.workspace.status.clone(),
                    unread: shown
                        .get("unread")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    body_text: shown
                        .get("body_text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                },
                message: message_value,
            });
        }
        Ok(out)
    }

    /// Every `drafts/*.md` under every local case, including archived cases.
    pub(crate) fn case_draft_views(&self, cases: &Value) -> Result<Vec<DraftRecord>> {
        let mut out = Vec::new();
        for item in value_array(cases, "items") {
            let Some(case_ref) = item.get("case_uid").and_then(Value::as_str) else {
                continue;
            };
            let case = self.resolve_case_any(case_ref)?;
            let case_name = item
                .get("case_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let drafts_dir = case.path.join("drafts");
            if !drafts_dir.exists() {
                continue;
            }
            let state = read_draft_state(&case.path)?;
            let mut paths = read_dir(&drafts_dir, "read drafts")?
                .into_iter()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                out.push(self.case_draft_view(
                    &case,
                    case_name,
                    &path,
                    state.drafts.get(&path_file_name(&path)),
                )?);
            }
        }
        out.sort_by(|a, b| {
            (a.case_ref.as_str(), a.draft_name.as_str())
                .cmp(&(b.case_ref.as_str(), b.draft_name.as_str()))
        });
        Ok(out)
    }

    fn case_draft_view(
        &self,
        case: &ResolvedCase,
        case_name: &str,
        path: &Path,
        state: Option<&DraftStateEntry>,
    ) -> Result<DraftRecord> {
        let draft_name = path_file_name(path);
        let text = read_to_string(path, "read draft")?;
        let current_hash = blake3_fingerprint(text.as_bytes());
        let validation_result = self.validate_draft_bytes_inner(
            &case.case_uid,
            &draft_name,
            &case.path,
            text.as_bytes(),
        );
        let parsed = read_doc::<DraftFrontmatter>(&text).map_err(|error| {
            AppError::new(
                "draft_invalid",
                format!("invalid draft frontmatter: {error}"),
            )
        });
        let (frontmatter, body, parse_error) = match parsed {
            Ok((frontmatter, body)) => (Some(frontmatter), body, None),
            Err(error) => (None, String::new(), Some(error)),
        };
        let content = parse_draft_content(&body);
        let authored_body = content
            .as_ref()
            .map(|content| {
                content
                    .authored_text()
                    .trim_matches(['\r', '\n'])
                    .to_string()
            })
            .unwrap_or_default();
        let completeness = content
            .as_ref()
            .map(|content| content.completeness())
            .unwrap_or(DraftCompleteness::Incomplete);
        let mut validation_error = validation_result.err().or(parse_error);
        let last_validated_hash = state.and_then(|value| value.last_validated_hash.clone());
        let last_validated_rfc3339 = state.and_then(|value| value.last_validated_rfc3339.clone());
        let recorded = last_validated_hash.as_deref() == Some(current_hash.as_str());

        let (to, cc, subject, attachments, sync_intent) = match frontmatter {
            Some(frontmatter) => {
                let attachments = frontmatter
                    .attachments
                    .iter()
                    .map(|attachment| {
                        serde_json::from_value::<DraftAttachmentView>(draft_attachment_show_value(
                            &case.path, attachment,
                        ))
                        .map_err(|error| AppError::json("deserialize draft attachment", &error))
                    })
                    .collect::<Result<Vec<_>>>()?;
                (
                    frontmatter.to,
                    frontmatter.cc,
                    frontmatter.subject.unwrap_or_default(),
                    attachments,
                    frontmatter.sync_intent,
                )
            }
            None => (Vec::new(), Vec::new(), String::new(), Vec::new(), None),
        };
        let queued =
            crate::push_queue::find_outbound_for_draft(&self.root, &case.case_uid, &draft_name)?;
        let projection = DraftStateProjection::from_queue(sync_intent, queued.as_ref());
        if let Some(issue) = projection.issue
            && validation_error.is_none()
        {
            validation_error = Some(AppError::new(
                "draft_invalid",
                format!("{}: {draft_name}", issue.message()),
            ));
        }
        let status = if validation_error.is_some() {
            DraftStatus::Failed
        } else {
            projection.status
        };
        let attachments_available = attachments.iter().all(|attachment| attachment.exists);
        let current_valid = validation_error.is_none();
        let editable = !case.archived && status == DraftStatus::Editable;
        let error_code = validation_error
            .as_ref()
            .map(|error| error.error_code.to_string());
        let error = validation_error.as_ref().map(|error| error.message.clone());
        let validation_state = if current_valid { "valid" } else { "invalid" };
        let reference = format!("{}/{}", case.case_uid, draft_name);
        let to_text = to.join(", ");
        let cc_text = cc.join(", ");
        Ok(DraftRecord {
            case_ref: case.case_uid.clone(),
            case_name: case_name.to_string(),
            group: case.group.clone(),
            case_archived: case.archived,
            draft_name: draft_name.clone(),
            draft_path: rel_path(&self.root, path),
            reference,
            to,
            cc,
            subject: subject.clone(),
            body: authored_body,
            attachments,
            status,
            queued_action: projection.queued_action,
            push_id: queued.map(|item| item.push_id),
            remote_changed: false,
            completeness,
            validation: DraftValidationView {
                state: validation_state.to_string(),
                current_hash,
                recorded,
                last_validated_hash,
                last_validated_rfc3339,
                error_code,
                error,
            },
            availability: DraftAvailabilityView {
                editable,
                attachments_available,
                usable: current_valid && attachments_available,
                send_ready: current_valid
                    && attachments_available
                    && completeness == DraftCompleteness::Complete,
            },
            view: DraftDisplayView {
                title: if subject.trim().is_empty() {
                    draft_name
                } else {
                    subject
                },
                case_label: if case_name.trim().is_empty() {
                    case.case_uid.clone()
                } else {
                    case_name.to_string()
                },
                to_text,
                cc_text,
            },
        })
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
    serde_json::from_str(&data).map_err(|error| AppError::json(context, &error))
}
