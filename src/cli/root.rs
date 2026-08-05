use super::{
    ApiAction, CaseCommand, ConfigAction, ContactCommand, DemoAction, DoctorAction, LogAction,
    MessageAction, NotificationCommand, PurgeAction, PushAction, RemoteAction, RenderAction,
    SkillAction, TriageAction,
};

/// One resolved afmail command.
///
/// The registry in [`super::spec`] decides which invocations are legal; this
/// enum is what a legal one resolved to. The HTTP API builds the same values
/// directly, so it stays independent of argv.
#[derive(Debug)]
pub enum Command {
    /// Serve or export the afmail OpenAPI 3.2 contract and JSON Schemas.
    Api { action: ApiAction },
    /// Create and run a complete local mailbox demo.
    Demo { action: DemoAction },
    /// Initialize the current directory or a child path as an afmail workspace.
    Init { path: Option<String> },
    /// Read configured IMAP mailbox ids into local message files without changing remote mail.
    Pull {
        ids: Vec<String>,
        dry_run: bool,
        since_days: Option<u32>,
        max_new_messages: Option<usize>,
        max_total_bytes: Option<u64>,
        all_history: bool,
    },
    /// Read or update local afmail configuration.
    Config { action: ConfigAction },
    /// Inspect remote IMAP state for configuring mailboxes.
    Remote { action: RemoteAction },
    /// Push queued local work or manage the local push queue.
    Push {
        dry_run: bool,
        confirm: Option<String>,
        action: Option<PushAction>,
    },
    /// Report workspace health, counts, and latest pull/push progress.
    Status,
    /// Check afmail workspace consistency without inspecting Git.
    Doctor { action: Option<DoctorAction> },
    /// Permanently delete old local spam, trash, and remote-deleted records.
    Purge {
        older_than_days: Option<u64>,
        confirm: Option<String>,
        action: Option<PurgeAction>,
    },
    /// Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes.
    Skill { action: SkillAction },
    /// Inspect the triage queue.
    Triage { action: TriageAction },
    /// Operate on any local message id.
    Message { action: MessageAction },
    /// Create a case or operate on an existing case ref.
    Case { action: CaseCommand },
    /// Create a contact or operate on an existing contact ref.
    Contact { action: ContactCommand },
    /// Inspect and manage notification collections.
    Notification { action: NotificationCommand },
    /// Rebuild generated read views from local workspace state.
    Render { action: RenderAction },
    /// Inspect the workspace audit log.
    Log { action: LogAction },
}
