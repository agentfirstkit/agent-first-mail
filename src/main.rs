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

fn install_stream_redirect_or_exit(
) -> Option<agent_first_data::stream_redirect::InstalledStreamRedirect> {
    match agent_first_data::stream_redirect::install_from_raw_args(std::env::args()) {
        Ok(redirect) => redirect,
        Err(err) => {
            let value = build_cli_error(&err.to_string(), None);
            let rendered = cli_output(&value, OutputFormat::Json);
            let _ = writeln!(std::io::stdout(), "{rendered}");
            std::process::exit(2);
        }
    }
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
