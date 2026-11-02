//! The afmail command registry.
//!
//! One closed `cli-spec-v1` document is the single source for argv parsing,
//! typed invocation values, which argument combinations are legal, each
//! command's output contract, `--help`, and `docs/cli.md`. Nothing in this file
//! runs a command: [`super::build`] turns a resolved invocation into a
//! [`super::Command`].
//!
//! Two rules shape everything below.
//!
//! - There is no argument inheritance. `--workspace`, `--log` and
//!   `--request-id` are afmail's own, so every command that accepts one
//!   declares it. [`Leaf`] does that once per command instead of once per
//!   shape.
//! - A constraint that used to be a runtime check is a shape here. Mutually
//!   exclusive flags, "this option requires that one", and "this command
//!   refuses to run without --confirm" are all expressed as disjoint
//!   combinations, so the parser rejects the illegal mix before anything runs
//!   and `--help` shows every legal mix at once.

use agent_first_data::{
    ArgSpec, BuiltCliSpec, CliSpec, CliSpecError, Combination, CommandSpec, OutputSpec, SourceSet,
    build_afdata_cli,
};

/// Diagnostic categories `--log` accepts.
const LOG_CATEGORIES: [&str; 4] = ["startup", "request", "progress", "retry"];

/// Named skill agents, and the fan-out value that is not one of them.
const SKILL_AGENTS: [&str; 4] = ["codex", "claude-code", "opencode", "hermes"];
const EVERY_SKILL_AGENT: &str = "all";

/// A command that produces at most one terminal event.
fn finite() -> OutputSpec {
    OutputSpec::protocol_finite(
        ["json", "yaml", "plain"],
        ["split", "stdout", "stderr"],
        "json",
        "split",
    )
    .file_sinks(["stdout", "stderr"])
}

/// A command whose caller-needed output arrives over time.
///
/// A review reports the URL a human must open, waits an unbounded time, then
/// reports that human's decision; `api serve` and `demo serve` report the
/// address to connect to, then keep serving. Both outputs are data the caller
/// acts on, so `split` is absent from the contract: it would strand the first
/// one on the diagnostic stream while only an acknowledgement reached stdout.
fn streaming() -> OutputSpec {
    OutputSpec::protocol_stream(
        ["json", "yaml", "plain"],
        ["stdout", "stderr"],
        "json",
        "stdout",
    )
    .file_sinks(["stdout", "stderr"])
}

/// One command, its arguments, and the shapes they may legally form.
///
/// `req`/`opt` describe arguments every shape of the command shares; a
/// multi-shape command adds only what tells its shapes apart.
struct Leaf {
    command: CommandSpec,
    output: OutputSpec,
    shared_required: Vec<String>,
    shared_optional: Vec<String>,
}

impl Leaf {
    /// A command that runs against an existing workspace.
    fn new<I, S>(path: I, about: &str) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::bare(path, about)
            .opt(ArgSpec::option("--workspace", "PATH").about(
                "Existing afmail workspace to operate on instead of discovering one from the \
                     current directory",
            ))
            .log()
    }

    /// A command that creates a workspace, so it has none to select.
    fn creating<I, S>(path: I, about: &str) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::bare(path, about).log()
    }

    fn bare<I, S>(path: I, about: &str) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            command: CommandSpec::new(path).about(about),
            output: finite(),
            shared_required: Vec::new(),
            shared_optional: Vec::new(),
        }
    }

    fn log(self) -> Self {
        self.opt(
            ArgSpec::option_enum("--log", LOG_CATEGORIES)
                .value_name("CATEGORY")
                .repeatable()
                .about("Diagnostic category to emit. Repeat the flag for more than one."),
        )
    }

    /// Accept an idempotency key, which only a non-idempotent local mutation can.
    fn idempotent(self) -> Self {
        self.opt(ArgSpec::option("--request-id", "ID").about(
            "Idempotency key; replaying it returns the first result instead of repeating the \
             mutation",
        ))
    }

    fn stream(mut self) -> Self {
        self.output = streaming();
        self
    }

    /// Declare an argument every shape requires.
    fn req(mut self, argument: ArgSpec) -> Self {
        self.shared_required.push(argument.argument_id.clone());
        self.command = self.command.arg(argument);
        self
    }

    /// Declare an argument every shape accepts.
    fn opt(mut self, argument: ArgSpec) -> Self {
        self.shared_optional.push(argument.argument_id.clone());
        self.command = self.command.arg(argument);
        self
    }

    /// Declare an argument only some shapes accept.
    fn arg(mut self, argument: ArgSpec) -> Self {
        self.command = self.command.arg(argument);
        self
    }

    /// The command has exactly one legal shape, so its `about` already
    /// describes it and the shape carries no second description.
    fn one(self, action: &str) -> CommandSpec {
        let shape = Combination::new(action).action(action);
        self.shapes([shape])
    }

    fn shapes<I>(self, shapes: I) -> CommandSpec
    where
        I: IntoIterator<Item = Combination>,
    {
        let mut command = self.command;
        for shape in shapes {
            command = command.combination(
                shape
                    .required(self.shared_required.clone())
                    .optional(self.shared_optional.clone())
                    .output(self.output.clone()),
            );
        }
        command
    }
}

fn shape(id: &str, action: &str, about: &str) -> Combination {
    Combination::new(id).action(action).about(about)
}

/// The sources a credential argument accepts.
///
/// No stream sources: `afmail` reads message bodies and patches on stdin, and
/// a prompt would block a server an agent started.
pub(crate) fn secret_sources() -> SourceSet {
    SourceSet::config()
}

fn reason_arg(about: &str) -> ArgSpec {
    ArgSpec::option("--reason", "TEXT").about(about)
}

fn case_ref_arg() -> ArgSpec {
    ArgSpec::positional("case_ref", 0, "CASE_REF")
        .about("Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix")
}

fn contact_ref_arg() -> ArgSpec {
    ArgSpec::positional("contact_ref", 0, "CONTACT_REF")
        .about("Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix")
}

fn notification_ref_arg() -> ArgSpec {
    ArgSpec::positional("notification_ref", 0, "NOTIFICATION_REF")
        .about("Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix")
}

fn draft_name_arg() -> ArgSpec {
    ArgSpec::positional("draft_name", 1, "DRAFT_NAME")
        .about("Draft markdown file under the case drafts directory")
}

/// Which deliveries a review offers, declared once: this flag and the plan the
/// runner builds come from the same value, and the rule about never giving it
/// a default is AFUI's to enforce now rather than each Provider's to remember.
pub(crate) const REVIEW_DELIVERY: agent_first_ui::cli::UiDeliveryOffer =
    agent_first_ui::cli::UiDeliveryOffer::WithLink;

fn mode_arg() -> ArgSpec {
    REVIEW_DELIVERY.arg("--mode")
}

fn confidence_arg() -> ArgSpec {
    ArgSpec::option_i64("--confidence-percent", "PERCENT")
        .about("Optional confidence from 0 through 100")
}

fn suggestion_action_arg<I, S>(values: I) -> ArgSpec
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    ArgSpec::option_enum("--action", values)
        .value_name("ACTION")
        .about("The complete action being suggested; it decides which other arguments apply")
}

fn older_than_days_arg(about: &str) -> ArgSpec {
    ArgSpec::option_i64("--older-than-days", "DAYS").about(about)
}

fn purge_confirm_arg() -> ArgSpec {
    ArgSpec::option("--confirm", "PLAN_ID")
        .about("Permanently delete exactly the reviewed purge plan")
}

/// The whole CLI.
pub fn cli_spec() -> Result<BuiltCliSpec, CliSpecError> {
    let mut spec = CliSpec::new("afmail", env!("CARGO_PKG_VERSION"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .display_name(env!("DISPLAY_NAME"))
        .lifecycle_output(finite())
        // Declared so `--docs` documents what the binary actually returns.
        // AFDATA supplies 0/1/2/4; the published
        // reference would omit 3 — the code that decides whether a confirmed
        // push may be repeated — and disagree with the skill file that has it.
        .exit_code(
            3,
            "Some items in a batch succeeded and some failed. The remote effects that landed \
             stay landed, so re-running the same call is not safe; read `failures` and re-queue \
             from there.",
        )
        .command(CommandSpec::root());
    for command in commands() {
        spec = spec.command(command);
    }
    if let Some(build) = Some(env!("GIT_SHA")).filter(|sha| *sha != "unknown") {
        spec = spec.build_id(build);
    }
    build_afdata_cli(spec)
}

fn commands() -> Vec<CommandSpec> {
    let mut commands = Vec::new();
    commands.extend(top_level_commands());
    commands.extend(config_commands());
    commands.extend(remote_commands());
    commands.extend(skill_commands());
    commands.extend(triage_commands());
    commands.extend(message_commands());
    commands.extend(case_commands());
    commands.extend(case_draft_commands());
    commands.extend(contact_commands());
    commands.extend(notification_commands());
    commands.extend(render_commands());
    commands.extend(log_commands());
    commands.extend(api_commands());
    commands.extend(demo_commands());
    commands
}

fn top_level_commands() -> Vec<CommandSpec> {
    vec![
        Leaf::creating(
            ["init"],
            "Initialize the current directory or a child path as an afmail workspace.",
        )
        .opt(
            ArgSpec::positional("path", 0, "PATH")
                .about("Workspace path to initialize under the current directory"),
        )
        .one("init"),
        pull_command(),
        CommandSpec::new(["cloudflare"]).about("Configure Cloudflare mail ingestion."),
        Leaf::creating(["cloudflare", "scaffold"], "Write an Email Worker project into an absent or empty directory.")
            .req(ArgSpec::positional("path", 0, "DIR").about("Worker project directory"))
            .one("cloudflare_scaffold"),
        Leaf::new(["cloudflare", "consume"], "Consume one Queue batch, import mail, and dispatch durable agent work.")
            .opt(ArgSpec::flag("--reconcile").about("Also scan the R2 pending prefix to recover missing or expired notifications"))
            .one("cloudflare_consume"),
        Leaf::new(["cloudflare", "watch"], "Watch Queue events and periodically reconcile R2, waking an agent only for durable new work.")
            .stream().one("cloudflare_watch"),
        Leaf::new(
            ["status"],
            "Report workspace health, counts, and latest pull/push progress.",
        )
        .one("status"),
        push_command(),
        Leaf::new(
            ["push", "list"],
            "Preview the complete resolved push plan without writing.",
        )
        .one("push_list"),
        Leaf::new(
            ["push", "recover"],
            "Check the evidence for an uncertain Drafts APPEND or send step; optionally repair or settle local state.",
        )
        .req(ArgSpec::positional("push_id", 0, "PUSH_ID").about("Queued push item to inspect"))
        .arg(ArgSpec::option("--confirm", "PLAN_ID").about(
            "Apply exactly the recovery plan returned by the check; local queue state only, never mail",
        ))
        .arg(ArgSpec::flag("--sent").about(
            "Assert an uncertain send was delivered; later steps run on the next confirmed push",
        ))
        .arg(ArgSpec::flag("--not-sent").about(
            "Assert an uncertain send was not delivered; it is sent only by a newly confirmed push",
        ))
        .shapes([
            shape("push_recover_check", "push_recover", "Read-only evidence check returning a recovery plan_id"),
            shape("push_recover_confirm", "push_recover", "Recheck an uncertain Drafts APPEND and repair local queue state")
                .required(["confirm"]),
            shape("push_recover_sent", "push_recover", "Settle an uncertain send as sent")
                .required(["confirm", "sent"]),
            shape("push_recover_not_sent", "push_recover", "Settle an uncertain send as not sent")
                .required(["confirm", "not_sent"]),
        ]),
        Leaf::new(
            ["doctor"],
            "Check afmail workspace consistency without inspecting Git.",
        )
        .one("doctor"),
        Leaf::new(
            ["doctor", "repair"],
            "Repair only unambiguous afmail-generated state.",
        )
        .req(ArgSpec::flag("--confirm").about(
            "Apply the repair actions. Repair never runs without it, so it is part of the only \
             registered shape.",
        ))
        .one("doctor_repair"),
        purge_command(),
        purge_target_command(
            "spam",
            "Permanently delete old local spam records.",
            "Only purge messages marked spam at least this many days ago. Defaults to 30.",
        ),
        purge_target_command(
            "trash",
            "Permanently delete old local trash records.",
            "Only purge messages marked trash at least this many days ago. Defaults to 30.",
        ),
        purge_target_command(
            "deleted",
            "Permanently delete old local records whose remote message disappeared.",
            "Only purge messages marked remote-deleted at least this many days ago. Defaults to \
             30.",
        ),
    ]
}

fn pull_command() -> CommandSpec {
    Leaf::new(
        ["pull"],
        "Import messages from configured IMAP, Cloudflare and file sources.",
    )
    .opt(
        ArgSpec::positional("ids", 0, "MAILBOX_ID")
            .repeatable()
            .about(
                "IMAP mailbox ids to pull. With none and no --cloudflare or --files, pulls \
                 every configured source.",
            ),
    )
    .arg(
        ArgSpec::flag("--cloudflare")
            .about("Pull Cloudflare R2 objects; with mailbox ids, also pull those IMAP mailboxes"),
    )
    .arg(ArgSpec::flag("--files").about("Pull native JSON from every configured file source"))
    .opt(
        ArgSpec::flag("--dry-run")
            .about("Estimate source candidates and budgets without downloading or writing locally"),
    )
    .opt(
        ArgSpec::option_i64("--max-new-messages", "COUNT")
            .about("Maximum number of new messages stored across all selected sources"),
    )
    .opt(
        ArgSpec::option_i64("--max-total-bytes", "BYTES")
            .about("Maximum new raw message bytes stored across all selected sources"),
    )
    .arg(
        ArgSpec::option_i64("--since-days", "DAYS")
            .about("Limit the remote candidate range to messages from the last DAYS days"),
    )
    .arg(
        ArgSpec::flag("--all-history")
            .about("Include all remote history instead of the default initial window"),
    )
    .shapes([
        shape(
            "pull-window",
            "pull",
            "Pull a bounded window: the default initial window, or --since-days",
        )
        .optional(["since_days"]),
        shape(
            "pull-all-history",
            "pull",
            "Pull every remote message; no window applies, so --since-days is absent here",
        )
        .required(["all_history"]),
        shape(
            "pull-cloudflare",
            "pull",
            "Select Cloudflare R2, optionally with file sources; time windows apply only to IMAP",
        )
        .required(["cloudflare"])
        .optional(["files"]),
        shape(
            "pull-files",
            "pull",
            "Select file sources; time windows apply only to IMAP",
        )
        .required(["files"]),
    ])
}

fn push_command() -> CommandSpec {
    Leaf::new(["push"], "Push queued local work to IMAP and SMTP.")
        .arg(ArgSpec::flag("--dry-run").about("Show planned push actions without IMAP/SMTP writes"))
        .arg(
            ArgSpec::option("--confirm", "PLAN_ID")
                .about("Apply exactly the reviewed plan returned by the push preview"),
        )
        .shapes([
            shape(
                "push-preview",
                "push",
                "Preview the plan; nothing is written, so no plan id is confirmed",
            )
            .optional(["dry_run"]),
            shape(
                "push-apply",
                "push",
                "Apply one reviewed plan; this shape writes, so --dry-run is absent",
            )
            .required(["confirm"]),
        ])
}

fn purge_command() -> CommandSpec {
    Leaf::new(
        ["purge"],
        "Permanently delete old local spam, trash, and remote-deleted records together.",
    )
    .opt(older_than_days_arg(
        "Only purge messages in a discard state at least this many days ago. Defaults to 30.",
    ))
    .opt(purge_confirm_arg())
    .one("purge")
}

fn purge_target_command(target: &str, about: &str, days_about: &str) -> CommandSpec {
    Leaf::new(["purge", target], about)
        .opt(older_than_days_arg(days_about))
        .opt(purge_confirm_arg())
        .one(&format!("purge_{target}"))
}

fn config_commands() -> Vec<CommandSpec> {
    let key = || ArgSpec::positional("key", 0, "KEY").about("Configuration key");
    vec![
        CommandSpec::new(["config"]).about("Read or update local afmail configuration."),
        Leaf::new(
            ["config", "show"],
            "Show effective config after applying built-in defaults.",
        )
        .one("config_show"),
        Leaf::new(["config", "get"], "Get one config key.")
            .req(key())
            .one("config_get"),
        Leaf::new(
            ["config", "set"],
            "Set one config key. Multiple values become an array for array keys.",
        )
        .req(key())
        .opt(
            ArgSpec::positional("values", 1, "VALUE")
                .repeatable()
                .about("Replacement value; repeat for an array key"),
        )
        .one("config_set"),
        Leaf::new(
            ["config", "add"],
            "Add an element to a keyed list such as `identities`.",
        )
        .req(key())
        .req(ArgSpec::positional("slug", 1, "SLUG").about("Stable slug identifying the element"))
        .opt(
            ArgSpec::positional("fields", 2, "KEY=VALUE")
                .repeatable()
                .about("Element field assignment"),
        )
        .one("config_add"),
        Leaf::new(
            ["config", "remove"],
            "Remove a keyed-list element by its slug.",
        )
        .req(key())
        .req(ArgSpec::positional("slug", 1, "SLUG").about("Slug of the element to remove"))
        .one("config_remove"),
    ]
}

fn remote_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["remote"]).about("Inspect remote IMAP state for configuring mailboxes."),
        Leaf::new(
            ["remote", "test"],
            "Test configured IMAP, Cloudflare R2, and SMTP access without sending mail.",
        )
        .one("remote_test"),
        Leaf::new(["remote", "folders"], "List IMAP folders/mailboxes.").one("remote_folders"),
    ]
}

fn skill_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["skill"]).about(
            "Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes.",
        ),
        skill_command(
            "status",
            "Show whether the Agent-First Mail skill is installed and valid.",
            false,
        ),
        skill_command("install", "Install the Agent-First Mail skill.", true),
        skill_command(
            "uninstall",
            "Remove an afmail-managed Agent-First Mail skill.",
            true,
        ),
    ]
}

/// One skill verb, as two shapes.
///
/// `--skills-dir` names a single directory, so it is meaningless when the verb
/// fans out across every agent. Registering that as two shapes rather than one
/// shape plus a runtime check means the illegal mix is rejected by the parser,
/// and both legal mixes are visible in one `--help`.
fn skill_command(verb: &str, about: &str, force: bool) -> CommandSpec {
    let mut leaf = Leaf::new(["skill", verb], about)
        .arg(
            ArgSpec::option_enum(
                "--agent",
                std::iter::once(EVERY_SKILL_AGENT).chain(SKILL_AGENTS),
            )
            .value_name("AGENT")
            .default(EVERY_SKILL_AGENT)
            .about("Agent to manage"),
        )
        .opt(
            ArgSpec::option_enum("--scope", ["personal", "workspace"])
                .value_name("SCOPE")
                .default("personal")
                .about("Skill scope"),
        )
        .arg(ArgSpec::option("--skills-dir", "DIR").about("Directory that contains skill folders"));
    if force {
        leaf =
            leaf.opt(ArgSpec::flag("--force").about(
                "Overwrite or remove an unmanaged Agent-First Mail skill at the target path",
            ));
    }
    leaf.shapes([
        shape(
            &format!("skill-{verb}-every-agent"),
            &format!("skill_{verb}"),
            "Target every agent that supports the scope",
        )
        .fixed("agent", EVERY_SKILL_AGENT),
        shape(
            &format!("skill-{verb}-one-agent"),
            &format!("skill_{verb}"),
            "Target one named agent; only this shape accepts --skills-dir",
        )
        .fixed_one_of("agent", SKILL_AGENTS)
        .optional(["skills_dir"]),
    ])
}

fn triage_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["triage"]).about("Inspect the triage queue."),
        CommandSpec::new(["triage", "suggestion"])
            .about("Set or clear the current typed Agent suggestion for a message."),
        Leaf::new(
            ["triage", "list"],
            "List compact untriaged message locators.",
        )
        .opt(
            ArgSpec::option_i64("--limit", "COUNT")
                .about("Maximum number of newest triage messages to return"),
        )
        .opt(
            ArgSpec::flag("--details")
                .about("Include message metadata, related message ids, and case refs"),
        )
        .one("triage_list"),
        triage_suggestion_set_command(),
        Leaf::new(
            ["triage", "suggestion", "clear"],
            "Clear exactly the current suggestion round.",
        )
        .idempotent()
        .req(
            ArgSpec::positional("message_id", 0, "MESSAGE_ID")
                .about("Message id whose current suggestion round should be cleared"),
        )
        .req(
            ArgSpec::option("--agent-suggestion-uid", "UID")
                .about("UID returned by the current suggestion set command"),
        )
        .one("triage_suggestion_clear"),
        Leaf::new(
            ["triage", "review"],
            "Open a review session for persisted triage suggestions.",
        )
        .stream()
        .opt(
            ArgSpec::positional("message_id", 0, "MESSAGE_ID")
                .repeatable()
                .about(
                    "Message ids to review. With none, reviews the current triage queue, newest \
                     first, up to a bound; the result says if any were left out.",
                ),
        )
        .opt(mode_arg())
        .one("triage_review"),
    ]
}

/// `triage suggestion set`, as one command with one shape per suggested action.
///
/// The suggested action used to be a nested subcommand after the message id,
/// which a command path cannot express — a path is a prefix of argv. Making it
/// a `--action` discriminator keeps the whole family in one `--help`, and each
/// shape names exactly the arguments its action needs.
fn triage_suggestion_set_command() -> CommandSpec {
    Leaf::new(
        ["triage", "suggestion", "set"],
        "Replace the current suggestion and clear any human review of the prior round.",
    )
    .idempotent()
    .req(ArgSpec::positional("message_id", 0, "MESSAGE_ID").about("Message id currently in triage"))
    .req(
        ArgSpec::option("--message-review-base-hash", "HASH")
            .about("Current hash returned by `afmail message show`"),
    )
    .req(reason_arg("Why this complete action is being suggested"))
    .opt(confidence_arg())
    .arg(suggestion_action_arg([
        "case-create",
        "case-add",
        "notification-create",
        "notification-add",
        "spam",
        "trash",
    ]))
    .arg(ArgSpec::option("--name", "NAME").about("Name for the suggested new case or notification"))
    .arg(ArgSpec::option("--group", "GROUP").about("Group for the suggested new case"))
    .arg(ArgSpec::option("--case-ref", "CASE_REF").about("Existing case to add the message to"))
    .arg(
        ArgSpec::option("--notification-ref", "NOTIFICATION_REF")
            .about("Existing notification collection to file the message into"),
    )
    .arg(ArgSpec::option("--summary", "TEXT").about("Summary for the notification entry"))
    .shapes([
        shape(
            "triage-suggestion-set-case-create",
            "triage_suggestion_set",
            "Suggest creating a new case from this message",
        )
        .fixed("action", "case-create")
        .required(["name", "group"]),
        shape(
            "triage-suggestion-set-case-add",
            "triage_suggestion_set",
            "Suggest adding this message to an existing case",
        )
        .fixed("action", "case-add")
        .required(["case_ref"]),
        shape(
            "triage-suggestion-set-notification-create",
            "triage_suggestion_set",
            "Suggest creating a new notification collection from this message",
        )
        .fixed("action", "notification-create")
        .required(["name", "summary"]),
        shape(
            "triage-suggestion-set-notification-add",
            "triage_suggestion_set",
            "Suggest filing this message into an existing notification collection",
        )
        .fixed("action", "notification-add")
        .required(["notification_ref", "summary"]),
        shape(
            "triage-suggestion-set-spam",
            "triage_suggestion_set",
            "Suggest marking this message as spam",
        )
        .fixed("action", "spam"),
        shape(
            "triage-suggestion-set-trash",
            "triage_suggestion_set",
            "Suggest trashing this message",
        )
        .fixed("action", "trash"),
    ])
}

fn message_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["message"]).about("Operate on any local message id."),
        CommandSpec::new(["message", "attachment"]).about("Fetch attachments for a local message."),
        Leaf::new(
            ["message", "show"],
            "Show the exact typed message document used by Markdown and Review templates.",
        )
        .req(
            ArgSpec::positional("message_id", 0, "MESSAGE_ID")
                .about("Message id, for example message_inbox_607146690_22"),
        )
        .one("message_show"),
        Leaf::new(
            ["message", "list"],
            "List messages by sender, subject, date, or workspace scope.",
        )
        .opt(
            ArgSpec::option("--sender", "TEXT")
                .about("Only include messages whose sender or from line contains this text"),
        )
        .opt(
            ArgSpec::option("--subject-contains", "TEXT")
                .about("Only include messages whose subject contains this text"),
        )
        .opt(
            ArgSpec::option("--since", "DATE")
                .about("Only include messages on or after this date or RFC3339 timestamp"),
        )
        .opt(
            ArgSpec::option_enum("--in", ["triage", "case", "notification", "all"])
                .value_name("SCOPE")
                .default("all")
                .about("Limit results to one workspace scope"),
        )
        .one("message_list"),
        message_disposition_command(
            "spam",
            "Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a \
             Junk move.",
            "Why this disposition is correct; required by default",
        ),
        message_disposition_command(
            "trash",
            "Explicitly discard this message locally, show it under trash/, and queue a Trash \
             move.",
            "Why this disposition is correct; required by default",
        ),
        message_disposition_command(
            "unspam",
            "Mark a spam false positive as triage and queue a move out of Junk when needed.",
            "Why this message is not spam; required by default",
        ),
        message_disposition_command(
            "untrash",
            "Mark a trash false positive as triage and queue a move out of Trash when needed.",
            "Why this message should not be trashed; required by default",
        ),
        Leaf::new(
            ["message", "attachment", "fetch"],
            "Fetch one attachment by MIME part id, or all attachments when omitted.",
        )
        .req(
            ArgSpec::positional("message_id", 0, "MESSAGE_ID")
                .about("Message id, for example message_inbox_607146690_22"),
        )
        .opt(
            ArgSpec::positional("part_id", 1, "PART_ID")
                .about("MIME part id; with none, every attachment is fetched"),
        )
        .one("message_attachment_fetch"),
    ]
}

fn message_disposition_command(verb: &str, about: &str, reason_about: &str) -> CommandSpec {
    Leaf::new(["message", verb], about)
        .idempotent()
        .req(
            ArgSpec::positional("message_id", 0, "MESSAGE_ID")
                .repeatable()
                .about("Message ids, for example message_inbox_607146690_22"),
        )
        .opt(reason_arg(reason_about))
        .one(&format!("message_{verb}"))
}

fn case_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["case"]).about("Create a case or operate on an existing case ref."),
        CommandSpec::new(["case", "suggestion"])
            .about("Set or clear the current typed Agent suggestion for a case."),
        CommandSpec::new(["case", "notes"]).about("Show or edit case notes."),
        CommandSpec::new(["case", "draft"])
            .about("Create, edit, validate, queue, or remove local case drafts."),
        Leaf::new(
            ["case", "create"],
            "Create a new case and return its stable UID/ref.",
        )
        .idempotent()
        .req(
            ArgSpec::option("--name", "NAME")
                .about("Human-readable case name used in case.md and the directory suffix"),
        )
        .req(
            ArgSpec::option("--group", "GROUP").about(
                "Case category/group, for example support, invoice, 客服, 发票, 合同, or 退款",
            ),
        )
        .opt(ArgSpec::option("--message", "MESSAGE_ID").about("First message to add to the case"))
        .opt(ArgSpec::option("--summary", "TEXT").about("Short summary for the first message"))
        .opt(reason_arg("Why this case is being created"))
        .one("case_create"),
        case_list_command(),
        Leaf::new(
            ["case", "show"],
            "Show the exact typed case collection document used by Markdown and Review.",
        )
        .req(case_ref_arg())
        .one("case_show"),
        Leaf::new(
            ["case", "review"],
            "Open a review session for this case, including editable notes.",
        )
        .stream()
        .req(case_ref_arg())
        .opt(mode_arg())
        .one("case_review"),
        case_suggestion_set_command(),
        Leaf::new(
            ["case", "suggestion", "clear"],
            "Clear exactly the current suggestion round.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(
            ArgSpec::option("--agent-suggestion-uid", "UID")
                .about("UID returned by the current suggestion set command"),
        )
        .one("case_suggestion_clear"),
        Leaf::new(["case", "add"], "Add a message to this existing case.")
            .idempotent()
            .req(case_ref_arg())
            .req(
                ArgSpec::positional("message_id", 1, "MESSAGE_ID")
                    .repeatable()
                    .about("Message ids to add"),
            )
            .opt(
                ArgSpec::option("--summary", "TEXT")
                    .about("Short summary for this message in the case"),
            )
            .opt(reason_arg(
                "Why this message belongs in this case; required by default",
            ))
            .one("case_add"),
        Leaf::new(
            ["case", "set-summary"],
            "Set or replace one case entry summary.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(
            ArgSpec::positional("message_id", 1, "MESSAGE_ID")
                .about("Message id whose summary should be changed"),
        )
        .req(ArgSpec::option("--summary", "TEXT").about("New summary text"))
        .opt(reason_arg(
            "Why this summary is useful; required by default",
        ))
        .one("case_set_summary"),
        Leaf::new(
            ["case", "remove"],
            "Remove one message from this case and return it to triage.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(ArgSpec::positional("message_id", 1, "MESSAGE_ID").about("Message id to remove"))
        .opt(reason_arg(
            "Why this message does not belong in this case; required by default",
        ))
        .one("case_remove"),
        Leaf::new(["case", "move"], "Move this case to another group.")
            .idempotent()
            .req(case_ref_arg())
            .req(ArgSpec::option("--group", "GROUP").about("Destination group"))
            .opt(reason_arg(
                "Why this group better represents the case; required by default",
            ))
            .one("case_move"),
        Leaf::new(
            ["case", "rename"],
            "Rename this case's human-readable name without changing its UID.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(ArgSpec::option("--name", "NAME").about("New human-readable case name"))
        .opt(reason_arg(
            "Why this name better represents the case; required by default",
        ))
        .one("case_rename"),
        Leaf::new(["case", "archive"], "Archive this active case.")
            .idempotent()
            .req(case_ref_arg())
            .opt(reason_arg(
                "Why this case is ready to archive; required by default",
            ))
            .one("case_archive"),
        Leaf::new(
            ["case", "unarchive"],
            "Move this archived case back to an active case group.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(ArgSpec::option("--group", "GROUP").about("Active case group to unarchive into"))
        .opt(reason_arg(
            "Why this case needs active attention again; required by default",
        ))
        .one("case_unarchive"),
        Leaf::new(
            ["case", "trash"],
            "Move every message in this case to local trash and queue Trash moves.",
        )
        .idempotent()
        .req(case_ref_arg())
        .opt(reason_arg(
            "Why this whole case should be discarded; required by default",
        ))
        .one("case_trash"),
        Leaf::new(["case", "delete"], "Delete an empty case.")
            .idempotent()
            .req(case_ref_arg())
            .opt(
                ArgSpec::flag("--allow-notes")
                    .about("Also delete notes.md when it contains user-authored notes"),
            )
            .opt(reason_arg(
                "Why this case should be deleted; required by default",
            ))
            .one("case_delete"),
        case_tag_command(
            "tag",
            "Add a case organization tag.",
            "Why this tag is useful; required by default",
        ),
        case_tag_command(
            "untag",
            "Remove a case organization tag.",
            "Why this tag should be removed; required by default",
        ),
        Leaf::new(["case", "merge"], "Merge another case into this case.")
            .req(case_ref_arg())
            .req(
                ArgSpec::positional("other_case_ref", 1, "OTHER_CASE_REF")
                    .about("Case ref to merge into the primary case"),
            )
            .opt(reason_arg(
                "Why these cases should be merged; required by default",
            ))
            .one("case_merge"),
        notes_command(
            ["case", "notes", "show"],
            "Show case notes markdown.",
            case_ref_arg(),
            "case_notes_show",
            None,
        ),
        notes_command(
            ["case", "notes", "append"],
            "Append text to case notes markdown.",
            case_ref_arg(),
            "case_notes_append",
            Some("Markdown text to append"),
        ),
        notes_command(
            ["case", "notes", "replace"],
            "Replace case notes markdown with text.",
            case_ref_arg(),
            "case_notes_replace",
            Some("Markdown text to write"),
        ),
    ]
}

fn case_tag_command(verb: &str, about: &str, reason_about: &str) -> CommandSpec {
    Leaf::new(["case", verb], about)
        .idempotent()
        .req(case_ref_arg())
        .req(ArgSpec::positional("tag", 1, "TAG").about("Case organization tag"))
        .opt(reason_arg(reason_about))
        .one(&format!("case_{verb}"))
}

fn case_list_command() -> CommandSpec {
    Leaf::new(["case", "list"], "List compact case locators.")
        .arg(ArgSpec::flag("--archived").about("List archived cases instead of active cases"))
        .arg(ArgSpec::flag("--all").about("List active and archived cases together"))
        .shapes([
            shape(
                "case-list-active",
                "case_list",
                "List active cases; this is the default",
            ),
            shape(
                "case-list-archived",
                "case_list",
                "List archived cases only",
            )
            .required(["archived"]),
            shape(
                "case-list-all",
                "case_list",
                "List active and archived cases together",
            )
            .required(["all"]),
        ])
}

fn case_suggestion_set_command() -> CommandSpec {
    Leaf::new(
        ["case", "suggestion", "set"],
        "Replace the current suggestion and clear any human review of the prior round.",
    )
    .idempotent()
    .req(case_ref_arg())
    .req(
        ArgSpec::option("--case-review-base-hash", "HASH")
            .about("Current hash returned by `afmail case show`"),
    )
    .req(reason_arg("Why this complete action is being suggested"))
    .opt(confidence_arg())
    .arg(suggestion_action_arg([
        "archive",
        "unarchive",
        "move",
        "rename",
        "tag",
        "untag",
        "trash",
        "delete",
    ]))
    .arg(ArgSpec::option("--group", "GROUP").about("Destination case group"))
    .arg(ArgSpec::option("--name", "NAME").about("New human-readable case name"))
    .arg(ArgSpec::option("--tag", "TAG").about("Case organization tag"))
    .arg(
        ArgSpec::flag("--allow-notes")
            .about("Also delete notes.md when it contains user-authored notes"),
    )
    .shapes([
        shape(
            "case-suggestion-set-archive",
            "case_suggestion_set",
            "Suggest archiving this case",
        )
        .fixed("action", "archive"),
        shape(
            "case-suggestion-set-unarchive",
            "case_suggestion_set",
            "Suggest unarchiving this case into a group",
        )
        .fixed("action", "unarchive")
        .required(["group"]),
        shape(
            "case-suggestion-set-move",
            "case_suggestion_set",
            "Suggest moving this case to another group",
        )
        .fixed("action", "move")
        .required(["group"]),
        shape(
            "case-suggestion-set-rename",
            "case_suggestion_set",
            "Suggest renaming this case",
        )
        .fixed("action", "rename")
        .required(["name"]),
        shape(
            "case-suggestion-set-tag",
            "case_suggestion_set",
            "Suggest adding a tag",
        )
        .fixed("action", "tag")
        .required(["tag"]),
        shape(
            "case-suggestion-set-untag",
            "case_suggestion_set",
            "Suggest removing a tag",
        )
        .fixed("action", "untag")
        .required(["tag"]),
        shape(
            "case-suggestion-set-trash",
            "case_suggestion_set",
            "Suggest trashing every message in this case",
        )
        .fixed("action", "trash"),
        shape(
            "case-suggestion-set-delete",
            "case_suggestion_set",
            "Suggest deleting this case",
        )
        .fixed("action", "delete")
        .optional(["allow_notes"]),
    ])
}

fn notes_command<I, S>(
    path: I,
    about: &str,
    ref_arg: ArgSpec,
    action: &str,
    text_about: Option<&str>,
) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut leaf = Leaf::new(path, about).req(ref_arg);
    if let Some(text_about) = text_about {
        leaf = leaf
            .idempotent()
            .req(ArgSpec::option("--text", "TEXT").about(text_about));
    }
    leaf.one(action)
}

fn case_draft_commands() -> Vec<CommandSpec> {
    vec![
        case_draft_new_command(),
        case_draft_reply_command(),
        case_draft_change_command(),
        Leaf::new(
            ["case", "draft", "show"],
            "Show an existing draft as a typed render document.",
        )
        .req(case_ref_arg())
        .req(draft_name_arg())
        .one("case_draft_show"),
        Leaf::new(
            ["case", "draft", "review"],
            "Open a review session for this draft.",
        )
        .stream()
        .req(case_ref_arg())
        .req(draft_name_arg())
        .opt(mode_arg())
        .one("case_draft_review"),
        Leaf::new(
            ["case", "draft", "validate"],
            "Validate a draft under the case drafts directory.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(draft_name_arg())
        .one("case_draft_validate"),
        Leaf::new(
            ["case", "draft", "list"],
            "List local drafts with their current status.",
        )
        .req(case_ref_arg())
        .one("case_draft_list"),
        Leaf::new(
            ["case", "draft", "upload"],
            "Queue this draft to be uploaded to the remote Drafts mailbox.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(draft_name_arg())
        .one("case_draft_upload"),
        Leaf::new(
            ["case", "draft", "send"],
            "Queue this draft to be sent and recorded in the case after push succeeds.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(draft_name_arg())
        .one("case_draft_send"),
        Leaf::new(
            ["case", "draft", "attach"],
            "Copy or reference a file and add it to a draft's attachments.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(draft_name_arg())
        .req(ArgSpec::positional("path", 2, "PATH").about("Local file path to attach"))
        .one("case_draft_attach"),
        Leaf::new(
            ["case", "draft", "remove"],
            "Remove a local draft and any queued outbound item for it.",
        )
        .idempotent()
        .req(case_ref_arg())
        .req(draft_name_arg())
        .opt(reason_arg(
            "Why this draft should be removed; required by default",
        ))
        .one("case_draft_remove"),
    ]
}

fn body_arg(about: &str) -> ArgSpec {
    ArgSpec::option("--body", "TEXT").about(about)
}

fn body_file_arg(about: &str) -> ArgSpec {
    ArgSpec::option("--body-file", "PATH").about(about)
}

fn identity_arg() -> ArgSpec {
    ArgSpec::option("--identity", "SLUG")
        .about("Workspace identity slug to send as. Defaults to the configured default identity.")
}

fn case_draft_new_command() -> CommandSpec {
    Leaf::new(
        ["case", "draft", "new"],
        "Scaffold a new outbound draft (not a reply) in this case.",
    )
    .idempotent()
    .req(case_ref_arg())
    .arg(
        ArgSpec::option("--to", "ADDRESS")
            .repeatable()
            .about("Recipient address; repeat for more than one"),
    )
    .req(ArgSpec::option("--subject", "TEXT").about("Draft subject"))
    .opt(
        ArgSpec::option("--cc", "ADDRESS")
            .repeatable()
            .about("Cc address; repeat for more than one"),
    )
    .arg(
        ArgSpec::option("--bcc", "ADDRESS")
            .repeatable()
            .about("Bcc address; repeat for more than one"),
    )
    .opt(identity_arg())
    .arg(body_arg("Draft body text"))
    .arg(body_file_arg(
        "Path to a file whose contents become the draft body",
    ))
    .shapes([
        shape(
            "case-draft-new-to-inline-body",
            "case_draft_new",
            "Visible recipient with an inline body or the default template",
        )
        .required(["to"])
        .optional(["bcc", "body"]),
        shape(
            "case-draft-new-to-body-file",
            "case_draft_new",
            "Visible recipient with the body read from a file",
        )
        .required(["to", "body_file"])
        .optional(["bcc"]),
        shape(
            "case-draft-new-bcc-only-inline-body",
            "case_draft_new",
            "Only blind recipients, with an inline body or the default template",
        )
        .required(["bcc"])
        .optional(["body"]),
        shape(
            "case-draft-new-bcc-only-body-file",
            "case_draft_new",
            "Only blind recipients, with the body read from a file",
        )
        .required(["bcc", "body_file"]),
    ])
}

fn case_draft_reply_command() -> CommandSpec {
    Leaf::new(
        ["case", "draft", "reply"],
        "Scaffold a reply draft to a message, prefilled and quoting the original.",
    )
    .idempotent()
    .req(case_ref_arg())
    .req(
        ArgSpec::positional("message_id", 1, "MESSAGE_ID")
            .about("Message id in this case to reply to"),
    )
    .opt(identity_arg())
    .opt(
        ArgSpec::flag("--all")
            .about("Reply to all original recipients (To and Cc), not just the sender"),
    )
    .opt(
        ArgSpec::option("--bcc", "ADDRESS")
            .repeatable()
            .about("Bcc address; repeat for more than one"),
    )
    .arg(body_arg(
        "Draft body text. Replaces the default reply template when provided.",
    ))
    .arg(body_file_arg(
        "Path to a file whose contents become the draft body",
    ))
    .shapes([
        shape(
            "case-draft-reply-inline-body",
            "case_draft_reply",
            "Body given inline, or left to the default reply template",
        )
        .optional(["body"]),
        shape(
            "case-draft-reply-body-file",
            "case_draft_reply",
            "Body read from a file; the inline --body is absent here",
        )
        .required(["body_file"]),
    ])
}

fn case_draft_change_command() -> CommandSpec {
    Leaf::new(
        ["case", "draft", "change"],
        "Change an existing editable draft in place.",
    )
    .idempotent()
    .req(case_ref_arg())
    .req(draft_name_arg())
    .opt(ArgSpec::option("--subject", "TEXT").about("Replacement subject"))
    .opt(
        ArgSpec::option("--to", "ADDRESS")
            .repeatable()
            .about("Replacement To list; when provided it replaces all To recipients"),
    )
    .opt(identity_arg())
    .arg(
        ArgSpec::option("--cc", "ADDRESS")
            .repeatable()
            .about("Replacement Cc list; when provided it replaces all Cc recipients"),
    )
    .arg(ArgSpec::flag("--clear-cc").about("Clear all Cc recipients"))
    .arg(
        ArgSpec::option("--bcc", "ADDRESS")
            .repeatable()
            .about("Replacement Bcc list; when provided it replaces all Bcc recipients"),
    )
    .arg(ArgSpec::flag("--clear-bcc").about("Clear all Bcc recipients"))
    .arg(body_arg("Replacement draft body text"))
    .arg(body_file_arg(
        "Path to a file whose contents replace the draft body",
    ))
    .shapes([
        shape(
            "case-draft-change-recipients-body",
            "case_draft_change",
            "Keep or replace Cc and Bcc, with an optional inline body",
        )
        .optional(["cc", "bcc", "body"]),
        shape(
            "case-draft-change-recipients-body-file",
            "case_draft_change",
            "Keep or replace Cc and Bcc, with the body read from a file",
        )
        .optional(["cc", "bcc"])
        .required(["body_file"]),
        shape(
            "case-draft-change-clear-cc-body",
            "case_draft_change",
            "Clear Cc and keep or replace Bcc, with an optional inline body",
        )
        .required(["clear_cc"])
        .optional(["bcc", "body"]),
        shape(
            "case-draft-change-clear-cc-body-file",
            "case_draft_change",
            "Clear Cc and keep or replace Bcc, with the body read from a file",
        )
        .required(["clear_cc", "body_file"])
        .optional(["bcc"]),
        shape(
            "case-draft-change-clear-bcc-body",
            "case_draft_change",
            "Clear Bcc and keep or replace Cc, with an optional inline body",
        )
        .required(["clear_bcc"])
        .optional(["cc", "body"]),
        shape(
            "case-draft-change-clear-bcc-body-file",
            "case_draft_change",
            "Clear Bcc and keep or replace Cc, with the body read from a file",
        )
        .required(["clear_bcc", "body_file"])
        .optional(["cc"]),
        shape(
            "case-draft-change-clear-cc-bcc-body",
            "case_draft_change",
            "Clear Cc and Bcc, with an optional inline body",
        )
        .required(["clear_cc", "clear_bcc"])
        .optional(["body"]),
        shape(
            "case-draft-change-clear-cc-bcc-body-file",
            "case_draft_change",
            "Clear Cc and Bcc, with the body read from a file",
        )
        .required(["clear_cc", "clear_bcc", "body_file"]),
    ])
}

fn contact_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["contact"])
            .about("Create a contact or operate on an existing contact ref."),
        CommandSpec::new(["contact", "email"])
            .about("Add or remove email addresses for a contact."),
        CommandSpec::new(["contact", "phone"]).about("Add or remove phone numbers for a contact."),
        CommandSpec::new(["contact", "notes"]).about("Show or edit contact notes."),
        Leaf::new(["contact", "create"], "Create a new contact record.")
            .idempotent()
            .req(ArgSpec::option("--name", "NAME").about("Display name for the contact"))
            .opt(
                ArgSpec::option("--group", "GROUP")
                    .about("Group. Defaults to the configured default contact group."),
            )
            .opt(
                ArgSpec::option("--email", "ADDRESS")
                    .repeatable()
                    .about("Email address; repeat for more than one"),
            )
            .opt(
                ArgSpec::option("--phone", "NUMBER")
                    .repeatable()
                    .about("Phone number; repeat for more than one"),
            )
            .opt(ArgSpec::option("--org", "NAME").about("Organization name"))
            .opt(ArgSpec::option("--role", "ROLE").about("Role or title"))
            .opt(
                ArgSpec::option("--tag", "TAG")
                    .repeatable()
                    .about("Tag; repeat for more than one"),
            )
            .opt(reason_arg(
                "Why this contact is being created; required by default",
            ))
            .one("contact_create"),
        Leaf::new(
            ["contact", "list"],
            "List contacts, optionally filtered by group, tag, org, or role.",
        )
        .opt(ArgSpec::option("--group", "GROUP").about("Filter to a specific group"))
        .opt(ArgSpec::option("--tag", "TAG").about("Filter by tag"))
        .opt(ArgSpec::option("--org", "NAME").about("Filter by organization"))
        .opt(ArgSpec::option("--role", "ROLE").about("Filter by role"))
        .one("contact_list"),
        Leaf::new(
            ["contact", "show"],
            "Show the exact typed contact document used by its Markdown template.",
        )
        .req(contact_ref_arg())
        .one("contact_show"),
        Leaf::new(["contact", "move"], "Move a contact to a different group.")
            .idempotent()
            .req(contact_ref_arg())
            .req(ArgSpec::option("--group", "GROUP").about("Destination group"))
            .opt(reason_arg(
                "Why this contact should move groups; required by default",
            ))
            .one("contact_move"),
        Leaf::new(["contact", "rename"], "Rename a contact's display name.")
            .idempotent()
            .req(contact_ref_arg())
            .req(ArgSpec::option("--name", "NAME").about("New display name"))
            .opt(reason_arg(
                "Why this name better represents the contact; required by default",
            ))
            .one("contact_rename"),
        contact_value_command(
            ["contact", "email", "add"],
            "Add an email address to the contact.",
            ArgSpec::positional("email", 1, "EMAIL").about("Email address to add"),
            "contact_email_add",
            "Why this email should be associated with the contact; required by default",
        ),
        contact_value_command(
            ["contact", "email", "remove"],
            "Remove an email address from the contact.",
            ArgSpec::positional("email", 1, "EMAIL").about("Email address to remove"),
            "contact_email_remove",
            "Why this email should be removed; required by default",
        ),
        contact_value_command(
            ["contact", "phone", "add"],
            "Add a phone number to the contact.",
            ArgSpec::positional("phone", 1, "PHONE").about("Phone number to add"),
            "contact_phone_add",
            "Why this phone number should be associated with the contact; required by default",
        ),
        contact_value_command(
            ["contact", "phone", "remove"],
            "Remove a phone number from the contact.",
            ArgSpec::positional("phone", 1, "PHONE").about("Phone number to remove"),
            "contact_phone_remove",
            "Why this phone number should be removed; required by default",
        ),
        contact_value_command(
            ["contact", "tag"],
            "Add a tag to a contact.",
            ArgSpec::positional("tag", 1, "TAG").about("Tag to add"),
            "contact_tag",
            "Why this tag is useful; required by default",
        ),
        contact_value_command(
            ["contact", "untag"],
            "Remove a tag from a contact.",
            ArgSpec::positional("tag", 1, "TAG").about("Tag to remove"),
            "contact_untag",
            "Why this tag should be removed; required by default",
        ),
        Leaf::new(["contact", "delete"], "Delete a contact.")
            .idempotent()
            .req(contact_ref_arg())
            .opt(
                ArgSpec::flag("--allow-notes")
                    .about("Also delete the contact body when it contains user-authored notes"),
            )
            .opt(reason_arg(
                "Why this contact should be deleted; required by default",
            ))
            .one("contact_delete"),
        contact_extract_command(),
        notes_command(
            ["contact", "notes", "show"],
            "Show the typed contact document with raw notes Markdown.",
            contact_ref_arg(),
            "contact_notes_show",
            None,
        ),
        notes_command(
            ["contact", "notes", "append"],
            "Append text to contact notes markdown.",
            contact_ref_arg(),
            "contact_notes_append",
            Some("Markdown text to append"),
        ),
        notes_command(
            ["contact", "notes", "replace"],
            "Replace contact notes markdown with text.",
            contact_ref_arg(),
            "contact_notes_replace",
            Some("Markdown text to write"),
        ),
    ]
}

fn contact_value_command<I, S>(
    path: I,
    about: &str,
    value: ArgSpec,
    action: &str,
    reason_about: &str,
) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Leaf::new(path, about)
        .idempotent()
        .req(contact_ref_arg())
        .req(value)
        .opt(reason_arg(reason_about))
        .one(action)
}

fn contact_extract_command() -> CommandSpec {
    Leaf::new(
        ["contact", "extract"],
        "Extract stub contacts from message senders not yet in the contact list.",
    )
    .opt(
        ArgSpec::option("--group", "GROUP")
            .about("Destination group for created contacts. Defaults to contact.default_group."),
    )
    .arg(ArgSpec::flag("--from-triage").about("Extract from triage messages only"))
    .arg(
        ArgSpec::option("--from-case", "CASE_REF").about("Extract from a specific case's messages"),
    )
    .arg(ArgSpec::flag("--all").about("Extract from all triage and case messages"))
    .shapes([
        shape(
            "contact-extract-triage",
            "contact_extract",
            "Extract from triage messages; this is the default source",
        )
        .optional(["from_triage"]),
        shape(
            "contact-extract-case",
            "contact_extract",
            "Extract from one case's messages",
        )
        .required(["from_case"]),
        shape(
            "contact-extract-all",
            "contact_extract",
            "Extract from every triage and case message",
        )
        .required(["all"]),
    ])
}

fn notification_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["notification"]).about("Inspect and manage notification collections."),
        CommandSpec::new(["notification", "suggestion"])
            .about("Set or clear the current typed Agent suggestion for a collection."),
        CommandSpec::new(["notification", "notes"])
            .about("Show or edit notification collection notes."),
        notification_create_command(),
        Leaf::new(
            ["notification", "add"],
            "File an existing message into this notification collection.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .req(
            ArgSpec::positional("message_id", 1, "MESSAGE_ID")
                .repeatable()
                .about("Message ids to file"),
        )
        .req(
            ArgSpec::option("--summary", "TEXT")
                .about("Human/agent-authored summary for this notification entry"),
        )
        .opt(reason_arg(
            "Why this message belongs in this notification; required by default",
        ))
        .one("notification_add"),
        Leaf::new(
            ["notification", "list"],
            "List compact notification collections.",
        )
        .one("notification_list"),
        Leaf::new(
            ["notification", "show"],
            "Show the exact typed notification document used by Markdown and Review.",
        )
        .req(notification_ref_arg())
        .one("notification_show"),
        Leaf::new(
            ["notification", "review"],
            "Open a review session for this notification and its persisted suggestion.",
        )
        .stream()
        .req(notification_ref_arg())
        .opt(mode_arg())
        .one("notification_review"),
        notification_suggestion_set_command(),
        Leaf::new(
            ["notification", "suggestion", "clear"],
            "Clear exactly the current suggestion round.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .req(
            ArgSpec::option("--agent-suggestion-uid", "UID")
                .about("UID returned by the current suggestion set command"),
        )
        .one("notification_suggestion_clear"),
        Leaf::new(
            ["notification", "remove"],
            "Remove a notification message from this collection and return it to triage.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .req(ArgSpec::positional("message_id", 1, "MESSAGE_ID").about("Message id to remove"))
        .opt(reason_arg(
            "Why this message does not belong in this notification; required by default",
        ))
        .one("notification_remove"),
        Leaf::new(
            ["notification", "move"],
            "Move a notification message to another notification collection.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .req(ArgSpec::positional("message_id", 1, "MESSAGE_ID").about("Message id to move"))
        .req(
            ArgSpec::positional("new_notification_ref", 2, "NEW_NOTIFICATION_REF")
                .about("Destination notification collection ref"),
        )
        .opt(reason_arg(
            "Why this collection is better; required by default",
        ))
        .one("notification_move"),
        Leaf::new(
            ["notification", "rename"],
            "Rename this notification collection's human-readable name without changing its UID.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .req(ArgSpec::option("--name", "NAME").about("New human-readable notification name"))
        .opt(reason_arg(
            "Why this name better represents the collection; required by default",
        ))
        .one("notification_rename"),
        Leaf::new(
            ["notification", "delete"],
            "Delete an empty notification collection.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .opt(
            ArgSpec::flag("--allow-notes")
                .about("Also delete notes.md when it contains user-authored notes"),
        )
        .opt(reason_arg(
            "Why this notification collection should be deleted; required by default",
        ))
        .one("notification_delete"),
        Leaf::new(
            ["notification", "set-summary"],
            "Set or replace one notification entry summary.",
        )
        .idempotent()
        .req(notification_ref_arg())
        .req(
            ArgSpec::positional("message_id", 1, "MESSAGE_ID")
                .about("Message id whose summary should be changed"),
        )
        .req(ArgSpec::option("--summary", "TEXT").about("New summary text"))
        .opt(reason_arg(
            "Why this summary is useful; required by default",
        ))
        .one("notification_set_summary"),
        notes_command(
            ["notification", "notes", "show"],
            "Show notification notes markdown.",
            notification_ref_arg(),
            "notification_notes_show",
            None,
        ),
        notes_command(
            ["notification", "notes", "append"],
            "Append text to notification notes markdown.",
            notification_ref_arg(),
            "notification_notes_append",
            Some("Markdown text to append"),
        ),
        notes_command(
            ["notification", "notes", "replace"],
            "Replace notification notes markdown with text.",
            notification_ref_arg(),
            "notification_notes_replace",
            Some("Markdown text to write"),
        ),
    ]
}

fn notification_create_command() -> CommandSpec {
    Leaf::new(
        ["notification", "create"],
        "Create a notification collection and optionally file one message.",
    )
    .idempotent()
    .req(
        ArgSpec::option("--name", "NAME")
            .about("Human-readable notification name used in metadata and the directory suffix"),
    )
    .opt(reason_arg(
        "Why this notification collection is being created",
    ))
    .arg(
        ArgSpec::option("--message", "MESSAGE_ID")
            .about("Message to immediately file into the new collection"),
    )
    .arg(
        ArgSpec::option("--summary", "TEXT")
            .about("Summary for the filed message's notification entry"),
    )
    .shapes([
        shape(
            "notification-create-empty",
            "notification_create",
            "Create an empty collection; with no message there is nothing to summarize",
        ),
        shape(
            "notification-create-with-message",
            "notification_create",
            "Create the collection and file one message, which always needs a summary",
        )
        .required(["message", "summary"]),
    ])
}

fn notification_suggestion_set_command() -> CommandSpec {
    Leaf::new(
        ["notification", "suggestion", "set"],
        "Replace the current suggestion and clear any human review of the prior round.",
    )
    .idempotent()
    .req(notification_ref_arg())
    .req(
        ArgSpec::option("--notification-review-base-hash", "HASH")
            .about("Current hash returned by `afmail notification show`"),
    )
    .req(reason_arg("Why this complete action is being suggested"))
    .opt(confidence_arg())
    .arg(suggestion_action_arg(["rename", "delete"]))
    .arg(ArgSpec::option("--name", "NAME").about("New human-readable notification name"))
    .arg(
        ArgSpec::flag("--allow-notes")
            .about("Also delete notes.md when it contains user-authored notes"),
    )
    .shapes([
        shape(
            "notification-suggestion-set-rename",
            "notification_suggestion_set",
            "Suggest renaming this collection",
        )
        .fixed("action", "rename")
        .required(["name"]),
        shape(
            "notification-suggestion-set-delete",
            "notification_suggestion_set",
            "Suggest deleting this collection",
        )
        .fixed("action", "delete")
        .optional(["allow_notes"]),
    ])
}

fn render_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["render"])
            .about("Rebuild generated read views from local workspace state."),
        Leaf::new(
            ["render", "refresh"],
            "Rebuild generated case and notification read views.",
        )
        .one("render_refresh"),
        Leaf::new(
            ["render", "templates"],
            "Export built-in language templates, keeping existing files unless forced.",
        )
        .opt(
            ArgSpec::flag("--force")
                .about("Overwrite existing workspace templates with built-in defaults"),
        )
        .one("render_templates"),
    ]
}

fn log_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["log"]).about("Inspect the workspace audit log."),
        Leaf::new(["log", "list"], "List recent audit events.")
            .opt(
                ArgSpec::option_i64("--limit", "COUNT")
                    .default_i64(50)
                    .about("Maximum number of events to return"),
            )
            .one("log_list"),
        Leaf::new(["log", "tail"], "Tail recent audit events as JSON data.").one("log_tail"),
        Leaf::new(["log", "message"], "List events for one message id.")
            .req(ArgSpec::positional("message_id", 0, "MESSAGE_ID").about("Message id"))
            .one("log_message"),
        Leaf::new(["log", "case"], "List events for one case id.")
            .req(ArgSpec::positional("case_uid", 0, "CASE_UID").about("Case UID"))
            .one("log_case"),
        Leaf::new(
            ["log", "notification"],
            "List events for one notification collection.",
        )
        .req(
            ArgSpec::positional("notification_uid", 0, "NOTIFICATION_UID")
                .about("Notification UID"),
        )
        .one("log_notification"),
    ]
}

fn api_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["api"])
            .about("Serve or export the afmail OpenAPI 3.2 contract and JSON Schemas."),
        Leaf::new(
            ["api", "serve"],
            "Serve the typed afmail HTTP API and its OpenAPI/JSON Schema documents.",
        )
        .stream()
        .opt(
            ArgSpec::option_enum("--mode", ["local", "lan"])
                .value_name("MODE")
                .default("local")
                .about("Network exposure mode"),
        )
        .opt(
            ArgSpec::option_i64("--port", "PORT")
                .default_i64(9417)
                .about("TCP port. Use 0 to let the operating system choose an available port."),
        )
        .opt(
            ArgSpec::option("--access-token-secret", "SOURCE")
                .about("Bearer credential; falls back to AFMAIL_API_ACCESS_TOKEN_SECRET.")
                .sources(secret_sources()),
        )
        .one("api_serve"),
        Leaf::new(
            ["api", "export"],
            "Write the generated OpenAPI document and standalone JSON Schemas.",
        )
        .opt(
            ArgSpec::option("--directory", "DIR")
                .default("openapi")
                .about("Destination directory"),
        )
        .opt(ArgSpec::flag("--force").about("Replace existing generated files"))
        .one("api_export"),
    ]
}

fn demo_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(["demo"]).about("Create and run a complete local mailbox demo."),
        Leaf::creating(
            ["demo", "init"],
            "Create a normal workspace wired to the local demo mail server.",
        )
        .opt(
            ArgSpec::positional("path", 0, "PATH")
                .default("afmail-demo")
                .about("Workspace path under the current directory"),
        )
        .opt(
            ArgSpec::option_enum("--scenario", ["founder-inbox"])
                .value_name("SCENARIO")
                .default("founder-inbox")
                .about("Built-in mailbox scenario"),
        )
        .opt(
            ArgSpec::option_i64("--imap-port", "PORT")
                .default_i64(14300)
                .about("Local IMAP port"),
        )
        .opt(
            ArgSpec::option_i64("--smtp-port", "PORT")
                .default_i64(14301)
                .about("Local SMTP port"),
        )
        .one("demo_init"),
        Leaf::new(
            ["demo", "serve"],
            "Run the local demo IMAP and SMTP servers until interrupted.",
        )
        .stream()
        .opt(
            ArgSpec::flag("--verbose")
                .about("Also report every raw IMAP and SMTP protocol line the demo server sees"),
        )
        .one("demo_serve"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::build_command;
    use agent_first_data::{CliErrorRule, CliOutcome};
    use serde_json::Value;

    const README: &str = include_str!("../../README.md");
    const SKILL: &str = include_str!("../../skills/agent-first-mail/SKILL.md");
    const FILE_FORMATS: &str = include_str!("../../docs/file-formats.md");
    const SPORE_CORE: &str = include_str!("../../spore.core.json");

    fn built() -> BuiltCliSpec {
        match cli_spec() {
            Ok(cli) => cli,
            Err(error) => panic!("registry must build: {error}"),
        }
    }

    /// Every declared shape, run through the same binding `main` uses, with
    /// strict argument reads: a builder arm that asks for an argument id its
    /// shape cannot supply names itself here rather than silently degrading to
    /// an empty string in production.
    ///
    /// Safe to run because `build_command` only projects — it opens no
    /// mailbox, writes no workspace file, and reaches no remote.
    #[test]
    fn every_combination_reads_only_ids_its_shape_declares() {
        let cli = built();
        let app = match cli.bind_actions(crate::cli::action_handlers(&cli)) {
            Ok(app) => app,
            Err(error) => panic!("handlers must cover every action: {error}"),
        };
        app.call_every_combination();
    }

    /// Each generated argv must resolve back to the shape it came from, so an
    /// overlapping or unreachable combination fails here rather than at a
    /// caller's first invocation. Building the command too proves every
    /// registered action id has an arm in `super::super::build`.
    #[test]
    fn registry_builds_and_every_shape_is_reachable() {
        let cli = built();
        let synthetics = cli.synthetic_invocations();
        assert!(!synthetics.is_empty(), "the registry generated no fixtures");
        for synthetic in synthetics {
            let argv = synthetic.argv.clone();
            match cli.resolve_from(argv.clone()) {
                Ok(CliOutcome::Run(invocation)) => {
                    assert_eq!(
                        invocation.combination_id(),
                        synthetic.combination_id,
                        "{argv:?} resolved to the wrong shape"
                    );
                    assert!(
                        build_command(&invocation).is_ok(),
                        "{argv:?} resolved but built no command"
                    );
                }
                Ok(_) => panic!("{argv:?} did not resolve to a run"),
                Err(error) => panic!("{argv:?} failed to resolve: {}", error.message),
            }
        }
    }

    /// `--dry-run` previews and `--confirm` writes; the pair was a runtime
    /// error and is now simply not a registered shape.
    #[test]
    fn push_dry_run_and_confirm_are_not_one_shape() {
        let cli = built();
        let error = match cli.resolve_from(["afmail", "push", "--dry-run", "--confirm", "plan_1"]) {
            Err(error) => error,
            Ok(_) => panic!("--dry-run with --confirm must be rejected"),
        };
        assert_eq!(error.rule, CliErrorRule::UnregisteredCombination);
    }

    /// A review or serve command reports an address the caller must act on,
    /// waits, then reports the outcome; its contract has no `split`.
    #[test]
    fn event_stream_commands_have_no_split_destination() {
        let cli = built();
        let error = match cli.resolve_from(["afmail", "demo", "serve", "--output-to", "split"]) {
            Err(error) => error,
            Ok(_) => panic!("a stream command must reject --output-to split"),
        };
        assert_eq!(error.rule, CliErrorRule::InvalidArgumentValue);
    }

    #[test]
    fn readme_main_journey_afmail_commands_resolve() {
        let commands = fenced_commands(README);
        assert!(!commands.is_empty());
        assert_commands_resolve("README.md", &commands);
    }

    #[test]
    fn embedded_skill_afmail_commands_resolve() {
        let commands = inline_commands(SKILL);
        assert!(
            commands.len() > 20,
            "expected broad skill command coverage, got {commands:?}"
        );
        assert_commands_resolve("skills/agent-first-mail/SKILL.md", &commands);
    }

    #[test]
    fn file_format_json_fixtures_parse() {
        let blocks = fenced_json_blocks(FILE_FORMATS);
        assert!(
            blocks.len() > 8,
            "expected file-format JSON fixtures, got {}",
            blocks.len()
        );
        for (index, block) in blocks.iter().enumerate() {
            let parsed = serde_json::from_str::<Value>(block);
            assert!(
                parsed.is_ok(),
                "docs/file-formats.md JSON block #{index} must parse: {block}"
            );
        }
    }

    #[test]
    fn manifest_intent_covers_cli_capability_inventory() {
        let manifest = serde_json::from_str::<Value>(SPORE_CORE);
        assert!(manifest.is_ok(), "spore.core.json must parse");
        let Ok(manifest) = manifest else {
            return;
        };
        let intent = manifest["intent"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .to_lowercase()
            })
            .unwrap_or_default();

        for required in [
            "pull",
            "triage",
            "case",
            "notification",
            "contact",
            "draft",
            "push",
            "audit",
            "queue",
            "remote",
            "attachment",
        ] {
            assert!(
                intent.contains(required),
                "spore.core.json intent must mention CLI capability '{required}'"
            );
        }
    }

    fn assert_commands_resolve(source: &str, commands: &[String]) {
        let cli = built();
        for command in commands {
            let argv = tokenize_command(command);
            assert!(
                argv.len() > 1,
                "{source} command should include an afmail subcommand: {command}"
            );
            match cli.resolve_from(&argv) {
                Ok(_) => {}
                Err(error) => panic!(
                    "{source} command must resolve: {command}\nargv: {argv:?}\nerror: {}",
                    error.message
                ),
            }
        }
    }

    fn fenced_commands(text: &str) -> Vec<String> {
        let mut commands = Vec::new();
        let mut in_shell_fence = false;
        let mut pending = String::new();

        for raw_line in text.lines() {
            let trimmed = raw_line.trim();
            if trimmed.starts_with("```") {
                in_shell_fence = matches!(trimmed, "```bash" | "```sh" | "```text");
                continue;
            }
            if !in_shell_fence {
                continue;
            }

            let Some(line) = command_part(raw_line) else {
                continue;
            };
            if pending.is_empty() && !line.trim_start().starts_with("afmail ") {
                continue;
            }

            let line = line.trim();
            if let Some(prefix) = line.strip_suffix('\\') {
                pending.push_str(prefix.trim_end());
                pending.push(' ');
                continue;
            }

            pending.push_str(line);
            if command_is_parseable_contract(&pending) {
                commands.push(normalize_placeholders(&pending));
            }
            pending.clear();
        }
        commands
    }

    fn inline_commands(text: &str) -> Vec<String> {
        let mut commands = Vec::new();
        for segment in text.split('`').skip(1).step_by(2) {
            let command = segment.trim();
            if command.starts_with("afmail ") && command_is_parseable_contract(command) {
                commands.push(normalize_placeholders(command));
            }
        }
        commands.sort();
        commands.dedup();
        commands
    }

    fn fenced_json_blocks(text: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut in_json_fence = false;
        let mut current = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "```json" {
                in_json_fence = true;
                current.clear();
                continue;
            }
            if in_json_fence && trimmed == "```" {
                in_json_fence = false;
                blocks.push(current.join("\n"));
                continue;
            }
            if in_json_fence {
                current.push(line);
            }
        }
        blocks
    }

    fn command_is_parseable_contract(command: &str) -> bool {
        let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
        !command.contains("...")
            && !command.contains('|')
            && !command.contains('[')
            && !command.contains(']')
            && !command.contains(" --help")
            && !command.starts_with("afmail --help")
            && normalized != "afmail case draft send"
            && normalized != "afmail case draft upload"
            && normalized != "afmail message attachment fetch"
            && normalized != "afmail case show"
            && normalized != "afmail case archive"
            && normalized != "afmail contact"
            && normalized != "afmail message show"
            && normalized != "afmail notification show"
    }

    fn normalize_placeholders(command: &str) -> String {
        command
            .replace("PLAN_ID", "push_plan_placeholder")
            .replace("purge_plan_...", "purge_plan_placeholder")
            .replace("<REVIEWED_PLAN_ID>", "reviewed_plan_id")
            .replace("MESSAGE_ID", "message_inbox_1")
            .replace("NOTIFICATION_REF", "n20260711001")
            .replace("DRAFT_NAME", "reply.md")
            .replace("CASE_REF", "c20260711001")
            .replace("CATEGORY", "support")
            .replace("GROUP", "support")
            .replace("TEXT", "reason")
            .replace("REF", "c20260711001")
            .replace("NAME", "Name")
            .replace("N", "10")
            .replace("PATH", "file.txt")
    }

    fn command_part(line: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = line.chars().peekable();
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while let Some(ch) = chars.next() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                out.push(ch);
                continue;
            }
            if let Some(active) = quote {
                if ch == active {
                    quote = None;
                }
                out.push(ch);
                continue;
            }
            if ch == '"' || ch == '\'' {
                quote = Some(ch);
                out.push(ch);
                continue;
            }
            if ch == '#' && chars.peek().is_some() {
                break;
            }
            out.push(ch);
        }
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn tokenize_command(command: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut token = String::new();
        let mut quote: Option<char> = None;
        let mut escaped = false;

        for ch in command.chars() {
            if escaped {
                token.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if let Some(active) = quote {
                if ch == active {
                    quote = None;
                } else {
                    token.push(ch);
                }
                continue;
            }
            if ch == '"' || ch == '\'' {
                quote = Some(ch);
                continue;
            }
            if ch.is_whitespace() {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                continue;
            }
            token.push(ch);
        }
        if !token.is_empty() {
            tokens.push(token);
        }
        tokens
    }
}
