use super::{
    ArchiveAction, CaseCommand, ConfigAction, DoctorAction, LogAction, MessageAction, PurgeAction,
    PushAction, RemoteAction, RenderAction, SkillAction, TriageAction,
};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "afmail", disable_version_flag = true, verbatim_doc_comment)]
#[doc = r#"Agent-First Mail: local-first email case workspace for agents.

### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF` and
  `afmail archive case show REF`.
- stdout carries structured Agent-First Data events; stderr is not a protocol channel.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus durable state/remote sidecars
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/workspace.progress.json latest push/pull runtime snapshot
triage/message_*.md      active unprocessed message views
spam/*.md                generated spam review views
trash/*.md               generated trash review views
deleted/*.md             generated remote-deleted review views
cases/<group>/<case_uid>-<name>/case.md generated case entry view
cases/<group>/<case_uid>-<name>/data/ canonical case state
cases/<group>/<case_uid>-<name>/views/ generated case detail views
archive/cases/<case_uid>-<name>/ archived case workspaces
archive/notifications/<archive_uid>-<name>/archive.md generated archive entry view
archive/notifications/<archive_uid>-<name>/data/ canonical archive state
archive/notifications/<archive_uid>-<name>/views/ generated archive detail views
```

### Examples

```text
afmail init
afmail skill status
afmail skill install
afmail status
afmail doctor
afmail pull
afmail pull sent archive
afmail remote folders
afmail case create --name 应用反馈-肥料登记 --message message_inbox_607146690_21 --group open --reason "new feedback thread"
afmail case show c20260603001
afmail case add c20260603001 message_inbox_607146690_22 --reason "follow-up belongs to same feedback case"
afmail archive message create --name 服务通知 --message message_inbox_607146690_23 --summary "billing notification" --reason "billing notification"
afmail archive message show a20260603001
afmail archive message restore a20260603001 message_inbox_607146690_23 --reason "needs triage again"
afmail message spam message_inbox_607146690_23 --reason "phishing attempt"
afmail message trash message_inbox_607146690_24 --reason "duplicate no longer needed"
afmail render refresh
afmail doctor repair --confirm
afmail case move c20260603001 waiting
afmail case archive c20260603001 --reason "feedback handled"
afmail archive case restore c20260603001 --group open --reason "customer replied"
afmail case tag c20260603001 legal --reason "legal review needed"
afmail case reply c20260603001 message_inbox_607146690_22
afmail case draft attach c20260603001 reply-message_inbox_607146690_22.md ./screenshot.png
afmail case draft validate c20260603001 reply-message_inbox_607146690_22.md
afmail case compose c20260603001 reply-message_inbox_607146690_22.md
afmail case draft remove c20260603001 reply-message_inbox_607146690_22.md --reason "mistaken draft"
afmail push drafts-send --dry-run
afmail push drafts-send
afmail push archive
afmail push spam
afmail push trash
afmail push list
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

    /// Print structured version event.
    #[arg(short = 'V', long, global = true)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize the current directory as an afmail workspace.
    Init,
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
    /// Manage the Agent-First Mail skill for Codex, Claude Code, and opencode.
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
    /// Inspect and manage archived cases and direct-message archive categories.
    Archive {
        #[command(subcommand)]
        action: ArchiveAction,
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
}
