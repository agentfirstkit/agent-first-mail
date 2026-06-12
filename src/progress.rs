use crate::error::{AppError, Result};
use serde_json::{json, Map, Value};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(crate) type ProgressCallback<'a> = dyn FnMut(&str, Value) + 'a;

pub(crate) fn emit(progress: &mut Option<&mut ProgressCallback<'_>>, phase: &str, fields: Value) {
    let Some(callback) = progress.as_deref_mut() else {
        return;
    };
    callback(phase, fields);
}

pub(crate) fn object_with_phase(phase: &str, fields: Value) -> Value {
    let mut map = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    map.insert("phase".to_string(), Value::String(phase.to_string()));
    Value::Object(map)
}

pub(crate) struct WorkspaceProgressSink {
    path: PathBuf,
    command: &'static str,
    started: Instant,
    started_rfc3339: String,
    last_phase: String,
    last_fields: Value,
}

impl WorkspaceProgressSink {
    pub(crate) fn start(root: &Path, command: &'static str) -> Self {
        let mut sink = Self {
            path: workspace_progress_path(root),
            command,
            started: Instant::now(),
            started_rfc3339: crate::store::now_rfc3339(),
            last_phase: "start".to_string(),
            last_fields: json!({}),
        };
        sink.write("running", "start", json!({}), None, None);
        sink
    }

    pub(crate) fn update(&mut self, phase: &str, fields: Value) {
        self.last_phase = phase.to_string();
        self.last_fields = fields.clone();
        self.write("running", phase, fields, None, None);
    }

    pub(crate) fn finish_success(&mut self, result: &Value) {
        let summary = scalar_summary(result);
        self.write(
            "succeeded",
            "finish",
            json!({"success": true}),
            Some(summary),
            None,
        );
    }

    pub(crate) fn finish_failure(&mut self, err: &AppError) {
        self.write(
            "failed",
            "finish",
            json!({
                "success": false,
                "failed_phase": self.last_phase.as_str(),
                "failed_fields": self.last_fields.clone(),
            }),
            None,
            Some(json!({
                "error_code": err.error_code,
                "error": err.message.as_str(),
                "retryable": err.retryable,
            })),
        );
    }

    fn write(
        &mut self,
        status: &str,
        phase: &str,
        fields: Value,
        result: Option<Value>,
        error: Option<Value>,
    ) {
        let value = json!({
            "schema_name": "workspace_progress",
            "schema_version": 1,
            "command": self.command,
            "status": status,
            "phase": phase,
            "message": progress_message(self.command, phase),
            "started_rfc3339": self.started_rfc3339.as_str(),
            "updated_rfc3339": crate::store::now_rfc3339(),
            "elapsed_ms": self.started.elapsed().as_millis() as u64,
            "fields": fields,
            "result": result,
            "error": error,
        });
        let _ = crate::util::write_json_pretty(&self.path, &value);
    }
}

pub(crate) fn workspace_status_progress(root: &Path) -> Result<Value> {
    let path = workspace_progress_path(root);
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(json!({"status": "idle"}));
        }
        Err(err) => return Err(AppError::io("read workspace progress", &err)),
    };
    let value: Value =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse workspace progress", &e))?;
    let Value::Object(snapshot) = &value else {
        return Err(AppError::new(
            "progress_invalid",
            "workspace progress snapshot must be a JSON object",
        ));
    };
    Ok(status_progress_from_snapshot(snapshot))
}

fn status_progress_from_snapshot(snapshot: &Map<String, Value>) -> Value {
    let status = string_field(snapshot, "status").unwrap_or("unknown");
    let command = string_field(snapshot, "command");
    let phase = string_field(snapshot, "phase");
    let fields = snapshot.get("fields").cloned().unwrap_or_else(|| json!({}));
    let error = snapshot.get("error").cloned().unwrap_or(Value::Null);
    let result = snapshot.get("result").cloned().unwrap_or(Value::Null);

    let mut out = Map::new();
    out.insert("status".to_string(), json!(status));
    insert_string(&mut out, "command", command);
    insert_string(&mut out, "phase", phase);
    out.insert(
        "summary".to_string(),
        json!(progress_summary(
            status, command, phase, &fields, &result, &error
        )),
    );
    let computed_fields = if status == "failed" {
        failed_fields(&fields)
    } else {
        &fields
    };
    insert_progress_counts(&mut out, command, computed_fields);
    if status == "failed" {
        insert_failed_progress_fields(&mut out, &fields);
    }
    out.insert("fields".to_string(), fields);
    out.insert("error".to_string(), error);
    Value::Object(out)
}

fn insert_progress_counts(out: &mut Map<String, Value>, command: Option<&str>, fields: &Value) {
    let Value::Object(fields) = fields else {
        return;
    };
    if let Some(value) = fields.get("processed_count").cloned() {
        out.insert("processed_count".to_string(), value);
    }
    if let Some(value) = fields.get("uid_count").cloned() {
        out.insert("total_count".to_string(), value);
    }
    if let (Some(processed), Some(total)) = (
        value_u64(fields.get("processed_count")),
        value_u64(fields.get("uid_count")),
    ) {
        if total > 0 {
            let percent = ((processed as f64 / total as f64) * 10000.0).round() / 100.0;
            out.insert("progress_percent".to_string(), json!(percent));
        }
    }
    for key in ["batch_index", "batch_count"] {
        if let Some(value) = fields.get(key).cloned() {
            out.insert(key.to_string(), value);
        }
    }
    match command {
        Some("pull") => insert_pull_progress_fields(out, fields),
        Some("push") => insert_push_progress_fields(out, fields),
        _ => {}
    }
}

fn insert_pull_progress_fields(out: &mut Map<String, Value>, fields: &Map<String, Value>) {
    for key in ["mailbox_id", "mailbox_name", "mailbox_count"] {
        if let Some(value) = fields.get(key).cloned() {
            out.insert(key.to_string(), value);
        }
    }
    if let Some(value) = fields.get("index").cloned() {
        out.insert("mailbox_index".to_string(), value);
    }
}

fn insert_push_progress_fields(out: &mut Map<String, Value>, fields: &Map<String, Value>) {
    for key in [
        "push_id",
        "mode",
        "kind",
        "display_kind",
        "item_index",
        "item_count",
        "step_index",
        "step_count",
        "label",
    ] {
        if let Some(value) = fields.get(key).cloned() {
            out.insert(key.to_string(), value);
        }
    }
    if !out.contains_key("item_index") {
        if let Some(value) = fields.get("index").cloned() {
            out.insert("item_index".to_string(), value);
        }
    }
}

fn insert_failed_progress_fields(out: &mut Map<String, Value>, fields: &Value) {
    let Value::Object(fields) = fields else {
        return;
    };
    if let Some(value) = fields.get("failed_phase").cloned() {
        out.insert("failed_phase".to_string(), value);
    }
    if let Some(value) = fields.get("failed_fields").cloned() {
        out.insert("failed_fields".to_string(), value);
    }
}

fn failed_fields(fields: &Value) -> &Value {
    fields
        .as_object()
        .and_then(|map| map.get("failed_fields"))
        .unwrap_or(fields)
}

fn progress_summary(
    status: &str,
    command: Option<&str>,
    phase: Option<&str>,
    fields: &Value,
    result: &Value,
    error: &Value,
) -> String {
    let command = command.unwrap_or("afmail");
    if status == "failed" {
        let (failed_phase, failed_fields) = failed_phase_and_fields(phase, fields);
        let error_code = error
            .as_object()
            .and_then(|map| string_field(map, "error_code"))
            .unwrap_or("error");
        let during = phase_summary(command, failed_phase, failed_fields);
        let during = during
            .strip_prefix(&format!("{command}: "))
            .unwrap_or(during.as_str());
        return format!("{command} failed: {error_code} during {during}");
    }
    if status == "succeeded" {
        return success_summary(command, result);
    }
    phase_summary(command, phase, fields)
}

fn failed_phase_and_fields<'a>(
    phase: Option<&'a str>,
    fields: &'a Value,
) -> (Option<&'a str>, &'a Value) {
    let Some(map) = fields.as_object() else {
        return (phase, fields);
    };
    let failed_phase = string_field(map, "failed_phase").or(phase);
    let failed_fields = map.get("failed_fields").unwrap_or(fields);
    (failed_phase, failed_fields)
}

fn success_summary(command: &str, result: &Value) -> String {
    let Some(result) = result.as_object() else {
        return format!("{command} succeeded");
    };
    match command {
        "pull" => {
            let new_count = value_u64(result.get("new_message_count")).unwrap_or(0);
            let mailbox_count = value_u64(result.get("mailbox_count")).unwrap_or(0);
            format!("pull succeeded: {new_count} new messages across {mailbox_count} mailboxes")
        }
        "push" => {
            let pushed = value_u64(result.get("pushed_count")).unwrap_or(0);
            let failed = value_u64(result.get("failed_count")).unwrap_or(0);
            format!("push succeeded: {pushed} pushed, {failed} failed")
        }
        _ => format!("{command} succeeded"),
    }
}

fn phase_summary(command: &str, phase: Option<&str>, fields: &Value) -> String {
    match command {
        "pull" => pull_phase_summary(phase, fields),
        "push" => push_phase_summary(phase, fields),
        _ => phase
            .map(|phase| format!("{command}: {phase}"))
            .unwrap_or_else(|| command.to_string()),
    }
}

fn pull_phase_summary(phase: Option<&str>, fields: &Value) -> String {
    let Some(fields) = fields.as_object() else {
        return phase
            .map(|phase| format!("pull: {phase}"))
            .unwrap_or_else(|| "pull".to_string());
    };
    let mailbox = mailbox_label(fields);
    match phase {
        Some("pull_mailbox_bodies_start")
        | Some("pull_mailbox_bodies_progress")
        | Some("pull_mailbox_bodies_done") => pull_bodies_summary(&mailbox, fields),
        Some("pull_mailbox_headers_start") => format!("pull: {mailbox} headers"),
        Some("pull_mailbox_headers_done") => {
            let new_candidates = value_u64(fields.get("new_candidate_count")).unwrap_or(0);
            format!("pull: {mailbox} headers done, {new_candidates} new candidates")
        }
        Some("pull_resolve_targets") => {
            let mailbox_count = value_u64(fields.get("mailbox_count")).unwrap_or(0);
            format!("pull: resolved {mailbox_count} mailboxes")
        }
        Some("pull_reconcile_start") => "pull: reconciling local remote state".to_string(),
        Some("pull_reconcile_done") => "pull: reconciled local remote state".to_string(),
        Some("pull_render_start") => "pull: refreshing generated views".to_string(),
        Some("pull_render_done") => "pull: refreshed generated views".to_string(),
        Some(phase) => format!("pull: {phase}"),
        None => "pull".to_string(),
    }
}

fn pull_bodies_summary(mailbox: &str, fields: &Map<String, Value>) -> String {
    let processed = value_u64(fields.get("processed_count")).unwrap_or(0);
    let total = value_u64(fields.get("uid_count")).unwrap_or(0);
    let mut summary = format!("pull: {mailbox} bodies {processed}/{total}");
    if let (Some(batch_index), Some(batch_count)) = (
        value_u64(fields.get("batch_index")),
        value_u64(fields.get("batch_count")),
    ) {
        summary.push_str(&format!(", batch {batch_index}/{batch_count}"));
    }
    summary
}

fn push_phase_summary(phase: Option<&str>, fields: &Value) -> String {
    let Some(fields) = fields.as_object() else {
        return phase
            .map(|phase| format!("push: {phase}"))
            .unwrap_or_else(|| "push".to_string());
    };
    let display_kind = string_field(fields, "display_kind")
        .or_else(|| string_field(fields, "mode"))
        .or_else(|| string_field(fields, "kind"))
        .unwrap_or("item");
    match phase {
        Some("push_start") => {
            let item_count = value_u64(fields.get("item_count")).unwrap_or(0);
            format!("push: {display_kind} started, {item_count} items")
        }
        Some("push_item_start") | Some("push_item_done") | Some("push_item_failed") => {
            let index = value_u64(fields.get("item_index"))
                .or_else(|| value_u64(fields.get("index")))
                .unwrap_or(0);
            let item_count = value_u64(fields.get("item_count")).unwrap_or(0);
            format!("push: {display_kind} item {index}/{item_count}")
        }
        Some("push_step_start") | Some("push_step_done") | Some("push_step_failed") => {
            let step_index = value_u64(fields.get("step_index")).unwrap_or(0);
            let step_count = value_u64(fields.get("step_count")).unwrap_or(0);
            let item = match (
                value_u64(fields.get("item_index")).or_else(|| value_u64(fields.get("index"))),
                value_u64(fields.get("item_count")),
            ) {
                (Some(item_index), Some(item_count)) => format!(" item {item_index}/{item_count},"),
                _ => String::new(),
            };
            format!("push: {display_kind}{item} step {step_index}/{step_count}")
        }
        Some("push_done") => {
            let pushed = value_u64(fields.get("pushed_count")).unwrap_or(0);
            let failed = value_u64(fields.get("failed_count")).unwrap_or(0);
            format!("push: done, {pushed} pushed, {failed} failed")
        }
        Some(phase) => format!("push: {phase}"),
        None => "push".to_string(),
    }
}

fn mailbox_label(fields: &Map<String, Value>) -> String {
    string_field(fields, "mailbox_name")
        .or_else(|| string_field(fields, "mailbox_id"))
        .unwrap_or("mailbox")
        .to_string()
}

fn insert_string(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        map.insert(key.to_string(), json!(value));
    }
}

fn string_field<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

fn value_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}

fn workspace_progress_path(root: &Path) -> PathBuf {
    root.join(".afmail/workspace.progress.json")
}

fn scalar_summary(value: &Value) -> Value {
    let mut out = Map::new();
    let Value::Object(map) = value else {
        return Value::Object(out);
    };
    for (key, value) in map {
        if matches!(
            value,
            Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Null
        ) {
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

fn progress_message(command: &str, phase: &str) -> &'static str {
    match (command, phase) {
        (_, "start") => "afmail command started",
        (_, "finish") => "afmail command finished",
        ("pull", "pull_resolve_targets") => "resolved pull targets",
        ("pull", "pull_mailbox_headers_start") => "fetching mailbox headers",
        ("pull", "pull_mailbox_headers_done") => "fetched mailbox headers",
        ("pull", "pull_mailbox_bodies_start") => "fetching new message bodies",
        ("pull", "pull_mailbox_bodies_progress") => "fetching new message bodies",
        ("pull", "pull_mailbox_bodies_done") => "fetched new message bodies",
        ("pull", "pull_reconcile_start") => "reconciling local remote state",
        ("pull", "pull_reconcile_done") => "reconciled local remote state",
        ("pull", "pull_render_start") => "refreshing generated views",
        ("pull", "pull_render_done") => "refreshed generated views",
        ("push", "push_start") => "pushing queued remote effects",
        ("push", "push_item_start") => "pushing queued item",
        ("push", "push_item_done") => "pushed queued item",
        ("push", "push_item_failed") => "queued item push failed",
        ("push", "push_step_start") => "running push step",
        ("push", "push_step_done") => "finished push step",
        ("push", "push_step_failed") => "push step failed",
        ("push", "push_done") => "finished pushing queued remote effects",
        _ => "afmail command progress",
    }
}
