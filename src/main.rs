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
    BoundOutcome, CliEmitter, Event, OutputFormat, OutputPlan, OutputTo, ResolvedInvocation,
    cli_error_event, cli_help_event, cli_parse_output, cli_version_event, render_cli_reference,
};
use agent_first_mail::cli::{self, ArgumentError, Command};
use agent_first_mail::runner::output_write_failed_exit_code;
use agent_first_mail::runner::run_command;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = match cli::cli_spec() {
        Ok(cli) => cli,
        Err(error) => return startup_error("cli_spec_invalid", &error.to_string()),
    };
    let app = match cli.bind_actions(cli::action_handlers(&cli)) {
        Ok(app) => app,
        Err(error) => return startup_error("cli_actions_invalid", &error.to_string()),
    };

    let outcome = match app.resolve_from(std::env::args_os()) {
        Ok(outcome) => outcome,
        // Rejected before anything ran: the error names its own rule in
        // `error.code`, and always lands on the diagnostic stream.
        Err(error) => {
            return emit_event(
                cli_error_event(&error),
                OutputFormat::Json,
                OutputTo::Stderr,
                error.exit_code(),
            );
        }
    };

    match outcome {
        BoundOutcome::Run(invocation) => {
            let _redirect = match install_output_redirect(invocation.output_plan()) {
                Ok(guard) => guard,
                Err(message) => return output_sink_error(&message),
            };
            let (format, output_to) = plan_output(invocation.output_plan());
            // The globals `run` reads come from the same resolved argv the
            // handler saw, which is why the invocation comes back out with it.
            let (built, invocation) = invocation.run_with_invocation();
            match built {
                Ok(command) => run(command, &invocation, format, output_to),
                Err(error) => emit_argument_error(&error, format, output_to),
            }
        }
        // `--docs` renders the whole registry as raw Markdown, so it carries no
        // format of its own and never becomes a protocol event.
        BoundOutcome::Docs(docs) => {
            let _redirect = match install_output_redirect(docs.output_plan()) {
                Ok(guard) => guard,
                Err(message) => return output_sink_error(&message),
            };
            write_text(&render_cli_reference(&cli), OutputTo::Stdout)
        }
        BoundOutcome::Help(help) => {
            let _redirect = match install_output_redirect(help.output_plan()) {
                Ok(guard) => guard,
                Err(message) => return output_sink_error(&message),
            };
            let (format, output_to) = plan_output(help.output_plan());
            if format == OutputFormat::Plain {
                write_text(&help.plain(), result_stream(output_to))
            } else {
                emit_event(cli_help_event(&help), format, output_to, 0)
            }
        }
        BoundOutcome::Version(version) => {
            let _redirect = match install_output_redirect(version.output_plan()) {
                Ok(guard) => guard,
                Err(message) => return output_sink_error(&message),
            };
            let (format, output_to) = plan_output(version.output_plan());
            emit_event(cli_version_event(&version), format, output_to, 0)
        }
    }
}

fn run(
    command: Command,
    invocation: &ResolvedInvocation,
    format: OutputFormat,
    output_to: OutputTo,
) -> ExitCode {
    let globals = cli::globals(invocation);
    if let Err(message) = apply_explicit_workspace(globals.workspace.as_deref()) {
        return emit_error("workspace_not_selected", &message, format, output_to, 1);
    }
    let argv = std::env::args().collect::<Vec<_>>();
    let exit_code = run_command(
        command,
        globals.request_id,
        format,
        output_to,
        &globals.log,
        &argv,
        invocation.command_path(),
    );
    ExitCode::from(u8::try_from(exit_code).unwrap_or(1))
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

fn plan_output(plan: &OutputPlan) -> (OutputFormat, OutputTo) {
    let format = plan
        .format()
        .and_then(|format| cli_parse_output(format).ok())
        .unwrap_or(OutputFormat::Json);
    let output_to = plan
        .destination()
        .and_then(|destination| OutputTo::parse(destination).ok())
        .unwrap_or(OutputTo::Split);
    (format, output_to)
}

fn result_stream(output_to: OutputTo) -> OutputTo {
    match output_to {
        OutputTo::Stderr => OutputTo::Stderr,
        OutputTo::Split | OutputTo::Stdout => OutputTo::Stdout,
    }
}

type OutputRedirectGuard = agent_first_data::stream_redirect::InstalledStreamRedirect;

fn install_output_redirect(plan: &OutputPlan) -> Result<Option<OutputRedirectGuard>, String> {
    let config = agent_first_data::stream_redirect::StreamRedirectConfig::new(
        plan.stdout_file().map(Path::to_path_buf),
        plan.stderr_file().map(Path::to_path_buf),
    )
    .map_err(|error| error.to_string())?;
    config
        .as_ref()
        .map(agent_first_data::stream_redirect::install)
        .transpose()
        .map_err(|error| error.to_string())
}

fn emit_argument_error(
    error: &ArgumentError,
    format: OutputFormat,
    output_to: OutputTo,
) -> ExitCode {
    // Same classification, same emit path as the resolver's own rejections
    // above. Spelling the code and the exit status out here instead let one
    // file carry two copies of AFDATA's answer for one kind of failure.
    let cli_error = agent_first_data::CliError {
        rule: agent_first_data::CliErrorRule::InvalidArgumentValue,
        message: error.message.clone(),
        hint: error.hint.clone(),
    };
    emit_event(
        cli_error_event(&cli_error),
        format,
        output_to,
        cli_error.exit_code(),
    )
}

fn emit_error(
    code: &str,
    message: &str,
    format: OutputFormat,
    output_to: OutputTo,
    exit_code: u8,
) -> ExitCode {
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    match emitter.emit_error(code, message) {
        Ok(()) => ExitCode::from(exit_code),
        Err(err) => ExitCode::from(u8::try_from(output_write_failed_exit_code(&err)).unwrap_or(4)),
    }
}

fn emit_event(event: Event, format: OutputFormat, output_to: OutputTo, exit_code: u8) -> ExitCode {
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    match emitter.emit(event) {
        Ok(()) => ExitCode::from(exit_code),
        Err(err) => ExitCode::from(u8::try_from(output_write_failed_exit_code(&err)).unwrap_or(4)),
    }
}

/// A registry that fails to build is a programming error in this binary, not a
/// caller mistake, so it reports before any output contract has been resolved.
fn startup_error(code: &str, message: &str) -> ExitCode {
    emit_error(code, message, OutputFormat::Json, OutputTo::Stderr, 1)
}

/// An unusable `--stdout-file`/`--stderr-file` target. The registry types the
/// argument as a path but cannot know what is at it, so this reports the same
/// classification the parser would for any other bad argument value.
fn output_sink_error(message: &str) -> ExitCode {
    let error = agent_first_data::CliError {
        rule: agent_first_data::CliErrorRule::InvalidArgumentValue,
        message: message.to_string(),
        hint: "choose a writable path for the output file".to_string(),
    };
    emit_event(
        cli_error_event(&error),
        OutputFormat::Json,
        OutputTo::Stderr,
        error.exit_code(),
    )
}

// AFDATA injects the raw outcomes this writes (`--docs`, plain help), so it owns
// the routing and the rule that a closed reader is success rather than failure.
fn write_text(text: &str, output_to: OutputTo) -> ExitCode {
    match agent_first_data::write_raw(text, output_to) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(4),
    }
}
