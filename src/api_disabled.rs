//! Structured fallback for builds without the `api` feature.

use std::io::Write;
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};

use crate::cli::Command;
use crate::error::{AppError, feature_disabled};
use crate::types::CommandResult;

pub(crate) fn is_api_command(command: &Command) -> bool {
    matches!(command, Command::Api { .. })
}

pub(crate) fn run_api_command<W: Write>(
    _command: &Command,
    _emitter: &mut CliEmitter<W>,
    _started: Instant,
) -> Result<CommandResult, ApiRunError> {
    Err(feature_disabled("api", "api").into())
}

pub(crate) enum ApiRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for ApiRunError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl From<CliEmitterError> for ApiRunError {
    fn from(value: CliEmitterError) -> Self {
        Self::Output(value)
    }
}
