use agent_first_ui::{
    validate_document, validate_state_bindings, Action, AfuiMessage, Document, Risk, Screen, State,
    Surface, View,
};
use serde_json::{json, Map, Value};

use crate::config::{MailConfig, TemplateLanguage};
use crate::error::{AppError, Result};
use crate::store::Workspace;

pub fn workspace_snapshot(workspace: &Workspace) -> Result<AfuiMessage> {
    let mut status = workspace.status()?;
    if let Value::Object(map) = &mut status {
        map.insert(
            "progress".to_string(),
            crate::progress::workspace_status_progress(workspace.root())?,
        );
    }

    let triage = workspace.triage_list()?;
    let cases = workspace.case_list()?;
    let archive = workspace.archive_list()?;
    let push = workspace.push_list()?;
    let logs = workspace.log_list(20)?;
    let language = MailConfig::load(workspace.root())?.template_language();
    let state = json!({
        "workspace": {
            "path": workspace.root().to_string_lossy().replace('\\', "/"),
            "version": env!("CARGO_PKG_VERSION"),
        },
        "status": status,
        "ui": {
            "triage_items": to_items(workspace.inbox_message_views(&triage)?)?,
            "case_items": to_items(workspace.case_summary_views(&cases)?)?,
            "archive_case_items": to_items(workspace.archive_case_summary_views(&archive)?)?,
            "archive_message_items": to_items(workspace.archive_category_views(&archive)?)?,
            "push_items": to_items(workspace.push_summary_views(&push)?)?,
            "log_items": log_items(&logs),
        },
        "logs": logs,
        "terminals": {
            "mail_terminal": {
                "session_id": "mail_terminal",
                "title": "Mail Terminal",
                "status": "host-owned local terminal"
            }
        }
    });

    let document = Document::new(
        Surface {
            id: "afmail".to_string(),
            title: "Agent-First Mail".to_string(),
            description: Some(
                "Dockview-ready local mail workspace. Buttons emit AFUI user_action events; afmail effects still require normal command confirmation boundaries."
                    .to_string(),
            ),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            extra: Map::new(),
        },
        vec![Screen {
            id: "mail_workspace".to_string(),
            title: Some("Mail Workspace".to_string()),
            description: Some(
                "Dockview supplies the workspace chrome; AFMail provides data panels, bindings, and safe actions."
                    .to_string(),
            ),
            views: vec![
                mailbox_status(),
                inbox_triage(),
                active_cases(language),
                push_queue(language),
                message_archives(language),
                case_archives(language),
                activity_log(),
                mail_terminal(),
            ],
            extra: Map::new(),
        }],
    )
    .with_actions(actions());
    validate_document(&document).map_err(|e| {
        AppError::new(
            "afui_invalid",
            format!("generated AFUI document did not validate: {e}"),
        )
    })?;
    let state = State::try_from_value(state).map_err(|e| {
        AppError::new(
            "afui_invalid",
            format!("generated AFUI state is invalid: {e}"),
        )
    })?;
    validate_state_bindings(&document, state.as_value()).map_err(|e| {
        AppError::new(
            "afui_invalid",
            format!("AFUI document references state that is not present: {e}"),
        )
    })?;
    Ok(AfuiMessage::ui_snapshot(document, state))
}

/// Serialize a typed view-model projection into the AFUI state's item array.
fn to_items<T: serde::Serialize>(views: Vec<T>) -> Result<Value> {
    serde_json::to_value(views).map_err(|e| AppError::json("serialize view-model items", &e))
}

fn log_items(logs: &Value) -> Value {
    let Some(events) = logs.get("events").and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };
    Value::Array(events.iter().map(log_item).collect())
}

fn log_item(event: &Value) -> Value {
    let kind = event_string(event, "kind")
        .or_else(|| event_string(event, "code"))
        .unwrap_or("event")
        .to_string();
    let summary = event_string(event, "message")
        .or_else(|| event_string(event, "summary"))
        .or_else(|| event_string(event, "reason"))
        .map(str::to_string)
        .or_else(|| target_summary(event))
        .unwrap_or_else(|| kind.clone());
    json!({
        "created_rfc3339": event_string(event, "created_rfc3339").unwrap_or_default(),
        "kind": kind,
        "message": summary.clone(),
        "summary": summary,
        "event": event,
    })
}

fn event_string<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn target_summary(event: &Value) -> Option<String> {
    let targets = event.get("targets")?.as_array()?;
    let labels = targets
        .iter()
        .filter_map(|target| {
            let kind = event_string(target, "kind")?;
            let id = event_string(target, "id")?;
            Some(format!("{kind}:{id}"))
        })
        .collect::<Vec<_>>();
    (!labels.is_empty()).then(|| labels.join(", "))
}

fn mailbox_status() -> View {
    View::new("mailbox_status", "stats")
        .set_field("title", json!("Mailbox Status"))
        .set_field("dockview", dockview_hint("utility", 30, None, Some(170)))
        .set_field(
            "description",
            json!("Compact workspace counts; Dockview keeps it as a utility tab, not a primary workspace column."),
        )
        .set_field(
            "items",
            json!([
                {"label": "Inbox", "value_bind": "state:status.triage_count"},
                {"label": "Active cases", "value_bind": "state:status.case_count"},
                {"label": "Push queue", "value_bind": "state:status.push_count"},
                {"label": "Message archives", "value_bind": "state:status.archive_message_category_count"},
                {"label": "Case archives", "value_bind": "state:status.archived_case_count"},
                {"label": "Activity", "value_bind": "state:logs.count"}
            ]),
        )
        .set_field("actions", json!(["afmail.status", "afmail.pull"]))
}

fn inbox_triage() -> View {
    View::new("inbox_triage", "record_list")
        .set_field("title", json!("Inbox Triage"))
        .set_field("dockview", dockview_hint("main", 10, Some(920), None))
        .set_field(
            "description",
            json!("Unprocessed messages ready to read, case, archive, or discard."),
        )
        .set_field("source_bind", json!("state:ui.triage_items"))
        .set_field("count_bind", json!("state:status.triage_count"))
        .set_field("message_id_bind", json!("record:message.message_id"))
        .set_field("id_key", json!("message.message_id"))
        .set_field("ref_bind", json!("record:message.message_id"))
        .set_field("ref_label", json!("Copy message id"))
        .set_field("title_key", json!("view.title"))
        .set_field("subtitle_key", json!("view.from"))
        .set_field("meta_key", json!("view.received_rfc3339"))
        .set_field("body_key", json!("view.body_text"))
        .set_field("empty_text", json!("Inbox zero — nothing to triage."))
        .set_field(
            "fields",
            json!([
                {"key": "view.from", "label": "From"},
                {"key": "view.to", "label": "To"},
                {"key": "view.received_rfc3339", "label": "Received"},
                {"key": "view.attachment_count", "label": "Attachments"}
            ]),
        )
        .set_field(
            "row_actions",
            json!([
                "afmail.case.create",
                "afmail.archive.message.add",
                "afmail.message.trash"
            ]),
        )
}

fn active_cases(language: TemplateLanguage) -> View {
    View::new("active_cases", "record_list")
        .set_field("title", json!("Active Cases"))
        .set_field("dockview", dockview_hint("main", 20, Some(920), None))
        .set_field(
            "description",
            json!("Live work items with markdown notes and message counts."),
        )
        .set_field("source_bind", json!("state:ui.case_items"))
        .set_field("count_bind", json!("state:status.case_count"))
        .set_field("case_ref_bind", json!("record:case.collection_uid"))
        .set_field("id_key", json!("case.collection_uid"))
        .set_field("ref_bind", json!("record:case.collection_uid"))
        .set_field("ref_label", json!("Copy case id"))
        .set_field("title_key", json!("case.collection_name"))
        .set_field("subtitle_key", json!("case.status"))
        .set_field("meta_key", json!("view.updated_rfc3339"))
        .set_field("badge_key", json!("case.message_count"))
        .set_field("body_key", json!("view.notes_text"))
        .set_field("body_format", json!("markdown"))
        .set_field("empty_text", json!("No active cases."))
        .set_field(
            "labels",
            count_labels(
                language,
                "case.message_count",
                "message",
                "messages",
                "封邮件",
            ),
        )
        .set_field(
            "fields",
            json!([
                {"key": "view.group", "label": "Group"},
                {"key": "case.status", "label": "Status"},
                {"key": "case.message_count", "label": "Messages"},
                {"key": "case.tags", "label": "Tags"},
                {"key": "view.last_message_rfc3339", "label": "Last message"},
                {"key": "view.updated_rfc3339", "label": "Updated"}
            ]),
        )
        .set_field(
            "row_actions",
            json!([
                "afmail.case.show",
                "afmail.case.notes.append",
                "afmail.case.archive"
            ]),
        )
}

fn case_archives(language: TemplateLanguage) -> View {
    View::new("case_archives", "record_list")
        .set_field("title", json!("Case Archives"))
        .set_field("dockview", dockview_hint("main", 50, Some(880), None))
        .set_field(
            "description",
            json!("Closed cases kept for lookup and restore."),
        )
        .set_field("source_bind", json!("state:ui.archive_case_items"))
        .set_field("count_bind", json!("state:status.archived_case_count"))
        .set_field("case_ref_bind", json!("record:case.collection_uid"))
        .set_field("id_key", json!("case.collection_uid"))
        .set_field("ref_bind", json!("record:case.collection_uid"))
        .set_field("ref_label", json!("Copy case id"))
        .set_field("title_key", json!("case.collection_name"))
        .set_field("subtitle_key", json!("case.status"))
        .set_field("meta_key", json!("view.updated_rfc3339"))
        .set_field("badge_key", json!("case.message_count"))
        .set_field("body_key", json!("view.notes_text"))
        .set_field("body_format", json!("markdown"))
        .set_field("empty_text", json!("No archived cases."))
        .set_field(
            "labels",
            count_labels(
                language,
                "case.message_count",
                "message",
                "messages",
                "封邮件",
            ),
        )
        .set_field(
            "fields",
            json!([
                {"key": "case.collection_uid", "label": "Case"},
                {"key": "case.message_count", "label": "Messages"},
                {"key": "view.updated_rfc3339", "label": "Updated"}
            ]),
        )
        .set_field(
            "row_actions",
            json!(["afmail.archive.case.show", "afmail.archive.case.restore"]),
        )
}

fn message_archives(language: TemplateLanguage) -> View {
    View::new("message_archives", "record_list")
        .set_field("title", json!("Message Archives"))
        .set_field("dockview", dockview_hint("main", 40, Some(900), None))
        .set_field(
            "description",
            json!("Archived message buckets and their notes."),
        )
        .set_field("source_bind", json!("state:ui.archive_message_items"))
        .set_field(
            "count_bind",
            json!("state:status.archive_message_category_count"),
        )
        .set_field("archive_ref_bind", json!("record:archive.collection_uid"))
        .set_field("id_key", json!("archive.collection_uid"))
        .set_field("ref_bind", json!("record:archive.collection_uid"))
        .set_field("ref_label", json!("Copy archive id"))
        .set_field("title_key", json!("archive.collection_name"))
        .set_field("subtitle_key", json!("archive.collection_uid"))
        .set_field("meta_key", json!("view.updated_rfc3339"))
        .set_field("badge_key", json!("archive.message_count"))
        .set_field("body_key", json!("view.notes_text"))
        .set_field("body_format", json!("markdown"))
        .set_field("empty_text", json!("No message archives."))
        .set_field(
            "labels",
            count_labels(
                language,
                "archive.message_count",
                "message",
                "messages",
                "封邮件",
            ),
        )
        .set_field(
            "fields",
            json!([
                {"key": "archive.collection_uid", "label": "Archive"},
                {"key": "archive.message_count", "label": "Messages"},
                {"key": "view.updated_rfc3339", "label": "Updated"}
            ]),
        )
        .set_field(
            "row_actions",
            json!([
                "afmail.archive.message.show",
                "afmail.archive.message.restore",
                "afmail.archive.message.rename"
            ]),
        )
}

fn push_queue(language: TemplateLanguage) -> View {
    View::new("push_queue", "record_list")
        .set_field("title", json!("Push Queue"))
        .set_field("dockview", dockview_hint("main", 30, Some(840), None))
        .set_field(
            "description",
            json!("Queued remote mailbox effects waiting for dry-run or confirmation."),
        )
        .set_field("source_bind", json!("state:ui.push_items"))
        .set_field("count_bind", json!("state:status.push_count"))
        .set_field("push_id_bind", json!("record:push.push_id"))
        .set_field("id_key", json!("push.push_id"))
        .set_field("ref_bind", json!("record:push.push_id"))
        .set_field("ref_label", json!("Copy push id"))
        .set_field("title_key", json!("view.display_kind"))
        .set_field("subtitle_key", json!("push.push_id"))
        .set_field("meta_key", json!("push.updated_rfc3339"))
        .set_field("badge_key", json!("view.step_count"))
        .set_field("empty_text", json!("Nothing queued to send."))
        .set_field(
            "labels",
            count_labels(language, "view.step_count", "step", "steps", "步"),
        )
        .set_field(
            "fields",
            json!([
                {"key": "push.push_id", "label": "Item"},
                {"key": "view.display_kind", "label": "Type"},
                {"key": "push.attempt_count", "label": "Attempts"},
                {"key": "view.step_count", "label": "Steps"},
                {"key": "push.created_rfc3339", "label": "Created"},
                {"key": "push.updated_rfc3339", "label": "Updated"}
            ]),
        )
        .set_field(
            "row_actions",
            json!([
                "afmail.push.list",
                "afmail.push.dry_run",
                "afmail.push.confirm"
            ]),
        )
}

fn activity_log() -> View {
    View::new("activity_log", "log")
        .set_field("title", json!("Activity"))
        .set_field("dockview", dockview_hint("utility", 10, None, Some(260)))
        .set_field(
            "description",
            json!("Recent audit events from the local AFMail workspace."),
        )
        .set_field("source_bind", json!("state:ui.log_items"))
        .set_field("level_key", json!("kind"))
        .set_field("message_key", json!("message"))
        .set_field("time_key", json!("created_rfc3339"))
        .set_field("empty_text", json!("No activity yet."))
}

/// Build an advisory `labels` hint mapping a numeric field to a per-language
/// count template. The host fills `{n}`; the producer supplies the wording.
fn count_labels(
    language: TemplateLanguage,
    field: &str,
    en_one: &str,
    en_other: &str,
    zh: &str,
) -> Value {
    let template = match language {
        TemplateLanguage::EnUs => {
            json!({ "one": format!("{{n}} {en_one}"), "other": format!("{{n}} {en_other}") })
        }
        TemplateLanguage::ZhCn => {
            json!({ "one": format!("{{n}} {zh}"), "other": format!("{{n}} {zh}") })
        }
    };
    let mut map = Map::new();
    map.insert(field.to_string(), template);
    Value::Object(map)
}

fn dockview_hint(
    role: &str,
    priority: u32,
    preferred_width: Option<u32>,
    preferred_height: Option<u32>,
) -> Value {
    let mut map = Map::new();
    map.insert("role".to_string(), json!(role));
    map.insert("priority".to_string(), json!(priority));
    if let Some(width) = preferred_width {
        map.insert("preferred_width".to_string(), json!(width));
    }
    if let Some(height) = preferred_height {
        map.insert("preferred_height".to_string(), json!(height));
    }
    Value::Object(map)
}

fn mail_terminal() -> View {
    View::new("mail_terminal", "terminal")
        .set_field("dockview", dockview_hint("utility", 20, None, Some(300)))
        .set_field(
            "session_bind",
            json!("state:terminals.mail_terminal.session_id"),
        )
        .set_field("title_bind", json!("state:terminals.mail_terminal.title"))
        .set_field("status_bind", json!("state:terminals.mail_terminal.status"))
        .set_field("title", json!("Mail Terminal"))
}

fn actions() -> Vec<Action> {
    vec![
        read_action("afmail.status", "Status"),
        external_action(
            "afmail.pull",
            "Pull",
            "Reads configured IMAP mailboxes into local files. It reaches the remote mailbox but does not apply remote writes.",
        )
        .with_input(input_schema(&[field("ids", "array", false)])),
        read_action("afmail.ui.snapshot", "UI Snapshot"),
        read_action("afmail.config.show", "Config Show"),
        read_action("afmail.config.get", "Config Get")
            .with_input(input_schema(&[field("key", "string", true)])),
        local_action("afmail.config.set", "Config Set")
            .with_input(input_schema(&[
                field("key", "string", true),
                field("values", "array", true),
            ])),
        external_action("afmail.remote.test", "Remote Test", "Tests configured IMAP login."),
        external_action("afmail.remote.folders", "Remote Folders", "Lists remote IMAP folders."),
        read_action("afmail.push.list", "Push List"),
        read_action("afmail.push.dry_run", "Push Dry Run")
            .with_input(input_schema(&[field("push_id", "string", false)])),
        destructive_action(
            "afmail.push.confirm",
            "Push Confirm",
            "Applies all queued mailbox effects and may send mail.",
        )
        .with_input(input_schema(&[field("push_id", "string", false)])),
        read_action("afmail.push.drafts.dry_run", "Draft Save Dry Run"),
        external_action(
            "afmail.push.drafts.confirm",
            "Save Drafts",
            "Saves queued drafts to the configured remote Drafts mailbox.",
        ),
        read_action("afmail.push.drafts_send.dry_run", "Send Drafts Dry Run"),
        destructive_action(
            "afmail.push.drafts_send.confirm",
            "Send Drafts",
            "Sends queued outbound mail and records related mailbox effects.",
        ),
        read_action("afmail.push.archive.dry_run", "Archive Push Dry Run"),
        external_action(
            "afmail.push.archive.confirm",
            "Push Archive",
            "Applies queued archive mailbox moves.",
        ),
        read_action("afmail.push.spam.dry_run", "Spam Push Dry Run"),
        external_action("afmail.push.spam.confirm", "Push Spam", "Applies queued Junk moves."),
        read_action("afmail.push.trash.dry_run", "Trash Push Dry Run"),
        external_action(
            "afmail.push.trash.confirm",
            "Push Trash",
            "Applies queued Trash moves.",
        ),
        read_action("afmail.doctor", "Doctor"),
        local_action("afmail.doctor.repair", "Doctor Repair"),
        destructive_action(
            "afmail.purge",
            "Purge Discards",
            "Permanently deletes old local discard records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        destructive_action(
            "afmail.purge.spam",
            "Purge Spam",
            "Permanently deletes old local spam records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        destructive_action(
            "afmail.purge.trash",
            "Purge Trash",
            "Permanently deletes old local trash records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        destructive_action(
            "afmail.purge.deleted",
            "Purge Deleted",
            "Permanently deletes old local remote-deleted records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        read_action("afmail.triage.list", "Triage List"),
        read_action("afmail.message.show", "Show Message")
            .with_input(input_schema(&[field("message_id", "string", true)])),
        local_action("afmail.archive.message.add", "Add To Archive")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("summary", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.message.spam", "Spam")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.message.trash", "Trash")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.message.restore", "Restore")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.message.attachment.fetch", "Fetch Attachment")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("part_id", "string", false),
            ])),
        local_action("afmail.case.create", "Create Case")
            .with_input(input_schema(&[
                field("name", "string", true),
                field("group", "string", false),
                field("message_id", "string", false),
                field("reason", "string", false),
            ])),
        read_action("afmail.case.list", "Case List"),
        read_action("afmail.case.show", "Show Case")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_action("afmail.case.add", "Add Message")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.case.move", "Move Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("group", "string", true),
            ])),
        local_action("afmail.case.rename", "Rename Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("name", "string", true),
                field("reason", "string", false),
            ])),
        read_action("afmail.case.notes.show", "Show Case Notes")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_action("afmail.case.notes.append", "Append Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_action("afmail.case.notes.replace", "Replace Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_action("afmail.case.archive", "Archive Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.case.reopen", "Reopen Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.case.tag", "Tag Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("tag", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.case.untag", "Untag Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("tag", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.case.draft.new", "New Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("to", "array", true),
                field("cc", "array", false),
                field("subject", "string", false),
            ])),
        local_action("afmail.case.draft.validate", "Validate Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
            ])),
        local_action("afmail.case.draft.change", "Change Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
                field("subject", "string", false),
                field("to", "array", false),
                field("cc", "array", false),
                field("clear_cc", "boolean", false),
                field("body", "string", false),
                field("body_file", "string", false),
            ])),
        local_action("afmail.case.draft.save", "Queue Draft Save")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
            ])),
        local_action("afmail.case.draft.send", "Queue Draft Send")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
            ])),
        local_action("afmail.case.draft.attach", "Attach Draft File")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
                field("path", "string", true),
            ])),
        local_action("afmail.case.draft.remove", "Remove Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.case.draft.reply", "Reply Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("message_id", "string", true),
                field("body", "string", false),
                field("body_file", "string", false),
                field("all", "boolean", false),
            ])),
        local_action("afmail.case.merge", "Merge Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("other_case_ref", "string", true),
                field("reason", "string", false),
            ])),
        read_action("afmail.archive.list", "Archive List"),
        read_action("afmail.archive.list.cases", "Archive Case List"),
        read_action("afmail.archive.list.messages", "Archive Message List"),
        local_action("afmail.archive.message.create", "Create Message Archive")
            .with_input(input_schema(&[
                field("name", "string", true),
                field("message_id", "string", false),
                field("summary", "string", false),
                field("reason", "string", false),
            ])),
        read_action("afmail.archive.message.show", "Show Message Archive")
            .with_input(input_schema(&[field("archive_ref", "string", true)])),
        local_action("afmail.archive.message.restore", "Restore Archived Message")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.archive.message.move", "Move Archived Message")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("new_archive_ref", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.archive.message.rename", "Rename Message Archive")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("name", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.archive.message.set_summary", "Set Archive Summary")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("summary", "string", true),
                field("reason", "string", false),
            ])),
        read_action("afmail.archive.message.notes.show", "Show Archive Notes")
            .with_input(input_schema(&[field("archive_ref", "string", true)])),
        local_action("afmail.archive.message.notes.append", "Append Archive Notes")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_action("afmail.archive.message.notes.replace", "Replace Archive Notes")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                markdown_field("text", true),
            ])),
        read_action("afmail.archive.case.show", "Show Archived Case")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_action("afmail.archive.case.restore", "Restore Archived Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("group", "string", true),
                field("reason", "string", false),
            ])),
        local_action("afmail.archive.case.rename", "Rename Archived Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("name", "string", true),
                field("reason", "string", false),
            ])),
        read_action("afmail.archive.case.notes.show", "Show Archived Case Notes")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_action("afmail.archive.case.notes.append", "Append Archived Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_action("afmail.archive.case.notes.replace", "Replace Archived Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_action("afmail.render.refresh", "Render Refresh"),
        local_action("afmail.render.templates", "Export Templates"),
        read_action("afmail.log.list", "Log List")
            .with_input(input_schema(&[field("limit", "number", false)])),
        read_action("afmail.log.tail", "Log Tail"),
        read_action("afmail.log.message", "Message Log")
            .with_input(input_schema(&[field("message_id", "string", true)])),
        read_action("afmail.log.case", "Case Log")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        read_action("afmail.log.archive", "Archive Log")
            .with_input(input_schema(&[field("archive_ref", "string", true)])),
        read_action("afmail.skill.status", "Skill Status"),
        local_action("afmail.skill.install", "Install Skill"),
        local_action("afmail.skill.uninstall", "Uninstall Skill"),
    ]
}

trait ActionExt {
    fn with_input(self, input_schema: Value) -> Self;
}

impl ActionExt for Action {
    fn with_input(mut self, input_schema: Value) -> Self {
        self.input_schema = Some(input_schema);
        self
    }
}

#[derive(Clone, Copy)]
struct InputField {
    name: &'static str,
    field_type: &'static str,
    required: bool,
    format: Option<&'static str>,
}

fn field(name: &'static str, field_type: &'static str, required: bool) -> InputField {
    InputField {
        name,
        field_type,
        required,
        format: None,
    }
}

/// A string input carrying markdown. The advisory `format` hint lets a host
/// collect it with a markdown editor; the emitted value is still a plain string.
fn markdown_field(name: &'static str, required: bool) -> InputField {
    InputField {
        name,
        field_type: "string",
        required,
        format: Some("markdown"),
    }
}

fn input_schema(fields: &[InputField]) -> Value {
    let required = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| field.name)
        .collect::<Vec<_>>();
    let mut properties = Map::new();
    for field in fields {
        let mut property = Map::new();
        property.insert("type".to_string(), json!(field.field_type));
        if let Some(format) = field.format {
            property.insert("format".to_string(), json!(format));
        }
        properties.insert(field.name.to_string(), Value::Object(property));
    }
    json!({
        "type": "object",
        "required": required,
        "properties": properties
    })
}

fn read_action(id: &str, label: &str) -> Action {
    Action::new(id, label, Risk::ReadOnly)
}

fn local_action(id: &str, label: &str) -> Action {
    Action::new(id, label, Risk::LocalMutation)
}

fn external_action(id: &str, label: &str, description: &str) -> Action {
    let mut action = Action::new(id, label, Risk::ExternalEffect);
    action.description = Some(description.to_string());
    action
}

fn destructive_action(id: &str, label: &str, description: &str) -> Action {
    let mut action = Action::new(id, label, Risk::Destructive);
    action.description = Some(description.to_string());
    action
}
