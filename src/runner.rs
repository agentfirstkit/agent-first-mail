pub(crate) mod dispatch;
mod lock;
mod output;
mod purge;
#[cfg(feature = "review")]
mod review;
#[cfg(not(feature = "review"))]
#[path = "runner/review_disabled.rs"]
mod review;

/// Rendering one review page, for `doctor` to validate a Review UI through the
/// same engine that serves it.
#[cfg(feature = "review")]
pub(crate) use review::render_page_from as render_review_page_from;

pub use dispatch::execute_command;
pub use output::{emit_result, output_write_failed_exit_code};

use crate::cli::Command;
use crate::progress::{ProgressCallback, object_with_phase};
use agent_first_data::{CliEmitter, CliEmitterError, LogFilters, OutputFormat, OutputTo};
use dispatch::execute_command_with_progress;
use output::{
    argv_shape, log_event, output_format_name, progress_event, redact_argv, redact_log_fields,
};
use serde_json::{Value, json};
use std::time::Instant;

fn emit_log_event<W: std::io::Write>(
    emitter: &mut CliEmitter<W>,
    value: Value,
    trace: Value,
) -> Result<(), CliEmitterError> {
    emitter.emit(agent_first_data::json_log(value).trace(trace).build())
}

fn emit_progress_event<W: std::io::Write>(
    emitter: &mut CliEmitter<W>,
    value: Value,
    trace: Value,
) -> Result<(), CliEmitterError> {
    emitter.emit(agent_first_data::json_progress(value).trace(trace).build())
}

pub fn run_command(
    command: Command,
    request_id: Option<String>,
    output: OutputFormat,
    output_to: OutputTo,
    log_filters: &LogFilters,
    argv: &[String],
    command_path: &[String],
) -> i32 {
    let started = Instant::now();
    let mut emitter = CliEmitter::from_output_to(output_to, output).with_strict_protocol();
    // The path the registry resolved to, so a diagnostic names `api export`
    // rather than the `api` group it happens to live under.
    let command_name = command_path.join(" ");
    if log_filters.enabled("startup") {
        let argv_shape = argv_shape(argv, command_path);
        let argv = redact_argv(argv, command_path);
        if let Err(err) = emit_log_event(
            &mut emitter,
            log_event(
                "startup",
                "info",
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "argv": argv,
                    "argv_shape": argv_shape,
                    "command": command_name.as_str(),
                    "output_format": output_format_name(output),
                    "log_filters": log_filters.as_slice(),
                }),
            ),
            json!({"duration_ms": 0}),
        ) {
            return output_write_failed_exit_code(&err);
        }
    }
    if log_filters.enabled("request") {
        let cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if let Err(err) = emit_log_event(
            &mut emitter,
            log_event(
                "request",
                "info",
                json!({
                    "phase": "start",
                    "command": command_name.as_str(),
                    "cwd_path": cwd,
                }),
            ),
            json!({"duration_ms": 0}),
        ) {
            return output_write_failed_exit_code(&err);
        }
    }

    let progress_enabled = log_filters.enabled("progress");
    let mut progress_write_error: Option<CliEmitterError> = None;
    let is_review_command = review::is_review_command(&command);
    let is_api_command = crate::api::is_api_command(&command);
    let is_demo_serve_command = crate::demo::is_demo_serve_command(&command);
    let result = if is_review_command {
        match dispatch::validate_request_id(&command, request_id.as_deref()) {
            Err(error) => Err(error),
            Ok(()) => match review::run_review_command(&command, &mut emitter, started) {
                Ok(result) => Ok(result),
                Err(review::ReviewRunError::App(error)) => Err(error),
                Err(review::ReviewRunError::Output(error)) => {
                    return output_write_failed_exit_code(&error);
                }
            },
        }
    } else if is_api_command {
        match dispatch::validate_request_id(&command, request_id.as_deref()) {
            Err(error) => Err(error),
            Ok(()) => match crate::api::run_api_command(&command, &mut emitter, started) {
                Ok(result) => Ok(result),
                Err(crate::api::ApiRunError::App(error)) => Err(error),
                Err(crate::api::ApiRunError::Output(error)) => {
                    return output_write_failed_exit_code(&error);
                }
            },
        }
    } else if is_demo_serve_command {
        match dispatch::validate_request_id(&command, request_id.as_deref()) {
            Err(error) => Err(error),
            Ok(()) => match crate::demo::run_serve_command(&command, &mut emitter, started) {
                Ok(result) => Ok(result),
                Err(crate::demo::DemoRunError::App(error)) => Err(error),
                Err(crate::demo::DemoRunError::Output(error)) => {
                    return output_write_failed_exit_code(&error);
                }
            },
        }
    } else {
        let mut emit_progress = |phase: &str, fields: Value| {
            if progress_write_error.is_some() {
                return;
            }
            if let Err(err) = emit_progress_event(
                &mut emitter,
                progress_event(redact_log_fields(object_with_phase(phase, fields))),
                json!({"duration_ms": started.elapsed().as_millis() as u64}),
            ) {
                progress_write_error = Some(err);
            }
        };
        let progress = if progress_enabled {
            Some(&mut emit_progress as &mut ProgressCallback<'_>)
        } else {
            None
        };
        execute_command_with_progress(command, request_id.as_deref(), progress)
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    if let Err(err) = &result
        && err.retryable
        && log_filters.enabled("retry")
        && let Err(err) = emit_log_event(
            &mut emitter,
            log_event(
                "retry",
                "warn",
                redact_log_fields(json!({
                    "phase": "retryable_error",
                    "command": command_name.as_str(),
                    "error_code": err.error_code,
                    "error": err.message,
                })),
            ),
            json!({"duration_ms": duration_ms}),
        )
    {
        return output_write_failed_exit_code(&err);
    }
    let (status, success, exit_code) = match &result {
        Ok(result) if result.is_success() => ("succeeded", true, 0),
        Ok(result) if result.is_partial() => ("partial", false, 3),
        Ok(_) | Err(_) => ("failed", false, 1),
    };
    let finish_level = match status {
        "succeeded" => "info",
        "partial" => "warn",
        _ => "error",
    };
    if let Some(err) = progress_write_error {
        return output_write_failed_exit_code(&err);
    }
    if log_filters.enabled("progress")
        && let Err(err) = emit_progress_event(
            &mut emitter,
            progress_event(json!({
                "phase": "finish",
                "command": command_name.as_str(),
                "status": status,
                "success": success,
                "exit_code": exit_code,
                "level": finish_level,
            })),
            json!({"duration_ms": duration_ms}),
        )
    {
        return output_write_failed_exit_code(&err);
    }
    match emit_result(&mut emitter, result, duration_ms) {
        Ok(exit_code) => exit_code,
        Err(err) => output_write_failed_exit_code(&err),
    }
}
