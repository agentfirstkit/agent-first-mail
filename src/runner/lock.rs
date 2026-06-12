use super::push::{push_has_confirm, push_has_dry_run};
use crate::cli::{
    ArchiveAction, ArchiveCaseAction, ArchiveCaseNotesAction, ArchiveMessageCommand,
    ArchiveMessageNotesAction, CaseCommand, CaseNotesAction, Command, ConfigAction, DoctorAction,
    MessageAction, PushAction, RemoteAction, RenderAction, TriageAction,
};
use crate::error::Result;
use crate::store::Workspace;
use crate::workspace_lock::LockMode;
use std::path::{Path, PathBuf};

pub(super) struct CommandLock {
    pub(super) root: PathBuf,
    pub(super) mode: LockMode,
}

pub(super) fn command_lock(command: &Command, cwd: &Path) -> Result<CommandLock> {
    let mode = lock_mode(command);
    let root = if matches!(command, Command::Init) {
        cwd.to_path_buf()
    } else {
        Workspace::discover(cwd)?.root().to_path_buf()
    };
    Ok(CommandLock { root, mode })
}

fn lock_mode(command: &Command) -> LockMode {
    match command {
        Command::Init | Command::Pull { .. } | Command::Purge { .. } => LockMode::Exclusive,
        Command::Message { action } => match action {
            MessageAction::Show { .. } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Config { action } => match action {
            ConfigAction::Show | ConfigAction::Get { .. } => LockMode::Shared,
            ConfigAction::Set { .. } => LockMode::Exclusive,
        },
        Command::Remote { action } => match action {
            RemoteAction::Test | RemoteAction::Folders => LockMode::Shared,
        },
        Command::Push {
            dry_run: _,
            confirm,
            action,
        } => match action {
            Some(PushAction::List) => LockMode::Shared,
            _ if push_has_confirm(*confirm, action) => LockMode::Exclusive,
            _ => LockMode::Shared,
        },
        Command::Status => LockMode::Shared,
        Command::Doctor { action } => match action {
            Some(DoctorAction::Repair { confirm: true }) => LockMode::Exclusive,
            _ => LockMode::Shared,
        },
        Command::Skill { .. } => LockMode::Shared,
        Command::Triage { action } => match action {
            TriageAction::List => LockMode::Shared,
        },
        Command::Case { action } => match action {
            CaseCommand::Create(_) => LockMode::Exclusive,
            CaseCommand::Show { .. }
            | CaseCommand::List
            | CaseCommand::Notes {
                action: CaseNotesAction::Show { .. },
            } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Archive { action } => match action {
            ArchiveAction::List { .. } => LockMode::Shared,
            ArchiveAction::Message {
                action:
                    ArchiveMessageCommand::Show { .. }
                    | ArchiveMessageCommand::Notes {
                        action: ArchiveMessageNotesAction::Show { .. },
                    },
            } => LockMode::Shared,
            ArchiveAction::Case {
                action:
                    ArchiveCaseAction::Show { .. }
                    | ArchiveCaseAction::Notes {
                        action: ArchiveCaseNotesAction::Show { .. },
                    },
            } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Render { action } => match action {
            RenderAction::Refresh => LockMode::Exclusive,
            RenderAction::Templates { .. } => LockMode::Exclusive,
        },
        Command::Log { .. } => LockMode::Shared,
    }
}

pub(super) fn workspace_progress_command(command: &Command) -> Option<&'static str> {
    match command {
        Command::Pull { .. } => Some("pull"),
        Command::Push {
            dry_run,
            confirm,
            action,
        } if push_has_confirm(*confirm, action) && !push_has_dry_run(*dry_run, action) => {
            Some("push")
        }
        _ => None,
    }
}
