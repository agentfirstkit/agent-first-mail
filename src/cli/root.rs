use super::{
    ApiAction, CaseCommand, ConfigAction, ContactCommand, DoctorAction, LogAction, MessageAction,
    NotificationCommand, PurgeAction, PushAction, RemoteAction, RenderAction, SkillAction,
    TriageAction,
};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = env!("DISPLAY_NAME"),
    bin_name = "afmail",
    disable_version_flag = true,
    disable_help_subcommand = true,
    verbatim_doc_comment,
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
#[doc = r#"### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF`.
- `--output-to split` sends results to stdout and errors/logs/progress to stderr.
- `--output-to stdout|stderr` selects one ordered Agent-First Data event stream.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus canonical message metadata
.afmail/workspace.json   immutable workspace identity used by plan digests
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/cache/progress.json latest push/pull runtime snapshot
templates/               user-editable Markdown and Review templates
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
afmail api export --directory openapi --force
afmail skill status
afmail skill install --agent codex --scope workspace
afmail status
afmail doctor
afmail pull
afmail pull sent archive
afmail remote folders
afmail case create --name 应用反馈-肥料登记 --group support --message message_inbox_607146690_21 --reason "new feedback thread"
afmail case show c20260603001
afmail case add c20260603001 message_inbox_607146690_22 message_inbox_607146690_23 --reason "follow-up belongs to same feedback case"
afmail notification create --name 服务通知 --message message_inbox_607146690_23 --summary "billing notification" --reason "billing notification"
afmail notification add n20260603001 message_inbox_607146690_24 message_inbox_607146690_25 --summary "billing notification" --reason "billing notification"
afmail notification show n20260603001
afmail notification remove n20260603001 message_inbox_607146690_23 --reason "does not belong in this notification"
afmail message spam message_inbox_607146690_23 message_inbox_607146690_24 --reason "phishing attempt"
afmail message trash message_inbox_607146690_25 message_inbox_607146690_26 --reason "duplicate no longer needed"
afmail render refresh
afmail doctor repair --confirm
afmail case move c20260603001 --group waiting --reason "better category"
afmail case archive c20260603001 --reason "feedback handled"
afmail case unarchive c20260603001 --group support --reason "customer replied"
afmail case tag c20260603001 legal --reason "legal review needed"
afmail contact create --name "Zhang San" --email zhang@example.com --org Acme --role Buyer --reason "new contact"
afmail contact list --group people
afmail contact email add p20260603001 zhang-alt@example.com --reason "alternate email"
afmail contact rename p20260603001 --name "Zhang Sanfeng" --reason "better display name"
afmail contact extract --from-triage
afmail contact delete p20260603001 --reason "duplicate contact"
afmail case draft reply c20260603001 message_inbox_607146690_22
afmail case draft attach c20260603001 reply-message_inbox_607146690_22.md ./screenshot.png
afmail case draft change c20260603001 reply-message_inbox_607146690_22.md --body "Thanks, I will check this."
afmail case draft validate c20260603001 reply-message_inbox_607146690_22.md
afmail case draft list c20260603001
afmail case draft send c20260603001 reply-message_inbox_607146690_22.md
afmail case draft remove c20260603001 reply-message_inbox_607146690_22.md --reason "mistaken draft"
afmail push list
afmail push
afmail push --confirm push_plan_<REVIEWED_PLAN_ID>
afmail purge
afmail purge spam --older-than-days 30
afmail purge deleted
afmail log list --limit 20
afmail triage list --limit 10 --details
afmail message show message_inbox_607146690_21
afmail message list --sender alice@example.com --subject-contains invoice --since 2026-05-21 --in all
afmail message attachment fetch message_inbox_607146690_21 2
```

### Exit Codes

- `0`: command completed successfully
- `1`: command failed, including an aggregate command where no item completed successfully
- `2`: invalid CLI arguments
- `3`: command completed partially; inspect the canonical error for successful and failed items
- `4`: a canonical output event was not fully written

Only completely successful commands emit `kind: result`; their payload has no generic `code`, `ok`,
or `outcome` field. Typed review results retain `outcome` as a business lifecycle field. Any item
failure emits `kind: error`. Partial failures use `error.code: partial`,
and the complete item list is carried in `error.items`; a failed item has an `error` child object.
Exit code `4` means the command could not deliver a complete protocol event;
inspect the selected output sink for the fallback diagnostic.
"#]
pub struct Cli {
    /// Output format: json (default), yaml, plain.
    #[arg(long, default_value = "json", global = true)]
    pub output: String,

    /// Where protocol events go: split, stdout, or stderr.
    ///
    /// Defaults to split for a finite command, and to stdout for a review or
    /// serve command, whose ordered event stream must stay on one stream.
    #[arg(
        long = "output-to",
        value_parser = ["split", "stdout", "stderr"],
        global = true
    )]
    pub output_to: Option<String>,

    /// Log categories (comma-separated): startup, request, progress, retry.
    #[arg(long, value_delimiter = ',', global = true)]
    pub log: Vec<String>,

    /// Enable all log categories. Startup argv redacts values and non-command positional tokens.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Redirect stdout bytes to this file.
    #[arg(long = "stdout-file", value_name = "PATH", global = true)]
    pub stdout_file: Option<String>,

    /// Redirect stderr bytes to this file.
    #[arg(long = "stderr-file", value_name = "PATH", global = true)]
    pub stderr_file: Option<String>,

    /// Operate on this existing afmail workspace instead of discovering from cwd.
    #[arg(long, value_name = "PATH", global = true)]
    pub workspace: Option<String>,

    /// Idempotency key for supported non-idempotent local mutations.
    #[arg(long = "request-id", value_name = "ID", global = true)]
    pub request_id: Option<String>,

    /// Print structured version event.
    #[arg(long, global = true)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Serve or export the afmail OpenAPI 3.2 contract and JSON Schemas.
    Api {
        #[command(subcommand)]
        action: ApiAction,
    },
    /// Create and run a complete local mailbox demo.
    Demo {
        #[command(subcommand)]
        action: crate::cli::DemoAction,
    },
    /// Initialize the current directory or a child path as an afmail workspace.
    Init {
        /// Optional workspace path to initialize under the current directory.
        path: Option<String>,
    },
    /// Read configured IMAP mailbox ids into local message files without changing remote mail.
    Pull {
        /// Configured mailbox ids to pull. With none, pulls actions.pull.default_mailbox_ids.
        ids: Vec<String>,
        /// Estimate remote candidates and budgets without downloading or writing locally.
        #[arg(long)]
        dry_run: bool,
        /// Limit the remote candidate range to messages from the last DAYS days.
        #[arg(long, value_name = "DAYS", conflicts_with = "all_history")]
        since_days: Option<u32>,
        /// Maximum number of new messages stored across all selected mailboxes.
        #[arg(long, value_name = "COUNT")]
        max_new_messages: Option<usize>,
        /// Maximum new raw message bytes stored across all selected mailboxes.
        #[arg(long, value_name = "BYTES")]
        max_total_bytes: Option<u64>,
        /// Explicitly include all remote history instead of the default initial window.
        #[arg(long, conflicts_with = "since_days")]
        all_history: bool,
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
        /// Apply exactly the reviewed plan returned by push preview.
        #[arg(long, value_name = "PLAN_ID")]
        confirm: Option<String>,
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
        /// Permanently delete exactly the reviewed purge plan.
        #[arg(long, value_name = "PLAN_ID", global = true)]
        confirm: Option<String>,
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
}

/// Whether this command produces more than one caller-needed output over time,
/// which makes it an AFDATA event stream rather than a finite command.
///
/// A `review` reports the URL a human must open, waits an unbounded time, then
/// reports that human's decision. `api serve` and `demo serve` report the
/// address to connect to, then keep serving. In each case both outputs are data
/// the caller acts on, so splitting the stream would leave the first one on
/// stderr while only an acknowledgement reached stdout.
pub fn is_event_stream_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Triage {
            action: TriageAction::Review { .. },
        } | Command::Case {
            action: CaseCommand::Review { .. }
                | CaseCommand::Draft {
                    action: crate::cli::CaseDraftAction::Review { .. },
                },
        } | Command::Notification {
            action: NotificationCommand::Review { .. },
        } | Command::Api {
            action: ApiAction::Serve { .. },
        } | Command::Demo {
            action: crate::cli::DemoAction::Serve { .. },
        }
    )
}
