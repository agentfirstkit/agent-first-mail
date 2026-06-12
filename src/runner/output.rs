use crate::cli::Command;
use crate::error::Result;
use agent_first_data::{cli_output, OutputFormat};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn emit_result(result: Result<Value>, output: OutputFormat, duration_ms: u64) -> i32 {
    match result {
        Ok(mut value) => {
            attach_trace(&mut value, duration_ms);
            emit_value(&value, output);
            0
        }
        Err(err) => {
            let mut value = err.to_value();
            attach_trace(&mut value, duration_ms);
            emit_value(&value, output);
            1
        }
    }
}

pub fn emit_value(value: &Value, output: OutputFormat) {
    let rendered = cli_output(value, output);
    let _ = writeln!(std::io::stdout(), "{rendered}");
}

fn attach_trace(value: &mut Value, duration_ms: u64) {
    let Value::Object(map) = value else {
        return;
    };
    map.remove("duration_ms");
    let trace = map.entry("trace").or_insert_with(|| json!({}));
    if !trace.is_object() {
        *trace = json!({});
    }
    if let Value::Object(trace_obj) = trace {
        trace_obj.insert("duration_ms".to_string(), json!(duration_ms));
    }
}

pub(super) fn log_event(event: &str, level: &str, fields: Value, duration_ms: u64) -> Value {
    let mut value = match fields {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    if let Value::Object(map) = &mut value {
        let default_message = if map.contains_key("message") {
            None
        } else {
            Some(log_message(event, map))
        };
        map.insert("code".to_string(), json!("log"));
        map.insert("level".to_string(), json!(level));
        map.insert("event".to_string(), json!(event));
        map.insert(
            "timestamp_epoch_ms".to_string(),
            json!(timestamp_epoch_ms()),
        );
        if let Some(message) = default_message {
            map.insert("message".to_string(), json!(message));
        }
        map.insert("trace".to_string(), json!({"duration_ms": duration_ms}));
    }
    value
}

fn timestamp_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn log_message(event: &str, fields: &Map<String, Value>) -> String {
    match event {
        "startup" => "afmail startup".to_string(),
        "request" => "afmail request started".to_string(),
        "progress" => match fields.get("phase").and_then(Value::as_str) {
            Some("finish") => "afmail command finished".to_string(),
            Some(phase) => format!("afmail command progress: {phase}"),
            None => "afmail command progress".to_string(),
        },
        "retry" => "afmail retryable error".to_string(),
        _ => "afmail log event".to_string(),
    }
}

pub(super) fn log_enabled(filters: &[String], event: &str) -> bool {
    filters.iter().any(|filter| filter == event)
}

pub(super) fn redact_argv(argv: &[String]) -> Vec<String> {
    let mut redact_next = false;
    let mut out = Vec::with_capacity(argv.len());
    for arg in argv {
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }
        if is_secret_assignment(arg) || arg.starts_with("literal:") || arg.contains("=literal:") {
            out.push("***".to_string());
            continue;
        }
        if is_secret_arg_name(arg) {
            out.push(arg.clone());
            redact_next = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn is_secret_assignment(arg: &str) -> bool {
    arg.contains(".password_secret=")
        || (arg.contains("_secret=") && !arg.contains("_secret_env="))
        || (arg.contains("-secret=") && !arg.contains("-secret-env="))
}

fn is_secret_arg_name(arg: &str) -> bool {
    arg.ends_with(".password_secret")
        || arg.ends_with("_secret")
        || arg.ends_with("-secret")
        || arg == "--password-secret"
}

pub(super) fn output_format_name(output: OutputFormat) -> &'static str {
    match output {
        OutputFormat::Json => "json",
        OutputFormat::Yaml => "yaml",
        OutputFormat::Plain => "plain",
    }
}

pub(super) fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Init => "init",
        Command::Pull { .. } => "pull",
        Command::Config { .. } => "config",
        Command::Remote { .. } => "remote",
        Command::Push { .. } => "push",
        Command::Status => "status",
        Command::Doctor { .. } => "doctor",
        Command::Purge { .. } => "purge",
        Command::Skill { .. } => "skill",
        Command::Triage { .. } => "triage",
        Command::Message { .. } => "message",
        Command::Case { .. } => "case",
        Command::Archive { .. } => "archive",
        Command::Render { .. } => "render",
        Command::Log { .. } => "log",
    }
}
