use crate::cli::{
    CaseCommand, CaseDraftAction, CaseNotesAction, Command, ConfigAction, ContactCommand,
    ContactNotesAction, DemoAction, DoctorAction, MessageAction, NotificationCommand,
    NotificationNotesAction, PushAction, RemoteAction, RenderAction, TriageAction,
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
    let root = if matches!(
        command,
        Command::Init { .. }
            | Command::Demo {
                action: DemoAction::Init { .. }
            }
    ) {
        cwd.to_path_buf()
    } else {
        Workspace::discover(cwd)?.root().to_path_buf()
    };
    Ok(CommandLock { root, mode })
}

fn lock_mode(command: &Command) -> LockMode {
    match command {
        Command::CloudflareScaffold { .. } => LockMode::Exclusive,
        Command::CloudflareConsume { .. } | Command::CloudflareWatch => LockMode::Exclusive,
        Command::Api { .. } => LockMode::Shared,
        Command::Demo { action } => match action {
            DemoAction::Init { .. } => LockMode::Exclusive,
            DemoAction::Serve { .. } => LockMode::Shared,
        },
        Command::Init { .. } => LockMode::Exclusive,
        Command::Pull { dry_run, .. } => {
            if *dry_run {
                LockMode::Shared
            } else {
                LockMode::Exclusive
            }
        }
        Command::Purge { confirm, .. } => {
            if confirm.is_some() {
                LockMode::Exclusive
            } else {
                LockMode::Shared
            }
        }
        Command::Message { action } => match action {
            MessageAction::Show { .. } | MessageAction::List { .. } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Config { action } => match action {
            ConfigAction::Show | ConfigAction::Get { .. } => LockMode::Shared,
            ConfigAction::Set { .. } | ConfigAction::Add { .. } | ConfigAction::Remove { .. } => {
                LockMode::Exclusive
            }
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
            Some(PushAction::Recover {
                confirm: Some(_), ..
            }) => LockMode::Exclusive,
            Some(PushAction::Recover { .. }) => LockMode::Shared,
            None if confirm.is_some() => LockMode::Exclusive,
            None => LockMode::Shared,
        },
        Command::Status => LockMode::Shared,
        Command::Doctor { action } => match action {
            Some(DoctorAction::Repair) => LockMode::Exclusive,
            _ => LockMode::Shared,
        },
        Command::Skill { .. } => LockMode::Shared,
        Command::Triage { action } => match action {
            TriageAction::List { .. } | TriageAction::Review { .. } => LockMode::Shared,
            TriageAction::Suggestion { .. } => LockMode::Exclusive,
        },
        Command::Case { action } => match action {
            CaseCommand::Create(_) => LockMode::Exclusive,
            CaseCommand::Show { .. }
            | CaseCommand::Review { .. }
            | CaseCommand::List { .. }
            | CaseCommand::Notes {
                action: CaseNotesAction::Show { .. },
            }
            | CaseCommand::Draft {
                action: CaseDraftAction::List { .. },
            } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Notification { action } => match action {
            NotificationCommand::List
            | NotificationCommand::Show { .. }
            | NotificationCommand::Notes {
                action: NotificationNotesAction::Show { .. },
            } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Render { action } => match action {
            RenderAction::Refresh => LockMode::Exclusive,
            RenderAction::Templates { .. } => LockMode::Exclusive,
        },
        Command::Contact { action } => match action {
            ContactCommand::List { .. }
            | ContactCommand::Show { .. }
            | ContactCommand::Notes {
                action: ContactNotesAction::Show { .. },
            } => LockMode::Shared,
            _ => LockMode::Exclusive,
        },
        Command::Log { .. } => LockMode::Shared,
    }
}

pub(super) fn workspace_progress_command(command: &Command) -> Option<&'static str> {
    match command {
        Command::Pull { dry_run: false, .. } => Some("pull"),
        Command::Render {
            action: RenderAction::Refresh,
        } => Some("render"),
        Command::Push {
            dry_run,
            confirm,
            action: None,
        } if confirm.is_some() && !*dry_run => Some("push"),
        _ => None,
    }
}
