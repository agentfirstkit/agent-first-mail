//! Structured fallback for builds without the `demo` feature.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};
use serde_json::Value;

use crate::cli::{Command, DemoAction, DemoScenario};
use crate::error::{AppError, Result, feature_disabled};
use crate::types::CommandResult;

pub(crate) fn is_demo_serve_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Demo {
            action: DemoAction::Serve { .. }
        }
    )
}

pub(crate) fn init(
    _cwd: &Path,
    _path: &str,
    _scenario: DemoScenario,
    _imap_port: u16,
    _smtp_port: u16,
) -> Result<Value> {
    Err(feature_disabled("demo", "demo"))
}

pub(crate) fn run_serve_command<W: Write>(
    _command: &Command,
    _emitter: &mut CliEmitter<W>,
    _started: Instant,
) -> std::result::Result<CommandResult, DemoRunError> {
    Err(feature_disabled("demo", "demo").into())
}

pub(crate) enum DemoRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for DemoRunError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl From<CliEmitterError> for DemoRunError {
    fn from(value: CliEmitterError) -> Self {
        Self::Output(value)
    }
}
