use agent_first_ui::{
    validate_space, validate_space_bindings, validate_space_profile, AgentFirstUiMessage,
    Capability, Layout, LayoutBuilder, LayoutRelation, LayoutRole, Prominence, Risk, Salience,
    SalienceState, Space, Unit, ValidationProfile,
};
use serde_json::{json, Map, Value};

use crate::error::{AppError, Result};
use crate::store::Workspace;

pub fn workspace_snapshot(workspace: &Workspace) -> Result<AgentFirstUiMessage> {
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
    let afmail_facts = json!({
        "title": "Agent-First Mail",
        "description": "Local mail workspace: triage, cases, push queue, and archives as data units with safe capabilities.",
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
        "logs": logs
    });
    let units = vec![
        mailbox_status(),
        inbox_triage(),
        active_cases(),
        push_queue(),
        message_archives(),
        case_archives(),
        activity_log(),
        mail_terminal(),
    ];
    let capabilities = capabilities();
    let layouts = layouts();
    let mut afmail_section = match afmail_facts {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    afmail_section.insert(
        "capabilities".to_string(),
        serde_json::to_value(capabilities)
            .map_err(|e| AppError::json("serialize AFUI capabilities", &e))?,
    );
    afmail_section.insert(
        "units".to_string(),
        serde_json::to_value(units).map_err(|e| AppError::json("serialize AFUI units", &e))?,
    );
    afmail_section.insert(
        "layouts".to_string(),
        serde_json::to_value(layouts).map_err(|e| AppError::json("serialize AFUI layouts", &e))?,
    );

    afmail_section.insert("root_layout".to_string(), json!("root"));

    let space_value = json!({
        "afmail": Value::Object(afmail_section),
        "host_web": {
            "terminals": {
                "mail_terminal": {
                    "session_id": "mail_terminal",
                    "title": "Mail Terminal",
                    "status": "host-owned local terminal"
                }
            }
        },
        "session_operator": {
            "selection": {
                "message_id": null,
                "case_ref": null,
                "archive_ref": null,
                "push_id": null
            }
        }
    });

    let space = Space::try_from_value(space_value).map_err(|e| {
        AppError::new(
            "afui_invalid",
            format!("generated AFUI space is invalid: {e}"),
        )
    })?;
    validate_space(&space).map_err(|e| {
        AppError::new(
            "afui_invalid",
            format!("generated AFUI space did not validate: {e}"),
        )
    })?;
    validate_space_bindings(&space).map_err(|e| {
        AppError::new(
            "afui_invalid",
            format!("AFUI space references facts that are not present: {e}"),
        )
    })?;
    let profile_report = validate_space_profile(&space, ValidationProfile::ReferenceHost);
    if !profile_report.valid() {
        return Err(AppError::new(
            "afui_invalid",
            format!(
                "AFUI space does not match the ReferenceHost View Hint Profile: {:?}",
                profile_report.issues
            ),
        ));
    }
    Ok(AgentFirstUiMessage::snapshot(space, 0))
}

/// Serialize a typed view-model projection into the AFUI space's item array.
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

fn mailbox_status() -> Unit {
    with_salience_state(
        Unit::new("mailbox_status", "facts"),
        Some(Prominence::Subtle),
        SalienceState::Info,
    )
        .set_field("title", json!("Mailbox Status"))
        .set_field(
            "description",
            json!("Compact workspace counts at a glance."),
        )
        .set_field(
            "items",
            json!([
                {"label": "Inbox", "value_binding": "space:afmail.status.triage_count"},
                {"label": "Active cases", "value_binding": "space:afmail.status.case_count"},
                {"label": "Push queue", "value_binding": "space:afmail.status.push_count"},
                {"label": "Message archives", "value_binding": "space:afmail.status.archive_message_category_count"},
                {"label": "Case archives", "value_binding": "space:afmail.status.archived_case_count"},
                {"label": "Activity", "value_binding": "space:afmail.logs.count"}
            ]),
        )
        .set_field("capabilities", json!(["status", "pull"]))
        .set_field("primary_capability", json!("pull"))
}

fn inbox_triage() -> Unit {
    with_salience_state(
        Unit::new("inbox_triage", "list"),
        Some(Prominence::Primary),
        SalienceState::Warning,
    )
        .set_field("title", json!("Inbox Triage"))
        .set_field(
            "summary",
            json!({
                "count_binding": "space:afmail.status.triage_count",
                "attention_binding": "space:afmail.status.triage_count"
            }),
        )
        .set_field(
            "description",
            json!("Unprocessed messages ready to read, case, archive, or discard."),
        )
        .set_field("source_binding", json!("space:afmail.ui.triage_items"))
        .set_field("count_binding", json!("space:afmail.status.triage_count"))
        .set_field(
            "input_bindings",
            json!({ "message_id": "record:message.message_id" }),
        )
        .set_field("id_key", json!("message.message_id"))
        .set_field("reference_binding", json!("record:message.message_id"))
        .set_field("reference_label", json!("Copy message id"))
        .set_field("title_key", json!("view.title"))
        .set_field("subtitle_key", json!("view.from"))
        .set_field("meta_key", json!("view.received_rfc3339"))
        .set_field("preview_key", json!("view.body_text"))
        .set_field("preview_length", json!(180))
        .set_field("body_key", json!("view.body_text"))
        .set_field("empty_text", json!("Inbox zero — nothing to triage."))
        .set_field(
            "fields",
            json!([
                {"key": "view.from", "label": "From"},
                {"key": "view.to", "label": "To"},
                {"key": "view.received_rfc3339", "label": "Received", "show_as": "relative_time", "salience": {"state": "info"}},
                {"key": "view.attachment_count", "label": "Attachments"}
            ]),
        )
        .set_field("activate_capability", json!("message_show"))
        .set_field("primary_capability", json!("case_create"))
        .set_field(
            "interactions",
            json!({
                "select": [
                    {"type": "set_actor_state", "path": "selection.message_id", "value_binding": "record:message.message_id"}
                ],
                "activate": [
                    {"type": "request", "capability": "message_show", "input_bindings": {"message_id": "record:message.message_id"}}
                ]
            }),
        )
        .set_field(
            "row_capabilities",
            json!([
                "message_show",
                "case_create",
                "archive_message_add",
                "message_trash"
            ]),
        )
}

fn active_cases() -> Unit {
    with_salience_state(
        Unit::new("active_cases", "list"),
        Some(Prominence::Primary),
        SalienceState::Info,
    )
        .set_field("title", json!("Active Cases"))
        .set_field(
            "summary",
            json!({ "count_binding": "space:afmail.status.case_count" }),
        )
        .set_field(
            "description",
            json!("Live work items with markdown notes and message counts."),
        )
        .set_field("source_binding", json!("space:afmail.ui.case_items"))
        .set_field("count_binding", json!("space:afmail.status.case_count"))
        .set_field(
            "input_bindings",
            json!({ "case_ref": "record:case.collection_uid" }),
        )
        .set_field("id_key", json!("case.collection_uid"))
        .set_field("reference_binding", json!("record:case.collection_uid"))
        .set_field("reference_label", json!("Copy case id"))
        .set_field("title_key", json!("case.collection_name"))
        .set_field("subtitle_key", json!("case.status"))
        .set_field("meta_key", json!("view.updated_rfc3339"))
        .set_field("badge_key", json!("case.message_count"))
        .set_field("preview_key", json!("view.notes_text"))
        .set_field("preview_length", json!(200))
        .set_field("body_key", json!("view.notes_text"))
        .set_field("body_format", json!("markdown"))
        .set_field("empty_text", json!("No active cases."))
        .set_field(
            "labels",
            count_labels("case.message_count", "message", "messages"),
        )
        .set_field(
            "fields",
            json!([
                {"key": "view.group", "label": "Group"},
                {"key": "case.status", "label": "Status"},
                {"key": "case.message_count", "label": "Messages"},
                {"key": "case.tags", "label": "Tags"},
                {"key": "view.last_message_rfc3339", "label": "Last message", "show_as": "relative_time", "salience": {"state": "info"}},
                {"key": "view.updated_rfc3339", "label": "Updated", "show_as": "relative_time", "salience": {"state": "neutral"}}
            ]),
        )
        .set_field("activate_capability", json!("case_show"))
        .set_field("primary_capability", json!("case_show"))
        .set_field(
            "interactions",
            json!({
                "select": [
                    {"type": "set_actor_state", "path": "selection.case_ref", "value_binding": "record:case.collection_uid"}
                ],
                "activate": [
                    {"type": "request", "capability": "case_show", "input_bindings": {"case_ref": "record:case.collection_uid"}}
                ]
            }),
        )
        .set_field(
            "row_capabilities",
            json!([
                "case_show",
                "case_notes_append",
                "case_archive"
            ]),
        )
}

fn case_archives() -> Unit {
    with_salience_state(
        Unit::new("case_archives", "list"),
        Some(Prominence::Subtle),
        SalienceState::Neutral,
    )
        .set_field("title", json!("Case Archives"))
        .set_field(
            "summary",
            json!({ "count_binding": "space:afmail.status.archived_case_count" }),
        )
        .set_field(
            "description",
            json!("Closed cases kept for lookup and restore."),
        )
        .set_field(
            "source_binding",
            json!("space:afmail.ui.archive_case_items"),
        )
        .set_field(
            "count_binding",
            json!("space:afmail.status.archived_case_count"),
        )
        .set_field(
            "input_bindings",
            json!({ "case_ref": "record:case.collection_uid" }),
        )
        .set_field("id_key", json!("case.collection_uid"))
        .set_field("reference_binding", json!("record:case.collection_uid"))
        .set_field("reference_label", json!("Copy case id"))
        .set_field("title_key", json!("case.collection_name"))
        .set_field("subtitle_key", json!("case.status"))
        .set_field("meta_key", json!("view.updated_rfc3339"))
        .set_field("badge_key", json!("case.message_count"))
        .set_field("preview_key", json!("view.notes_text"))
        .set_field("preview_length", json!(160))
        .set_field("body_key", json!("view.notes_text"))
        .set_field("body_format", json!("markdown"))
        .set_field("empty_text", json!("No archived cases."))
        .set_field(
            "labels",
            count_labels("case.message_count", "message", "messages"),
        )
        .set_field(
            "fields",
            json!([
                {"key": "case.collection_uid", "label": "Case"},
                {"key": "case.message_count", "label": "Messages"},
                {"key": "view.updated_rfc3339", "label": "Updated", "show_as": "relative_time", "salience": {"state": "neutral"}}
            ]),
        )
        .set_field("activate_capability", json!("archive_case_show"))
        .set_field("primary_capability", json!("archive_case_show"))
        .set_field(
            "interactions",
            json!({
                "select": [
                    {"type": "set_actor_state", "path": "selection.case_ref", "value_binding": "record:case.collection_uid"}
                ],
                "activate": [
                    {"type": "request", "capability": "archive_case_show", "input_bindings": {"case_ref": "record:case.collection_uid"}}
                ]
            }),
        )
        .set_field(
            "row_capabilities",
            json!(["archive_case_show", "archive_case_restore"]),
        )
}

fn message_archives() -> Unit {
    with_salience_state(
        Unit::new("message_archives", "list"),
        Some(Prominence::Subtle),
        SalienceState::Neutral,
    )
        .set_field("title", json!("Message Archives"))
        .set_field(
            "summary",
            json!({ "count_binding": "space:afmail.status.archive_message_category_count" }),
        )
        .set_field(
            "description",
            json!("Archived message buckets and their notes."),
        )
        .set_field(
            "source_binding",
            json!("space:afmail.ui.archive_message_items"),
        )
        .set_field(
            "count_binding",
            json!("space:afmail.status.archive_message_category_count"),
        )
        .set_field(
            "input_bindings",
            json!({ "archive_ref": "record:archive.collection_uid" }),
        )
        .set_field("id_key", json!("archive.collection_uid"))
        .set_field("reference_binding", json!("record:archive.collection_uid"))
        .set_field("reference_label", json!("Copy archive id"))
        .set_field("title_key", json!("archive.collection_name"))
        .set_field("subtitle_key", json!("archive.collection_uid"))
        .set_field("meta_key", json!("view.updated_rfc3339"))
        .set_field("badge_key", json!("archive.message_count"))
        .set_field("preview_key", json!("view.notes_text"))
        .set_field("preview_length", json!(160))
        .set_field("body_key", json!("view.notes_text"))
        .set_field("body_format", json!("markdown"))
        .set_field("empty_text", json!("No message archives."))
        .set_field(
            "labels",
            count_labels("archive.message_count", "message", "messages"),
        )
        .set_field(
            "fields",
            json!([
                {"key": "archive.collection_uid", "label": "Archive"},
                {"key": "archive.message_count", "label": "Messages"},
                {"key": "view.updated_rfc3339", "label": "Updated", "show_as": "relative_time", "salience": {"state": "neutral"}}
            ]),
        )
        .set_field("activate_capability", json!("archive_message_show"))
        .set_field("primary_capability", json!("archive_message_show"))
        .set_field(
            "interactions",
            json!({
                "select": [
                    {"type": "set_actor_state", "path": "selection.archive_ref", "value_binding": "record:archive.collection_uid"}
                ],
                "activate": [
                    {"type": "request", "capability": "archive_message_show", "input_bindings": {"archive_ref": "record:archive.collection_uid"}}
                ]
            }),
        )
        .set_field(
            "row_capabilities",
            json!([
                "archive_message_show",
                "archive_message_restore",
                "archive_message_rename"
            ]),
        )
}

fn push_queue() -> Unit {
    with_salience_state(
        Unit::new("push_queue", "list"),
        Some(Prominence::Normal),
        SalienceState::Warning,
    )
        .set_field("title", json!("Push Queue"))
        .set_field(
            "summary",
            json!({
                "count_binding": "space:afmail.status.push_count",
                "attention_binding": "space:afmail.status.push_count"
            }),
        )
        .set_field(
            "description",
            json!("Queued remote mailbox effects waiting for dry-run or confirmation."),
        )
        .set_field("source_binding", json!("space:afmail.ui.push_items"))
        .set_field("count_binding", json!("space:afmail.status.push_count"))
        .set_field(
            "input_bindings",
            json!({ "push_id": "record:push.push_id" }),
        )
        .set_field("id_key", json!("push.push_id"))
        .set_field("reference_binding", json!("record:push.push_id"))
        .set_field("reference_label", json!("Copy push id"))
        .set_field("title_key", json!("view.display_kind"))
        .set_field("subtitle_key", json!("push.push_id"))
        .set_field("meta_key", json!("push.updated_rfc3339"))
        .set_field("badge_key", json!("view.step_count"))
        .set_field("empty_text", json!("Nothing queued to send."))
        .set_field(
            "labels",
            count_labels("view.step_count", "step", "steps"),
        )
        .set_field(
            "fields",
            json!([
                {"key": "push.push_id", "label": "Item"},
                {"key": "view.display_kind", "label": "Type"},
                {"key": "push.attempt_count", "label": "Attempts"},
                {"key": "view.step_count", "label": "Steps"},
                {"key": "push.created_rfc3339", "label": "Created", "show_as": "relative_time", "salience": {"state": "neutral"}},
                {"key": "push.updated_rfc3339", "label": "Updated", "show_as": "relative_time", "salience": {"state": "warning"}}
            ]),
        )
        .set_field("activate_capability", json!("push_dry_run"))
        .set_field("primary_capability", json!("push_dry_run"))
        .set_field(
            "interactions",
            json!({
                "select": [
                    {"type": "set_actor_state", "path": "selection.push_id", "value_binding": "record:push.push_id"}
                ],
                "activate": [
                    {"type": "request", "capability": "push_dry_run", "input_bindings": {"push_id": "record:push.push_id"}}
                ]
            }),
        )
        .set_field(
            "row_capabilities",
            json!([
                "push_list",
                "push_dry_run",
                "push_confirm"
            ]),
        )
}

fn activity_log() -> Unit {
    with_salience_state(
        Unit::new("activity_log", "log"),
        Some(Prominence::Subtle),
        SalienceState::Info,
    )
    .set_field("title", json!("Activity"))
    .set_field(
        "description",
        json!("Recent audit events from the local AFMail workspace."),
    )
    .set_field("source_binding", json!("space:afmail.ui.log_items"))
    .set_field("level_key", json!("kind"))
    .set_field("message_key", json!("message"))
    .set_field("time_key", json!("created_rfc3339"))
    .set_field("empty_text", json!("No activity yet."))
}

/// Build an advisory `labels` hint mapping a numeric field to a count template.
/// The host fills `{n}`; the producer supplies the wording. This is afmail's
/// machine-facing UI view-hint output, which stays English (only the human-facing
/// Markdown views are localized, via templates).
fn count_labels(field: &str, one: &str, other: &str) -> Value {
    let template = json!({ "one": format!("{{n}} {one}"), "other": format!("{{n}} {other}") });
    let mut map = Map::new();
    map.insert(field.to_string(), template);
    Value::Object(map)
}

fn with_salience_state(
    mut unit: Unit,
    prominence: Option<Prominence>,
    state: SalienceState,
) -> Unit {
    let mut salience = Salience::new().state(state);
    if let Some(prominence) = prominence {
        salience = salience.prominence(prominence);
    }
    unit.salience = Some(salience);
    unit
}

/// The producer's recommended, implementation-neutral layout pool. Regions name
/// units or other regions by id; overlays replace the old "closed by default"
/// hint without naming a widget or viewport.
fn layouts() -> Vec<Layout> {
    vec![
        LayoutBuilder::new("primary", ["inbox_triage", "active_cases"])
            .description("Primary triage and case work.")
            .relation(LayoutRelation::Workspace)
            .role(LayoutRole::Primary)
            .primary()
            .build(),
        LayoutBuilder::new("glance", ["mailbox_status", "push_queue"])
            .description("Counts and queued effects.")
            .relation(LayoutRelation::OverviewDetail)
            .role(LayoutRole::Summary)
            .subtle()
            .build(),
        LayoutBuilder::new("archives", ["message_archives", "case_archives"])
            .description("Lookup and restore surfaces.")
            .relation(LayoutRelation::Supporting)
            .role(LayoutRole::History)
            .overlay(true)
            .build(),
        LayoutBuilder::new("tools", ["mail_terminal"])
            .description("Host-owned local tools.")
            .relation(LayoutRelation::Supporting)
            .role(LayoutRole::Input)
            .overlay(true)
            .build(),
        LayoutBuilder::new("root", ["primary", "glance", "archives", "tools"])
            .relation(LayoutRelation::Workspace)
            .build(),
    ]
}

fn mail_terminal() -> Unit {
    Unit::new("mail_terminal", "terminal")
        .set_field(
            "session_id_binding",
            json!("space:host_web.terminals.mail_terminal.session_id"),
        )
        .set_field(
            "title_binding",
            json!("space:host_web.terminals.mail_terminal.title"),
        )
        .set_field(
            "status_binding",
            json!("space:host_web.terminals.mail_terminal.status"),
        )
        .set_field("title", json!("Mail Terminal"))
        .set_field("salience", json!({"state": "info"}))
}

fn capabilities() -> Vec<Capability> {
    vec![
        read_capability("status", "Status"),
        external_capability(
            "pull",
            "Pull",
            "Reads configured IMAP mailboxes into local files. It reaches the remote mailbox but does not apply remote writes.",
        )
        .with_input(input_schema(&[field("ids", "array", false)])),
        read_capability("ui_snapshot", "UI Snapshot"),
        read_capability("config_show", "Config Show"),
        read_capability("config_get", "Config Get")
            .with_input(input_schema(&[field("key", "string", true)])),
        local_capability("config_set", "Config Set")
            .with_input(input_schema(&[
                field("key", "string", true),
                field("values", "array", true),
            ])),
        external_capability("remote_test", "Remote Test", "Tests configured IMAP login."),
        external_capability("remote_folders", "Remote Folders", "Lists remote IMAP folders."),
        read_capability("push_list", "Push List"),
        read_capability("push_dry_run", "Push Dry Run")
            .with_input(input_schema(&[field("push_id", "string", false)])),
        destructive_capability(
            "push_confirm",
            "Push Confirm",
            "Applies all queued mailbox effects and may send mail.",
        )
        .with_input(input_schema(&[field("push_id", "string", false)])),
        read_capability("push_drafts_dry_run", "Draft Save Dry Run"),
        external_capability(
            "push_drafts_confirm",
            "Save Drafts",
            "Saves queued drafts to the configured remote Drafts mailbox.",
        ),
        read_capability("push_drafts_send_dry_run", "Send Drafts Dry Run"),
        destructive_capability(
            "push_drafts_send_confirm",
            "Send Drafts",
            "Sends queued outbound mail and records related mailbox effects.",
        ),
        read_capability("push_archive_dry_run", "Archive Push Dry Run"),
        external_capability(
            "push_archive_confirm",
            "Push Archive",
            "Applies queued archive mailbox moves.",
        ),
        read_capability("push_spam_dry_run", "Spam Push Dry Run"),
        external_capability("push_spam_confirm", "Push Spam", "Applies queued Junk moves."),
        read_capability("push_trash_dry_run", "Trash Push Dry Run"),
        external_capability(
            "push_trash_confirm",
            "Push Trash",
            "Applies queued Trash moves.",
        ),
        read_capability("doctor", "Doctor"),
        local_capability("doctor_repair", "Doctor Repair"),
        destructive_capability(
            "purge",
            "Purge Discards",
            "Permanently deletes old local discard records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        destructive_capability(
            "purge_spam",
            "Purge Spam",
            "Permanently deletes old local spam records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        destructive_capability(
            "purge_trash",
            "Purge Trash",
            "Permanently deletes old local trash records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        destructive_capability(
            "purge_deleted",
            "Purge Deleted",
            "Permanently deletes old local remote-deleted records.",
        )
        .with_input(input_schema(&[field("older_than_days", "number", false)])),
        read_capability("triage_list", "Triage List"),
        read_capability("message_show", "Show Message")
            .with_input(input_schema(&[field("message_id", "string", true)])),
        local_capability("archive_message_add", "Add To Archive")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("summary", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("message_spam", "Spam")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("message_trash", "Trash")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("message_restore", "Restore")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("message_attachment_fetch", "Fetch Attachment")
            .with_input(input_schema(&[
                field("message_id", "string", true),
                field("part_id", "string", false),
            ])),
        local_capability("case_create", "Create Case")
            .with_input(input_schema(&[
                field("name", "string", true),
                field("group", "string", false),
                field("message_id", "string", false),
                field("reason", "string", false),
            ])),
        read_capability("case_list", "Case List"),
        read_capability("case_show", "Show Case")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_capability("case_add", "Add Message")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("case_move", "Move Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("group", "string", true),
            ])),
        local_capability("case_rename", "Rename Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("name", "string", true),
                field("reason", "string", false),
            ])),
        read_capability("case_notes_show", "Show Case Notes")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_capability("case_notes_append", "Append Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_capability("case_notes_replace", "Replace Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_capability("case_archive", "Archive Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("case_reopen", "Reopen Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("case_tag", "Tag Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("tag", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("case_untag", "Untag Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("tag", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("case_draft_new", "New Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("to", "array", true),
                field("cc", "array", false),
                field("subject", "string", false),
            ])),
        local_capability("case_draft_validate", "Validate Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
            ])),
        local_capability("case_draft_change", "Change Draft")
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
        local_capability("case_draft_save", "Queue Draft Save")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
            ])),
        local_capability("case_draft_send", "Queue Draft Send")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
            ])),
        local_capability("case_draft_attach", "Attach Draft File")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
                field("path", "string", true),
            ])),
        local_capability("case_draft_remove", "Remove Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("draft_name", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("case_draft_reply", "Reply Draft")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("message_id", "string", true),
                field("body", "string", false),
                field("body_file", "string", false),
                field("all", "boolean", false),
            ])),
        local_capability("case_merge", "Merge Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("other_case_ref", "string", true),
                field("reason", "string", false),
            ])),
        read_capability("archive_list", "Archive List"),
        read_capability("archive_list_cases", "Archive Case List"),
        read_capability("archive_list_messages", "Archive Message List"),
        local_capability("archive_message_create", "Create Message Archive")
            .with_input(input_schema(&[
                field("name", "string", true),
                field("message_id", "string", false),
                field("summary", "string", false),
                field("reason", "string", false),
            ])),
        read_capability("archive_message_show", "Show Message Archive")
            .with_input(input_schema(&[field("archive_ref", "string", true)])),
        local_capability("archive_message_restore", "Restore Archived Message")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("archive_message_move", "Move Archived Message")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("new_archive_ref", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("archive_message_rename", "Rename Message Archive")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("name", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("archive_message_set_summary", "Set Archive Summary")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                field("message_id", "string", true),
                field("summary", "string", true),
                field("reason", "string", false),
            ])),
        read_capability("archive_message_notes_show", "Show Archive Notes")
            .with_input(input_schema(&[field("archive_ref", "string", true)])),
        local_capability("archive_message_notes_append", "Append Archive Notes")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_capability("archive_message_notes_replace", "Replace Archive Notes")
            .with_input(input_schema(&[
                field("archive_ref", "string", true),
                markdown_field("text", true),
            ])),
        read_capability("archive_case_show", "Show Archived Case")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_capability("archive_case_restore", "Restore Archived Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("group", "string", true),
                field("reason", "string", false),
            ])),
        local_capability("archive_case_rename", "Rename Archived Case")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                field("name", "string", true),
                field("reason", "string", false),
            ])),
        read_capability("archive_case_notes_show", "Show Archived Case Notes")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        local_capability("archive_case_notes_append", "Append Archived Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_capability("archive_case_notes_replace", "Replace Archived Case Notes")
            .with_input(input_schema(&[
                field("case_ref", "string", true),
                markdown_field("text", true),
            ])),
        local_capability("render_refresh", "Render Refresh"),
        local_capability("render_templates", "Export Templates"),
        read_capability("log_list", "Log List")
            .with_input(input_schema(&[field("limit", "number", false)])),
        read_capability("log_tail", "Log Tail"),
        read_capability("log_message", "Message Log")
            .with_input(input_schema(&[field("message_id", "string", true)])),
        read_capability("log_case", "Case Log")
            .with_input(input_schema(&[field("case_ref", "string", true)])),
        read_capability("log_archive", "Archive Log")
            .with_input(input_schema(&[field("archive_ref", "string", true)])),
        read_capability("skill_status", "Skill Status"),
        local_capability("skill_install", "Install Skill"),
        local_capability("skill_uninstall", "Uninstall Skill"),
    ]
}

trait CapabilityExt {
    fn with_input(self, input_schema: Value) -> Self;
}

impl CapabilityExt for Capability {
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

fn read_capability(id: &str, label: &str) -> Capability {
    Capability::new(id, label, Risk::ReadOnly)
}

fn local_capability(id: &str, label: &str) -> Capability {
    Capability::new(id, label, Risk::LocalMutation)
}

fn external_capability(id: &str, label: &str, description: &str) -> Capability {
    let mut capability = Capability::new(id, label, Risk::ExternalEffect);
    capability.description = Some(description.to_string());
    capability
}

fn destructive_capability(id: &str, label: &str, description: &str) -> Capability {
    let mut capability = Capability::new(id, label, Risk::Destructive);
    capability.description = Some(description.to_string());
    capability
}
