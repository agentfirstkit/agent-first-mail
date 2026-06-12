mod dispatch;
mod lock;
mod output;
mod purge;
mod push;

pub use dispatch::execute_command;
pub use output::{emit_result, emit_value};

use crate::cli::Command;
use crate::progress::{object_with_phase, ProgressCallback};
use agent_first_data::OutputFormat;
use dispatch::execute_command_with_progress;
use output::{command_name, log_enabled, log_event, output_format_name, redact_argv};
use serde_json::{json, Value};
use std::time::Instant;

pub fn run_command(
    command: Command,
    output: OutputFormat,
    log_filters: &[String],
    argv: &[String],
) -> i32 {
    let started = Instant::now();
    let command_name = command_name(&command).to_string();
    if log_enabled(log_filters, "startup") {
        let argv = redact_argv(argv);
        emit_value(
            &log_event(
                "startup",
                "info",
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "argv": argv,
                    "command": command_name.as_str(),
                    "output_format": output_format_name(output),
                    "log_filters": log_filters,
                }),
                0,
            ),
            output,
        );
    }
    if log_enabled(log_filters, "request") {
        let cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        emit_value(
            &log_event(
                "request",
                "info",
                json!({
                    "phase": "start",
                    "command": command_name.as_str(),
                    "cwd_path": cwd,
                }),
                0,
            ),
            output,
        );
    }

    let progress_enabled = log_enabled(log_filters, "progress");
    let mut emit_progress = |phase: &str, fields: Value| {
        emit_value(
            &log_event(
                "progress",
                "info",
                object_with_phase(phase, fields),
                started.elapsed().as_millis() as u64,
            ),
            output,
        );
    };
    let progress = if progress_enabled {
        Some(&mut emit_progress as &mut ProgressCallback<'_>)
    } else {
        None
    };
    let result = execute_command_with_progress(command, progress);
    let duration_ms = started.elapsed().as_millis() as u64;
    if let Err(err) = &result {
        if err.retryable && log_enabled(log_filters, "retry") {
            emit_value(
                &log_event(
                    "retry",
                    "warn",
                    json!({
                        "phase": "retryable_error",
                        "command": command_name.as_str(),
                        "error_code": err.error_code,
                        "error": err.message,
                    }),
                    duration_ms,
                ),
                output,
            );
        }
    }
    if log_enabled(log_filters, "progress") {
        emit_value(
            &log_event(
                "progress",
                "info",
                json!({
                    "phase": "finish",
                    "command": command_name.as_str(),
                    "success": result.is_ok(),
                    "exit_code": if result.is_ok() { 0 } else { 1 },
                }),
                duration_ms,
            ),
            output,
        );
    }
    emit_result(result, output, duration_ms)
}
