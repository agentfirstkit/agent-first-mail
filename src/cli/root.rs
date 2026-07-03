#[cfg(feature = "ui")]
use super::UiCommand;
use super::{
    CaseCommand, ConfigAction, ContactCommand, DoctorAction, LogAction, MessageAction,
    NotificationCommand, PurgeAction, PushAction, RemoteAction, RenderAction, SkillAction,
    TriageAction,
};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = env!("DISPLAY_NAME"),
    bin_name = "afmail",
    disable_version_flag = true,
    verbatim_doc_comment,
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
#[doc = r#"### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF`.
- stdout carries structured Agent-First Data events; stderr is not a protocol channel.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus durable remote sidecars
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/workspace.progress.json latest push/pull runtime snapshot
templates/               user-editable generated Markdown templates
triage/message_*.md      active unprocessed message views
spam/*.md                generated spam review views
trash/*.md               generated trash review views
deleted/*.md             generated remote-deleted review views
cases/<group>/<case_uid>-<name>/case.md generated case entry view
cases/<group>/<case_uid>-<name>/data/ canonical case state
cases/<group>/<case_uid>-<name>/views/ generated case detail views
archived-cases/<group>/<case_uid>-<name>/ archived case workspaces; <group> is a case group
contacts/<group>/<contact_uid>-<name>.md canonical contact cards
notifications/<notification_uid>-<name>/notification.md generated notification entry view
notifications/<notification_uid>-<name>/data/ canonical notification state
notifications/<notification_uid>-<name>/views/ generated notification detail views
```

### Examples

```text
afmail init
afmail init email-a
afmail skill status
afmail skill install --agent codex --scope workspace
afmail status
afmail doctor
afmail pull
afmail pull sent archive
afmail remote folders
afmail case create --name 应用反馈-肥料登记 --group support --message message_inbox_607146690_21 --reason "new feedback thread"
afmail case show c20260603001
afmail case add c20260603001 message_inbox_607146690_22 --reason "follow-up belongs to same feedback case"
afmail notification create --name 服务通知 --message message_inbox_607146690_23 --summary "billing notification" --reason "billing notification"
afmail notification show n20260603001
afmail notification remove n20260603001 message_inbox_607146690_23 --reason "does not belong in this notification"
afmail message spam message_inbox_607146690_23 --reason "phishing attempt"
afmail message trash message_inbox_607146690_24 --reason "duplicate no longer needed"
afmail render refresh
afmail doctor repair --confirm
afmail case move c20260603001 waiting --reason "better category"
afmail case archive c20260603001 --reason "feedback handled"
afmail case unarchive c20260603001 --group support --reason "customer replied"
afmail case tag c20260603001 legal --reason "legal review needed"
afmail contact create --name "Zhang San" --email zhang@example.com --org Acme --role Buyer
afmail contact list --group people
afmail contact email add p20260603001 zhang-alt@example.com
afmail contact rename p20260603001 --name "Zhang Sanfeng"
afmail contact extract --from-triage
afmail contact delete p20260603001 --reason "duplicate contact"
afmail case draft reply c20260603001 message_inbox_607146690_22
afmail case draft attach c20260603001 reply-message_inbox_607146690_22.md ./screenshot.png
afmail case draft change c20260603001 reply-message_inbox_607146690_22.md --body "Thanks, I will check this."
afmail case draft validate c20260603001 reply-message_inbox_607146690_22.md
afmail case draft send c20260603001 reply-message_inbox_607146690_22.md
afmail case draft remove c20260603001 reply-message_inbox_607146690_22.md --reason "mistaken draft"
afmail push list
afmail push
afmail push --confirm
afmail purge
afmail purge spam --older-than-days 30
afmail purge deleted
afmail log list --limit 20
afmail message show message_inbox_607146690_21
afmail message attachment fetch message_inbox_607146690_21 2
```

### Exit Codes

- `0`: command completed successfully
- `1`: runtime/store/protocol error
- `2`: invalid CLI arguments
"#]
pub struct Cli {
    /// Output format: json (default), yaml, plain.
    #[arg(long, default_value = "json", global = true)]
    pub output: String,

    /// Log categories (comma-separated): startup, request, progress, retry.
    #[arg(long, value_delimiter = ',', global = true)]
    pub log: Vec<String>,

    /// Enable all log categories.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Redirect stdout bytes to this file.
    #[arg(long = "stdout-file", value_name = "PATH", global = true)]
    pub stdout_file: Option<String>,

    /// Redirect stderr bytes to this file.
    #[arg(long = "stderr-file", value_name = "PATH", global = true)]
    pub stderr_file: Option<String>,

    /// Print structured version event.
    #[arg(short = 'V', long, global = true)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize the current directory or a child path as an afmail workspace.
    Init {
        /// Optional workspace path to initialize under the current directory.
        path: Option<String>,
    },
    /// Read configured IMAP mailbox ids into local message files without changing remote mail.
    Pull {
        /// Configured mailbox ids to pull. With none, pulls actions.pull.default_mailbox_ids.
        ids: Vec<String>,
    },
    /// Read or update local afmail configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Inspect remote IMAP state for configuring mailboxes.
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
    },
    /// Push queued local work or manage the local push queue.
    Push {
        /// Show planned push actions without IMAP/SMTP writes. This is the default.
        #[arg(long)]
        dry_run: bool,
        /// Apply queued IMAP/SMTP effects.
        #[arg(long)]
        confirm: bool,
        #[command(subcommand)]
        action: Option<PushAction>,
    },
    /// Report workspace health, counts, and latest pull/push progress.
    Status,
    /// Check afmail workspace consistency without inspecting Git.
    Doctor {
        #[command(subcommand)]
        action: Option<DoctorAction>,
    },
    /// Permanently delete old local spam, trash, and remote-deleted records.
    Purge {
        /// Only purge messages in a discard state at least this many days ago. Defaults to 30.
        #[arg(long = "older-than-days", value_name = "DAYS")]
        older_than_days: Option<u64>,
        #[command(subcommand)]
        action: Option<PurgeAction>,
    },
    /// Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Inspect the triage queue.
    Triage {
        #[command(subcommand)]
        action: TriageAction,
    },
    /// Operate on any local message id.
    Message {
        #[command(subcommand)]
        action: MessageAction,
    },
    /// Create a case or operate on an existing case ref.
    Case {
        #[command(subcommand)]
        action: CaseCommand,
    },
    /// Create a contact or operate on an existing contact ref.
    Contact {
        #[command(subcommand)]
        action: ContactCommand,
    },
    /// Inspect and manage notification collections.
    Notification {
        #[command(subcommand)]
        action: NotificationCommand,
    },
    /// Rebuild generated read views from local workspace state.
    Render {
        #[command(subcommand)]
        action: RenderAction,
    },
    /// Inspect the workspace audit log.
    Log {
        #[command(subcommand)]
        action: LogAction,
    },
    /// Emit or open an Agent-First UI operation surface for this workspace.
    #[cfg(feature = "ui")]
    Ui(UiCommand),
}
