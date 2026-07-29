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

use agent_first_data::{OutputFormat, OutputTo, build_cli_error, cli_parse_output};
use agent_first_mail::cli;
use agent_first_mail::runner::output_write_failed_exit_code;
use agent_first_mail::runner::run_command;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let _stream_redirect = install_stream_redirect_or_exit();
    let started = Instant::now();
    handle_help_and_version(&started);
    let argv = std::env::args().collect::<Vec<_>>();
    let parsed = match cli::parse_args() {
        Ok(mode) => mode,
        Err(err) => {
            let hint = cli::error_hint(&argv);
            let value = cli_error_with_trace(&err, Some(hint.as_str()), elapsed_ms(&started));
            let output = requested_output_format().unwrap_or(OutputFormat::Json);
            let output_to = requested_output_to().unwrap_or(OutputTo::Split);
            match emit_value(value, output, output_to) {
                Ok(()) => std::process::exit(2),
                Err(err) => std::process::exit(output_write_failed_exit_code(&err)),
            }
        }
    };
    let cli::ParsedArgs {
        command,
        output,
        output_to,
        log,
        workspace,
        request_id,
    } = parsed;
    if let Err(err) = apply_explicit_workspace(workspace.as_deref()) {
        let value = cli_error_with_trace(&err, None, elapsed_ms(&started));
        match emit_value(value, output, output_to) {
            Ok(()) => std::process::exit(1),
            Err(err) => std::process::exit(output_write_failed_exit_code(&err)),
        }
    }
    std::process::exit(run_command(
        command, request_id, output, output_to, &log, &argv,
    ));
}

fn apply_explicit_workspace(workspace: Option<&str>) -> Result<(), String> {
    let Some(workspace) = workspace else {
        return Ok(());
    };
    let path = PathBuf::from(workspace);
    let canonical = std::fs::canonicalize(&path)
        .map_err(|err| format!("canonicalize --workspace {}: {err}", display_path(&path)))?;
    if !canonical.join(".afmail").is_dir() {
        return Err(format!(
            "--workspace {} is not an afmail workspace root: missing .afmail directory",
            display_path(&canonical)
        ));
    }
    std::env::set_current_dir(&canonical)
        .map_err(|err| format!("enter --workspace {}: {err}", display_path(&canonical)))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn install_stream_redirect_or_exit()
-> Option<agent_first_data::stream_redirect::InstalledStreamRedirect> {
    match agent_first_data::stream_redirect::install_from_raw_args(std::env::args()) {
        Ok(redirect) => redirect,
        Err(err) => {
            let output_to = requested_output_to().unwrap_or(OutputTo::Split);
            let mut emitter =
                agent_first_data::CliEmitter::from_output_to(output_to, OutputFormat::Json)
                    .with_strict_protocol();
            match emitter.emit_error("cli_error", &err.to_string()) {
                Ok(()) => std::process::exit(2),
                Err(err) => std::process::exit(output_write_failed_exit_code(&err)),
            }
        }
    }
}

fn handle_help_and_version(started: &Instant) {
    let raw: Vec<String> = std::env::args().collect();
    let build = match env!("GIT_SHA") {
        "unknown" => None,
        sha => Some(sha),
    };
    match agent_first_data::cli_handle_version_or_help_or_continue(
        &raw,
        &cli::command(),
        &agent_first_data::HelpConfig::output_aware(),
        "afmail",
        Some(env!("DISPLAY_NAME")),
        env!("CARGO_PKG_VERSION"),
        build,
    ) {
        Ok(Some(version)) => {
            match write_result_text(&version, requested_output_to().unwrap_or(OutputTo::Split)) {
                Ok(()) => std::process::exit(0),
                Err(err) => std::process::exit(output_write_failed_exit_code(&err)),
            }
        }
        Ok(None) => {}
        Err(err) => {
            let mut err: Value = err;
            attach_trace(&mut err, elapsed_ms(started));
            let output = requested_output_format().unwrap_or(OutputFormat::Json);
            let output_to = requested_output_to().unwrap_or(OutputTo::Split);
            match emit_value(err, output, output_to) {
                Ok(()) => std::process::exit(2),
                Err(err) => std::process::exit(output_write_failed_exit_code(&err)),
            }
        }
    }
}

fn requested_output_to() -> Option<OutputTo> {
    requested_output_to_result().ok().flatten()
}

fn requested_output_to_result() -> Result<Option<OutputTo>, String> {
    let raw = std::env::args().collect::<Vec<_>>();
    let mut output_to = None;
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if let Some(value) = arg.strip_prefix("--output-to=") {
            output_to = Some(OutputTo::parse(value)?);
            continue;
        }
        if arg == "--output-to" {
            let value = iter.next().ok_or_else(|| {
                "--output-to requires a value: expected split, stdout, or stderr".to_string()
            })?;
            output_to = Some(OutputTo::parse(value)?);
        }
    }
    Ok(output_to)
}

fn emit_value(
    value: Value,
    output: OutputFormat,
    output_to: OutputTo,
) -> Result<(), agent_first_data::CliEmitterError> {
    let mut emitter =
        agent_first_data::CliEmitter::from_output_to(output_to, output).with_strict_protocol();
    emitter.emit_validated_value(value)
}

#[allow(clippy::disallowed_methods)]
fn write_result_text(text: &str, output_to: OutputTo) -> std::io::Result<()> {
    let mut writer: Box<dyn Write> = match output_to {
        OutputTo::Stderr => Box::new(std::io::stderr()),
        OutputTo::Split | OutputTo::Stdout => Box::new(std::io::stdout()),
    };
    writer.write_all(text.as_bytes())?;
    writer.flush()
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
    let mut value: Value = build_cli_error(message, hint).into();
    attach_trace(&mut value, duration_ms);
    value
}

fn attach_trace(value: &mut Value, duration_ms: u64) {
    let Value::Object(map) = value else {
        return;
    };
    if let Some(Value::Object(error)) = map.get_mut("error") {
        error
            .entry("retryable".to_string())
            .or_insert_with(|| json!(false));
    }
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
