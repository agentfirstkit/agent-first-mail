//! Structured fallback for builds without the `review` feature.

use std::io::Write;
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};

use crate::cli::{CaseCommand, CaseDraftAction, Command, NotificationCommand, TriageAction};
use crate::error::{AppError, feature_disabled};
use crate::types::CommandResult;

pub(super) fn is_review_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Triage {
            action: TriageAction::Review { .. },
        } | Command::Case {
            action: CaseCommand::Review { .. }
                | CaseCommand::Draft {
                    action: CaseDraftAction::Review { .. },
                },
        } | Command::Notification {
            action: NotificationCommand::Review { .. },
        }
    )
}

pub(super) fn run_review_command<W: Write>(
    _command: &Command,
    _emitter: &mut CliEmitter<W>,
    _started: Instant,
) -> Result<CommandResult, ReviewRunError> {
    Err(feature_disabled("review", "review").into())
}

pub(super) enum ReviewRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for ReviewRunError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl From<CliEmitterError> for ReviewRunError {
    fn from(value: CliEmitterError) -> Self {
        Self::Output(value)
    }
}
