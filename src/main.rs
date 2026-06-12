#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::print_stdout,
        clippy::print_stderr,
    )
)]

use agent_first_data::{
    build_cli_error, cli_output, cli_parse_output, OutputFormat, VersionConfig,
};
use agent_first_mail::cli;
use agent_first_mail::runner::run_command;
use serde_json::{json, Value};
use std::io::Write;
use std::time::Instant;

fn main() {
    let started = Instant::now();
    handle_help_and_version(&started);
    let argv = std::env::args().collect::<Vec<_>>();
    let parsed = match cli::parse_args() {
        Ok(mode) => mode,
        Err(err) => {
            let hint = cli_error_hint(&argv);
            let value = cli_error_with_trace(&err, Some(hint.as_str()), elapsed_ms(&started));
            let output = requested_output_format().unwrap_or(OutputFormat::Json);
            let _ = writeln!(std::io::stdout(), "{}", cli_output(&value, output));
            std::process::exit(2);
        }
    };
    let cli::ParsedArgs {
        command,
        output,
        log,
    } = parsed;
    std::process::exit(run_command(command, output, &log, &argv));
}

fn handle_help_and_version(started: &Instant) {
    let raw: Vec<String> = std::env::args().collect();
    match agent_first_data::cli_handle_version_or_continue(
        &raw,
        "afmail",
        env!("CARGO_PKG_VERSION"),
        &VersionConfig::conventional_default(),
    ) {
        Ok(Some(version)) => {
            let _ = write!(std::io::stdout(), "{version}");
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(err) => {
            let mut err = err;
            attach_trace(&mut err, elapsed_ms(started));
            let _ = writeln!(
                std::io::stdout(),
                "{}",
                cli_output(&err, OutputFormat::Json)
            );
            std::process::exit(2);
        }
    }
    match agent_first_data::cli_handle_help_or_continue(
        &raw,
        &cli::command(),
        &agent_first_data::HelpConfig::human_cli_default(),
    ) {
        Ok(Some(help)) => {
            let _ = write!(std::io::stdout(), "{help}");
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(err) => {
            let mut err = err;
            attach_trace(&mut err, elapsed_ms(started));
            let output = requested_output_format().unwrap_or(OutputFormat::Json);
            let _ = writeln!(std::io::stdout(), "{}", cli_output(&err, output));
            std::process::exit(2);
        }
    }
}

fn requested_output_format() -> Option<OutputFormat> {
    requested_output_format_result().ok().flatten()
}

fn requested_output_format_result() -> Result<Option<OutputFormat>, String> {
    let raw = std::env::args().collect::<Vec<_>>();
    let mut output = None;
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if let Some(value) = arg.strip_prefix("--output=") {
            output = Some(cli_parse_output(value)?);
            continue;
        }
        if arg == "--output" {
            if let Some(value) = iter.next() {
                output = Some(cli_parse_output(value)?);
            } else {
                return Err("--output requires a value: expected json, yaml, or plain".to_string());
            }
        }
    }
    Ok(output)
}

fn cli_error_with_trace(message: &str, hint: Option<&str>, duration_ms: u64) -> Value {
    let mut value = build_cli_error(message, hint);
    attach_trace(&mut value, duration_ms);
    value
}

fn attach_trace(value: &mut Value, duration_ms: u64) {
    let Value::Object(map) = value else {
        return;
    };
    let trace = map.entry("trace").or_insert_with(|| json!({}));
    if !trace.is_object() {
        *trace = json!({});
    }
    if let Value::Object(trace_obj) = trace {
        trace_obj.insert("duration_ms".to_string(), json!(duration_ms));
    }
}

fn elapsed_ms(started: &Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn cli_error_hint(argv: &[String]) -> String {
    let tokens = command_tokens(argv);
    if tokens.is_empty() {
        return "try: afmail --help".to_string();
    }
    match tokens[0].as_str() {
        "case" => nested_hint("afmail case", tokens.get(1), CASE_ACTIONS),
        "message" => {
            if tokens.get(1).is_some_and(|token| token == "attachment") {
                nested_hint(
                    "afmail message attachment",
                    tokens.get(2),
                    MESSAGE_ATTACHMENT_ACTIONS,
                )
            } else {
                nested_hint("afmail message", tokens.get(1), MESSAGE_ACTIONS)
            }
        }
        "archive" => match tokens.get(1).map(String::as_str) {
            Some("message") => nested_hint(
                "afmail archive message",
                tokens.get(2),
                ARCHIVE_MESSAGE_ACTIONS,
            ),
            Some("case") => nested_hint("afmail archive case", tokens.get(2), ARCHIVE_CASE_ACTIONS),
            Some("list") => nested_hint("afmail archive list", tokens.get(2), ARCHIVE_LIST_ACTIONS),
            Some(_) => "try: afmail archive --help".to_string(),
            None => "try: afmail archive --help".to_string(),
        },
        "config" => nested_hint("afmail config", tokens.get(1), CONFIG_ACTIONS),
        "remote" => nested_hint("afmail remote", tokens.get(1), REMOTE_ACTIONS),
        "push" => nested_hint("afmail push", tokens.get(1), PUSH_ACTIONS),
        "doctor" => nested_hint("afmail doctor", tokens.get(1), DOCTOR_ACTIONS),
        "skill" => nested_hint("afmail skill", tokens.get(1), SKILL_ACTIONS),
        "triage" => nested_hint("afmail triage", tokens.get(1), TRIAGE_ACTIONS),
        "render" => nested_hint("afmail render", tokens.get(1), RENDER_ACTIONS),
        "log" => nested_hint("afmail log", tokens.get(1), LOG_ACTIONS),
        command if ROOT_COMMANDS.contains(&command) => format!("try: afmail {command} --help"),
        _ => "try: afmail --help".to_string(),
    }
}

fn nested_hint(prefix: &str, action: Option<&String>, known_actions: &[&str]) -> String {
    if let Some(action) = action {
        if known_actions.contains(&action.as_str()) {
            return format!("try: {prefix} {action} --help");
        }
    }
    format!("try: {prefix} --help")
}

fn command_tokens(argv: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if arg == "--output" || arg == "--log" {
            let _ = iter.next();
            continue;
        }
        if arg == "--verbose" || arg.starts_with("--output=") || arg.starts_with("--log=") {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

const ROOT_COMMANDS: &[&str] = &[
    "init", "pull", "config", "remote", "push", "status", "doctor", "purge", "skill", "triage",
    "message", "case", "archive", "render", "log",
];
const CASE_ACTIONS: &[&str] = &[
    "create", "list", "show", "add", "move", "rename", "notes", "archive", "reopen", "tag",
    "untag", "draft", "compose", "reply", "merge",
];
const MESSAGE_ACTIONS: &[&str] = &[
    "show",
    "archive",
    "spam",
    "unspam",
    "trash",
    "untrash",
    "unarchive",
    "attachment",
];
const MESSAGE_ATTACHMENT_ACTIONS: &[&str] = &["fetch"];
const ARCHIVE_MESSAGE_ACTIONS: &[&str] = &[
    "create",
    "show",
    "restore",
    "move",
    "rename",
    "set-summary",
    "notes",
];
const ARCHIVE_CASE_ACTIONS: &[&str] = &["show", "restore", "rename", "notes"];
const ARCHIVE_LIST_ACTIONS: &[&str] = &["cases", "messages"];
const CONFIG_ACTIONS: &[&str] = &["show", "get", "set"];
const REMOTE_ACTIONS: &[&str] = &["test", "folders"];
const PUSH_ACTIONS: &[&str] = &["list", "drafts", "drafts-send", "archive", "spam", "trash"];
const DOCTOR_ACTIONS: &[&str] = &["repair"];
const SKILL_ACTIONS: &[&str] = &["status", "install", "uninstall"];
const TRIAGE_ACTIONS: &[&str] = &["list"];
const RENDER_ACTIONS: &[&str] = &["refresh", "templates"];
const LOG_ACTIONS: &[&str] = &["list", "tail", "message", "case", "archive"];
