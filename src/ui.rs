use agent_first_ui::{
    AgentFirstUiMessage, Capability, Layout, LayoutBuilder, LayoutRelation, LayoutRole, Prominence,
    Risk, Salience, SalienceState, Space, Unit, ValidationProfile, validate_space,
    validate_space_bindings, validate_space_profile,
};
use serde_json::{Map, Value, json};

use crate::error::{AppError, Result};
use crate::store::Workspace;

pub fn workspace_snapshot(workspace: &Workspace) -> Result<AgentFirstUiMessage> {
    let status = workspace.status()?;
    let message_list = workspace.message_list(None, None, None, "all")?;
    let cases = workspace.case_list(false, true)?;
    let message_items = workspace.mail_message_views(&message_list)?;
    let draft_items = workspace.case_draft_views(&cases)?;
    let draft_usable_count = draft_items
        .iter()
        .filter(|draft| draft.availability.usable)
        .count();
    let draft_editable_count = draft_items
        .iter()
        .filter(|draft| draft.availability.editable)
        .count();

    let afmail_facts = json!({
        "title": "Agent-First Mail",
        "description": "Read local mail, edit local case drafts, and delegate either record to an agent by reference.",
        "workspace": {
            "path": workspace.root().to_string_lossy().replace('\\', "/"),
            "version": env!("CARGO_PKG_VERSION"),
        },
        "mail": {
            "count": message_items.len(),
            "items": message_items,
        },
        "drafts": {
            "count": draft_items.len(),
            "editable_count": draft_editable_count,
            "usable_count": draft_usable_count,
            "items": draft_items,
        },
        "glance": {
            "message_count": status.get("message_count").and_then(Value::as_u64).unwrap_or(0),
            "inbox_count": status.get("triage_count").and_then(Value::as_u64).unwrap_or(0),
            "draft_count": draft_items.len(),
            "editable_draft_count": draft_editable_count,
            "usable_draft_count": draft_usable_count,
        },
    });
    let units = units();
    let capabilities = capabilities();
    let layouts = layouts();
    let mut afmail_section = match afmail_facts {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    afmail_section.insert(
        "capabilities".to_string(),
        serde_json::to_value(capabilities)
            .map_err(|error| AppError::json("serialize AFUI capabilities", &error))?,
    );
    afmail_section.insert(
        "units".to_string(),
        serde_json::to_value(units)
            .map_err(|error| AppError::json("serialize AFUI units", &error))?,
    );
    afmail_section.insert(
        "layouts".to_string(),
        serde_json::to_value(layouts)
            .map_err(|error| AppError::json("serialize AFUI layouts", &error))?,
    );
    afmail_section.insert("root_layout".to_string(), json!("root"));
    afmail_section.insert("surface".to_string(), json!({ "genre": "triage" }));

    let selected_message = afmail_section
        .get("mail")
        .and_then(|mail| mail.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(empty_message_selection);
    let selected_draft = afmail_section
        .get("drafts")
        .and_then(|drafts| drafts.get("items"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(empty_draft_selection);
    let space_value = json!({
        "afmail": Value::Object(afmail_section),
        "session_operator": {
            "selection": {
                "message": selected_message,
                "draft": selected_draft,
            }
        }
    });

    let space = Space::try_from_value(space_value).map_err(|error| {
        AppError::new(
            "afui_invalid",
            format!("generated AFUI space is invalid: {error}"),
        )
    })?;
    validate_space(&space).map_err(|error| {
        AppError::new(
            "afui_invalid",
            format!("generated AFUI space did not validate: {error}"),
        )
    })?;
    validate_space_bindings(&space).map_err(|error| {
        AppError::new(
            "afui_invalid",
            format!("AFUI space references facts that are not present: {error}"),
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

fn units() -> Vec<Unit> {
    vec![mail_queue(), draft_list(), draft_editor(), mailbox_glance()]
}

fn mail_queue() -> Unit {
    with_salience_state(
        Unit::new("mail_queue", "list"),
        Some(Prominence::Primary),
        SalienceState::Neutral,
    )
    .set_field("title", json!("Mail"))
    .set_field(
        "description",
        json!("All locally cached mail, newest first. Select a message to read it."),
    )
    .set_field("source_binding", json!("space:afmail.mail.items"))
    .set_field("count_binding", json!("space:afmail.mail.count"))
    .set_field("id_key", json!("message.message_id"))
    .set_field("reference_binding", json!("record:message.message_id"))
    .set_field("reference_kind", json!("message"))
    .set_field("title_key", json!("view.title"))
    .set_field("subtitle_key", json!("view.from"))
    .set_field("meta_key", json!("view.received_rfc3339"))
    .set_field("badge_key", json!("view.workspace_status"))
    .set_field("preview_key", json!("view.body_text"))
    .set_field("preview_length", json!(180))
    .set_field("body_key", json!("view.body_text"))
    .set_field("no_body_text", json!("This message has no text body."))
    .set_field("empty_text", json!("No local mail."))
    .set_field(
        "fields",
        json!([
            {"key": "view.to", "label": "To"},
            {"key": "view.cc", "label": "Cc", "visible_when": {"empty": false}},
            {"key": "view.received_rfc3339", "label": "Received", "show_as": "relative_time"},
            {"key": "view.attachment_count", "label": "Attachments", "visible_when": {"gt": 0}}
        ]),
    )
    .set_field(
        "interactions",
        json!({
            "select": [
                {"type": "set_actor_state", "path": "selection.message", "value_binding": "record:."}
            ],
            "activate": [
                {"type": "set_actor_state", "path": "selection.message", "value_binding": "record:."}
            ]
        }),
    )
}

fn draft_list() -> Unit {
    with_salience_state(
        Unit::new("draft_list", "list"),
        Some(Prominence::Normal),
        SalienceState::Info,
    )
    .set_field("title", json!("Drafts"))
    .set_field(
        "description",
        json!("Every local case draft, including retained drafts in archived cases."),
    )
    .set_field("source_binding", json!("space:afmail.drafts.items"))
    .set_field("count_binding", json!("space:afmail.drafts.count"))
    .set_field("id_key", json!("reference"))
    .set_field("reference_binding", json!("record:reference"))
    .set_field("reference_kind", json!("draft"))
    .set_field("title_key", json!("view.title"))
    .set_field("subtitle_key", json!("view.case_label"))
    .set_field("meta_key", json!("validation.state"))
    .set_field("preview_key", json!("body"))
    .set_field("preview_length", json!(160))
    .set_field("body_key", json!("body"))
    .set_field("no_body_text", json!("This draft has no body."))
    .set_field("empty_text", json!("No local case drafts."))
    .set_field(
        "fields",
        json!([
            {"key": "view.to_text", "label": "To"},
            {"key": "status", "label": "Draft state"},
            {"key": "validation.state", "label": "Validation"},
            {"key": "attachments", "label": "Attachments", "visible_when": {"empty": false}}
        ]),
    )
    .set_field(
        "interactions",
        json!({
            "select": [
                {"type": "set_actor_state", "path": "selection.draft", "value_binding": "record:."}
            ],
            "activate": [
                {"type": "set_actor_state", "path": "selection.draft", "value_binding": "record:."}
            ]
        }),
    )
}

fn draft_editor() -> Unit {
    with_salience_state(
        Unit::new("draft_editor", "form"),
        Some(Prominence::Primary),
        SalienceState::Neutral,
    )
    .set_field("title", json!("Draft Editor"))
    .set_field(
        "description",
        json!("Edit the selected local draft or delegate it to an agent by reference."),
    )
    .set_field("source_binding", json!("actor:selection.draft"))
    .set_field(
        "reference_binding",
        json!("actor:selection.draft.reference"),
    )
    .set_field("reference_kind", json!("draft"))
    .set_field(
        "fields",
        json!([
            {"name": "to", "label": "To", "type": "array", "format": "email_list", "value_binding": "actor:selection.draft.to", "required": true},
            {"name": "cc", "label": "Cc", "type": "array", "format": "email_list", "value_binding": "actor:selection.draft.cc"},
            {"name": "subject", "label": "Subject", "type": "string", "format": "text", "value_binding": "actor:selection.draft.subject", "required": true},
            {"name": "body", "label": "Body", "type": "string", "format": "markdown", "value_binding": "actor:selection.draft.body", "required": true}
        ]),
    )
    .set_field(
        "input_bindings",
        json!({
            "case_ref": "actor:selection.draft.case_ref",
            "draft_name": "actor:selection.draft.draft_name"
        }),
    )
    .set_field("submit_capability", json!("draft_change"))
    .set_field("primary_capability", json!("draft_change"))
}

fn mailbox_glance() -> Unit {
    with_salience_state(
        Unit::new("mailbox_glance", "facts"),
        Some(Prominence::Subtle),
        SalienceState::Info,
    )
    .set_field("title", json!("Mailbox Glance"))
    .set_field("source_binding", json!("space:afmail.glance"))
    .set_field(
        "fields",
        json!([
            {"key": "message_count", "label": "Messages"},
            {"key": "inbox_count", "label": "Inbox"},
            {"key": "draft_count", "label": "Drafts"},
            {"key": "editable_draft_count", "label": "Editable drafts"},
            {"key": "usable_draft_count", "label": "Usable drafts"}
        ]),
    )
}

fn layouts() -> Vec<Layout> {
    vec![
        LayoutBuilder::new("mail", ["mail_queue"])
            .description("Side-by-side message queue and selected-message reader.")
            .relation(LayoutRelation::MasterDetail)
            .role(LayoutRole::Primary)
            .primary()
            .build(),
        LayoutBuilder::new("drafts", ["draft_list", "draft_editor"])
            .description("Draft selection and editing workspace.")
            .relation(LayoutRelation::MasterDetail)
            .role(LayoutRole::Input)
            .build(),
        LayoutBuilder::new("glance", ["mailbox_glance"])
            .description("Optional mailbox and draft counts.")
            .relation(LayoutRelation::OverviewDetail)
            .role(LayoutRole::Summary)
            .subtle()
            .build(),
        LayoutBuilder::new("root", ["mail", "drafts", "glance"])
            .relation(LayoutRelation::Workspace)
            .build(),
    ]
}

fn capabilities() -> Vec<Capability> {
    let mut change = Capability::new("draft_change", "Save Draft", Risk::LocalMutation);
    change.description =
        Some("Writes the selected local case draft after owner-side validation.".to_string());
    change.applicable_binding = Some("actor:selection.draft.availability.editable".to_string());
    change.input_schema = Some(input_schema(&[
        field("case_ref", "string", true),
        field("draft_name", "string", true),
        email_list_field("to", true),
        email_list_field("cc", false),
        field("subject", "string", true),
        formatted_field("body", "string", "markdown", true),
    ]));

    vec![change]
}

#[derive(Clone, Copy)]
struct InputField {
    name: &'static str,
    field_type: &'static str,
    format: Option<&'static str>,
    item_format: Option<&'static str>,
    required: bool,
}

fn field(name: &'static str, field_type: &'static str, required: bool) -> InputField {
    InputField {
        name,
        field_type,
        format: None,
        item_format: None,
        required,
    }
}

fn formatted_field(
    name: &'static str,
    field_type: &'static str,
    format: &'static str,
    required: bool,
) -> InputField {
    InputField {
        name,
        field_type,
        format: Some(format),
        item_format: None,
        required,
    }
}

fn email_list_field(name: &'static str, required: bool) -> InputField {
    InputField {
        name,
        field_type: "array",
        format: Some("email_list"),
        item_format: Some("email"),
        required,
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
        if let Some(item_format) = field.item_format {
            property.insert(
                "items".to_string(),
                json!({"type": "string", "format": item_format}),
            );
        }
        properties.insert(field.name.to_string(), Value::Object(property));
    }
    json!({
        "type": "object",
        "required": required,
        "properties": properties
    })
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

fn empty_message_selection() -> Value {
    json!({
        "message": {"message_id": ""},
        "view": {
            "title": "No message selected",
            "from": "",
            "to": [],
            "cc": [],
            "received_rfc3339": "",
            "sent_rfc3339": "",
            "direction": "",
            "attachment_count": 0,
            "workspace_status": "",
            "unread": false,
            "body_text": ""
        }
    })
}

fn empty_draft_selection() -> Value {
    json!({
        "case_ref": "",
        "case_name": "",
        "group": "",
        "case_archived": false,
        "draft_name": "",
        "draft_path": "",
        "reference": "",
        "to": [],
        "cc": [],
        "subject": "",
        "body": "",
        "attachments": [],
        "status": "unavailable",
        "queued_action": null,
        "push_id": null,
        "validation": {
            "state": "unavailable",
            "current_hash": "",
            "recorded": false,
            "last_validated_hash": null,
            "last_validated_rfc3339": null,
            "error_code": null,
            "error": null
        },
        "availability": {
            "editable": false,
            "attachments_available": false,
            "usable": false
        },
        "view": {
            "title": "No draft selected",
            "case_label": "",
            "to_text": "",
            "cc_text": ""
        }
    })
}
