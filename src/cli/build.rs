//! One resolved invocation, projected onto the afmail command it names.
//!
//! The registry in [`super::spec`] has already decided that the invocation is
//! legal, so nothing here re-checks which arguments may appear together. What
//! is left is the handful of facts a `cli-spec-v1` argument type cannot carry:
//! integer ranges narrower than `i64`, and how many message ids an idempotency
//! key can cover. Those report the same classification the parser would —
//! `cli_invalid_argument_value` — rather than inventing a second spelling for
//! the same failure.

use super::{
    ApiAction, ApiMode, CaseCommand, CaseCreateArgs, CaseDraftAction, CaseNotesAction,
    CaseSuggestionAction, CaseSuggestionActionKind, CaseSuggestionSetArgs, Command, ConfigAction,
    ContactCommand, ContactCreateArgs, ContactEmailAction, ContactNotesAction, ContactPhoneAction,
    DemoAction, DemoScenario, DoctorAction, LogAction, MessageAction, MessageAttachmentAction,
    MessageListScope, MessageSuggestionAction, NotificationCommand, NotificationCreateArgs,
    NotificationNotesAction, NotificationSuggestionAction, NotificationSuggestionActionKind,
    NotificationSuggestionSetArgs, PurgeAction, PushAction, RemoteAction, RenderAction, ReviewMode,
    SkillAction, SkillAgentSelection, SkillScope, SkillTargetArgs, SkillWriteArgs, TriageAction,
    TriageSuggestionAction, TriageSuggestionSetArgs,
};
use agent_first_data::{BuiltCliSpec, CliValue, LogFilters, ResolvedInvocation};

pub type ActionHandler = fn(&ResolvedInvocation) -> Result<Command, ArgumentError>;

/// Every action id the registry declares, bound to the one builder that turns a
/// resolved invocation into a command.
///
/// The ids come from the registry rather than a second hand-written list: with
/// a surface this size the copy would be the only thing that could drift.
pub fn action_handlers(cli: &BuiltCliSpec) -> Vec<(String, ActionHandler)> {
    let mut actions: Vec<String> = cli
        .spec()
        .commands
        .iter()
        .flat_map(|command| command.combinations.iter())
        .map(|combination| combination.action_id.clone())
        .collect();
    actions.sort();
    actions.dedup();
    actions
        .into_iter()
        .map(|action| (action, build_command as ActionHandler))
        .collect()
}

/// A fact about one argument's value that the registry cannot type.
#[derive(Clone, Debug)]
pub struct ArgumentError {
    pub message: String,
    pub hint: String,
}

impl ArgumentError {
    fn new(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: hint.into(),
        }
    }
}

/// afmail's own cross-command arguments, read off the invocation that carried
/// them. A command that does not accept one simply never produces it.
pub struct Globals {
    pub workspace: Option<String>,
    pub log: LogFilters,
    pub request_id: Option<String>,
}

pub fn globals(invocation: &ResolvedInvocation) -> Globals {
    Globals {
        workspace: opt_text(invocation, "workspace"),
        log: LogFilters::new(strings(invocation, "log")),
        request_id: opt_text(invocation, "request_id"),
    }
}

/// A value the matched shape declares as required or fixed.
///
/// Reading it cannot fail: the shape that matched supplies every id it
/// declares. Asking for an id it does not declare is a defect in this file, not
/// a value the caller omitted.
fn text(invocation: &ResolvedInvocation, id: &str) -> String {
    invocation
        .required(id)
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn opt_text(invocation: &ResolvedInvocation, id: &str) -> Option<String> {
    invocation
        .optional(id)
        .and_then(CliValue::as_str)
        .map(str::to_string)
}

/// Read a credential the caller named a source for.
///
/// The registry refused an unacceptable source while resolving argv; what can
/// still fail here is the read — an unset variable, an unreadable file — and
/// that is an argument error too. `read_secret` keeps whatever went wrong out
/// of the message.
fn read_secret_arg(
    invocation: &ResolvedInvocation,
    id: &str,
) -> Result<Option<String>, ArgumentError> {
    let Some(raw) = opt_text(invocation, id) else {
        return Ok(None);
    };
    let flag = format!("--{}", id.replace('_', "-"));
    super::spec::secret_sources()
        .parse(&raw)
        .and_then(|source| source.read_secret())
        .map(|secret| Some(secret.expose_secret().to_string()))
        .map_err(|error| {
            ArgumentError::new(
                format!("{flag} {error}"),
                format!("run `afmail api serve --help` and check {flag}"),
            )
        })
}

fn flag(invocation: &ResolvedInvocation, id: &str) -> bool {
    invocation
        .optional(id)
        .and_then(CliValue::as_bool)
        .unwrap_or(false)
}

fn strings(invocation: &ResolvedInvocation, id: &str) -> Vec<String> {
    invocation
        .repeated(id)
        .iter()
        .filter_map(CliValue::as_str)
        .map(str::to_string)
        .collect()
}

/// A whole number the registry can only carry as `i64`.
fn bounded(
    invocation: &ResolvedInvocation,
    id: &str,
    flag_name: &str,
    max: u64,
) -> Result<Option<u64>, ArgumentError> {
    let Some(value) = invocation.optional(id).and_then(CliValue::as_i64) else {
        return Ok(None);
    };
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= max)
        .map(Some)
        .ok_or_else(|| {
            ArgumentError::new(
                format!("invalid value for `{flag_name}`: expected 0 through {max}"),
                format!("pass a whole number from 0 through {max}"),
            )
        })
}

fn bounded_usize(
    invocation: &ResolvedInvocation,
    id: &str,
    flag_name: &str,
) -> Result<Option<usize>, ArgumentError> {
    let value = bounded(invocation, id, flag_name, usize::MAX as u64)?;
    Ok(value.map(|value| value as usize))
}

fn bounded_u16(
    invocation: &ResolvedInvocation,
    id: &str,
    flag_name: &str,
) -> Result<Option<u16>, ArgumentError> {
    Ok(bounded(invocation, id, flag_name, u64::from(u16::MAX))?.map(|value| value as u16))
}

fn review_mode(invocation: &ResolvedInvocation) -> ReviewMode {
    match opt_text(invocation, "mode").as_deref() {
        Some("lan") => ReviewMode::Lan,
        _ => ReviewMode::Local,
    }
}

/// Message ids for a disposition that also accepts an idempotency key.
///
/// A request id names one stored result, and a batch produces one per message,
/// so the two only agree on a single id. That is a count, which no argument
/// type expresses.
fn disposition_ids(invocation: &ResolvedInvocation) -> Result<Vec<String>, ArgumentError> {
    let ids = strings(invocation, "message_id");
    if invocation.was_explicit("request_id") && ids.len() != 1 {
        return Err(ArgumentError::new(
            "invalid value for `--request-id`: an idempotency key covers exactly one message id",
            "pass one message id per --request-id, or drop --request-id to run the batch",
        ));
    }
    Ok(ids)
}

fn skill_target(invocation: &ResolvedInvocation) -> SkillTargetArgs {
    SkillTargetArgs {
        agent: match opt_text(invocation, "agent").as_deref() {
            Some("codex") => SkillAgentSelection::Codex,
            Some("claude-code") => SkillAgentSelection::ClaudeCode,
            Some("opencode") => SkillAgentSelection::Opencode,
            Some("hermes") => SkillAgentSelection::Hermes,
            _ => SkillAgentSelection::All,
        },
        scope: match opt_text(invocation, "scope").as_deref() {
            Some("workspace") => SkillScope::Workspace,
            _ => SkillScope::Personal,
        },
        skills_dir: opt_text(invocation, "skills_dir"),
    }
}

fn skill_write(invocation: &ResolvedInvocation) -> SkillWriteArgs {
    SkillWriteArgs {
        target: skill_target(invocation),
        force: flag(invocation, "force"),
    }
}

fn confidence(invocation: &ResolvedInvocation) -> Result<Option<u8>, ArgumentError> {
    Ok(bounded(
        invocation,
        "confidence_percent",
        "--confidence-percent",
        100,
    )?
    .map(|value| value as u8))
}

fn message_suggestion(invocation: &ResolvedInvocation) -> MessageSuggestionAction {
    match opt_text(invocation, "action").as_deref() {
        Some("case-add") => MessageSuggestionAction::CaseAdd {
            case_ref: text(invocation, "case_ref"),
        },
        Some("notification-create") => MessageSuggestionAction::NotificationCreate {
            name: text(invocation, "name"),
            summary: text(invocation, "summary"),
        },
        Some("notification-add") => MessageSuggestionAction::NotificationAdd {
            notification_ref: text(invocation, "notification_ref"),
            summary: text(invocation, "summary"),
        },
        Some("spam") => MessageSuggestionAction::Spam,
        Some("trash") => MessageSuggestionAction::Trash,
        _ => MessageSuggestionAction::CaseCreate {
            name: text(invocation, "name"),
            group: text(invocation, "group"),
        },
    }
}

fn case_suggestion(invocation: &ResolvedInvocation) -> CaseSuggestionActionKind {
    match opt_text(invocation, "action").as_deref() {
        Some("unarchive") => CaseSuggestionActionKind::Unarchive {
            group: text(invocation, "group"),
        },
        Some("move") => CaseSuggestionActionKind::Move {
            group: text(invocation, "group"),
        },
        Some("rename") => CaseSuggestionActionKind::Rename {
            name: text(invocation, "name"),
        },
        Some("tag") => CaseSuggestionActionKind::Tag {
            tag: text(invocation, "tag"),
        },
        Some("untag") => CaseSuggestionActionKind::Untag {
            tag: text(invocation, "tag"),
        },
        Some("trash") => CaseSuggestionActionKind::Trash,
        Some("delete") => CaseSuggestionActionKind::Delete {
            allow_notes: flag(invocation, "allow_notes"),
        },
        _ => CaseSuggestionActionKind::Archive,
    }
}

fn notification_suggestion(invocation: &ResolvedInvocation) -> NotificationSuggestionActionKind {
    match opt_text(invocation, "action").as_deref() {
        Some("delete") => NotificationSuggestionActionKind::Delete {
            allow_notes: flag(invocation, "allow_notes"),
        },
        _ => NotificationSuggestionActionKind::Rename {
            name: text(invocation, "name"),
        },
    }
}

fn case(action: CaseCommand) -> Command {
    Command::Case { action }
}

fn case_draft(action: CaseDraftAction) -> Command {
    Command::Case {
        action: CaseCommand::Draft { action },
    }
}

fn contact(action: ContactCommand) -> Command {
    Command::Contact { action }
}

fn notification(action: NotificationCommand) -> Command {
    Command::Notification { action }
}

fn message(action: MessageAction) -> Command {
    Command::Message { action }
}

/// The registry's action id, resolved to the command it names.
///
/// Every action id in the registry has an arm here; the registry's own
/// synthetic invocations prove it in `super::spec`'s tests, so the fallback
/// exists only to keep this total.
pub fn build_command(invocation: &ResolvedInvocation) -> Result<Command, ArgumentError> {
    let action = invocation.action_id();
    let command = match action {
        "init" => Command::Init {
            path: opt_text(invocation, "path"),
        },
        "pull" => Command::Pull {
            ids: strings(invocation, "ids"),
            dry_run: flag(invocation, "dry_run"),
            since_days: bounded(
                invocation,
                "since_days",
                "--since-days",
                u64::from(u32::MAX),
            )?
            .map(|value| value as u32),
            max_new_messages: bounded_usize(invocation, "max_new_messages", "--max-new-messages")?,
            max_total_bytes: bounded(invocation, "max_total_bytes", "--max-total-bytes", u64::MAX)?,
            all_history: flag(invocation, "all_history"),
        },
        "status" => Command::Status,
        "push" => Command::Push {
            dry_run: flag(invocation, "dry_run"),
            confirm: opt_text(invocation, "confirm"),
            action: None,
        },
        "push_list" => Command::Push {
            dry_run: true,
            confirm: None,
            action: Some(PushAction::List),
        },
        "doctor" => Command::Doctor { action: None },
        "doctor_repair" => Command::Doctor {
            action: Some(DoctorAction::Repair),
        },
        "purge" | "purge_spam" | "purge_trash" | "purge_deleted" => Command::Purge {
            older_than_days: bounded(invocation, "older_than_days", "--older-than-days", u64::MAX)?,
            confirm: opt_text(invocation, "confirm"),
            action: match action {
                "purge_spam" => Some(PurgeAction::Spam),
                "purge_trash" => Some(PurgeAction::Trash),
                "purge_deleted" => Some(PurgeAction::Deleted),
                _ => None,
            },
        },
        "config_show" => Command::Config {
            action: ConfigAction::Show,
        },
        "config_get" => Command::Config {
            action: ConfigAction::Get {
                key: text(invocation, "key"),
            },
        },
        "config_set" => Command::Config {
            action: ConfigAction::Set {
                key: text(invocation, "key"),
                values: strings(invocation, "values"),
            },
        },
        "config_add" => Command::Config {
            action: ConfigAction::Add {
                key: text(invocation, "key"),
                slug: text(invocation, "slug"),
                fields: strings(invocation, "fields"),
            },
        },
        "config_remove" => Command::Config {
            action: ConfigAction::Remove {
                key: text(invocation, "key"),
                slug: text(invocation, "slug"),
            },
        },
        "remote_test" => Command::Remote {
            action: RemoteAction::Test,
        },
        "remote_folders" => Command::Remote {
            action: RemoteAction::Folders,
        },
        "skill_status" => Command::Skill {
            action: SkillAction::Status(skill_target(invocation)),
        },
        "skill_install" => Command::Skill {
            action: SkillAction::Install(skill_write(invocation)),
        },
        "skill_uninstall" => Command::Skill {
            action: SkillAction::Uninstall(skill_write(invocation)),
        },
        "triage_list" => Command::Triage {
            action: TriageAction::List {
                limit: bounded_usize(invocation, "limit", "--limit")?,
                details: flag(invocation, "details"),
            },
        },
        "triage_suggestion_set" => Command::Triage {
            action: TriageAction::Suggestion {
                action: TriageSuggestionAction::Set(TriageSuggestionSetArgs {
                    message_id: text(invocation, "message_id"),
                    message_review_base_hash: text(invocation, "message_review_base_hash"),
                    reason: text(invocation, "reason"),
                    confidence_percent: confidence(invocation)?,
                    action: message_suggestion(invocation),
                }),
            },
        },
        "triage_suggestion_clear" => Command::Triage {
            action: TriageAction::Suggestion {
                action: TriageSuggestionAction::Clear {
                    message_id: text(invocation, "message_id"),
                    agent_suggestion_uid: text(invocation, "agent_suggestion_uid"),
                },
            },
        },
        "triage_review" => Command::Triage {
            action: TriageAction::Review {
                message_id: strings(invocation, "message_id"),
                mode: review_mode(invocation),
            },
        },
        "message_show" => message(MessageAction::Show {
            message_id: text(invocation, "message_id"),
        }),
        "message_list" => message(MessageAction::List {
            sender: opt_text(invocation, "sender"),
            subject_contains: opt_text(invocation, "subject_contains"),
            since: opt_text(invocation, "since"),
            scope: match opt_text(invocation, "in").as_deref() {
                Some("triage") => MessageListScope::Triage,
                Some("case") => MessageListScope::Case,
                Some("notification") => MessageListScope::Notification,
                _ => MessageListScope::All,
            },
        }),
        "message_spam" => message(MessageAction::Spam {
            message_id: disposition_ids(invocation)?,
            reason: opt_text(invocation, "reason"),
        }),
        "message_trash" => message(MessageAction::Trash {
            message_id: disposition_ids(invocation)?,
            reason: opt_text(invocation, "reason"),
        }),
        "message_unspam" => message(MessageAction::Unspam {
            message_id: disposition_ids(invocation)?,
            reason: opt_text(invocation, "reason"),
        }),
        "message_untrash" => message(MessageAction::Untrash {
            message_id: disposition_ids(invocation)?,
            reason: opt_text(invocation, "reason"),
        }),
        "message_attachment_fetch" => message(MessageAction::Attachment {
            action: MessageAttachmentAction::Fetch {
                message_id: text(invocation, "message_id"),
                part_id: opt_text(invocation, "part_id"),
            },
        }),
        "case_create" => case(CaseCommand::Create(CaseCreateArgs {
            name: text(invocation, "name"),
            group: opt_text(invocation, "group"),
            message: opt_text(invocation, "message"),
            summary: opt_text(invocation, "summary"),
            reason: opt_text(invocation, "reason"),
        })),
        "case_list" => case(CaseCommand::List {
            archived: flag(invocation, "archived"),
            all: flag(invocation, "all"),
        }),
        "case_show" => case(CaseCommand::Show {
            case_ref: text(invocation, "case_ref"),
        }),
        "case_review" => case(CaseCommand::Review {
            case_ref: text(invocation, "case_ref"),
            mode: review_mode(invocation),
        }),
        "case_suggestion_set" => case(CaseCommand::Suggestion {
            action: CaseSuggestionAction::Set(CaseSuggestionSetArgs {
                case_ref: text(invocation, "case_ref"),
                case_review_base_hash: text(invocation, "case_review_base_hash"),
                reason: text(invocation, "reason"),
                confidence_percent: confidence(invocation)?,
                action: case_suggestion(invocation),
            }),
        }),
        "case_suggestion_clear" => case(CaseCommand::Suggestion {
            action: CaseSuggestionAction::Clear {
                case_ref: text(invocation, "case_ref"),
                agent_suggestion_uid: text(invocation, "agent_suggestion_uid"),
            },
        }),
        "case_add" => case(CaseCommand::Add {
            case_ref: text(invocation, "case_ref"),
            message_id: disposition_ids(invocation)?,
            summary: opt_text(invocation, "summary"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_set_summary" => case(CaseCommand::SetSummary {
            case_ref: text(invocation, "case_ref"),
            message_id: text(invocation, "message_id"),
            summary: text(invocation, "summary"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_remove" => case(CaseCommand::Remove {
            case_ref: text(invocation, "case_ref"),
            message_id: text(invocation, "message_id"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_move" => case(CaseCommand::Move {
            case_ref: text(invocation, "case_ref"),
            group: text(invocation, "group"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_rename" => case(CaseCommand::Rename {
            case_ref: text(invocation, "case_ref"),
            name: text(invocation, "name"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_archive" => case(CaseCommand::Archive {
            case_ref: text(invocation, "case_ref"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_unarchive" => case(CaseCommand::Unarchive {
            case_ref: text(invocation, "case_ref"),
            group: text(invocation, "group"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_trash" => case(CaseCommand::Trash {
            case_ref: text(invocation, "case_ref"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_delete" => case(CaseCommand::Delete {
            case_ref: text(invocation, "case_ref"),
            allow_notes: flag(invocation, "allow_notes"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_tag" => case(CaseCommand::Tag {
            case_ref: text(invocation, "case_ref"),
            tag: text(invocation, "tag"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_untag" => case(CaseCommand::Untag {
            case_ref: text(invocation, "case_ref"),
            tag: text(invocation, "tag"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_merge" => case(CaseCommand::Merge {
            case_ref: text(invocation, "case_ref"),
            other_case_ref: text(invocation, "other_case_ref"),
            reason: opt_text(invocation, "reason"),
        }),
        "case_notes_show" => case(CaseCommand::Notes {
            action: CaseNotesAction::Show {
                case_ref: text(invocation, "case_ref"),
            },
        }),
        "case_notes_append" => case(CaseCommand::Notes {
            action: CaseNotesAction::Append {
                case_ref: text(invocation, "case_ref"),
                text: text(invocation, "text"),
            },
        }),
        "case_notes_replace" => case(CaseCommand::Notes {
            action: CaseNotesAction::Replace {
                case_ref: text(invocation, "case_ref"),
                text: text(invocation, "text"),
            },
        }),
        "case_draft_new" => case_draft(CaseDraftAction::New {
            case_ref: text(invocation, "case_ref"),
            to: strings(invocation, "to"),
            cc: strings(invocation, "cc"),
            bcc: strings(invocation, "bcc"),
            subject: text(invocation, "subject"),
            identity: opt_text(invocation, "identity"),
            body: opt_text(invocation, "body"),
            body_file: opt_text(invocation, "body_file"),
        }),
        "case_draft_reply" => case_draft(CaseDraftAction::Reply {
            case_ref: text(invocation, "case_ref"),
            message_id: text(invocation, "message_id"),
            bcc: strings(invocation, "bcc"),
            identity: opt_text(invocation, "identity"),
            body: opt_text(invocation, "body"),
            body_file: opt_text(invocation, "body_file"),
            all: flag(invocation, "all"),
        }),
        "case_draft_change" => case_draft(CaseDraftAction::Change {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
            subject: opt_text(invocation, "subject"),
            to: strings(invocation, "to"),
            cc: strings(invocation, "cc"),
            clear_cc: flag(invocation, "clear_cc"),
            bcc: strings(invocation, "bcc"),
            clear_bcc: flag(invocation, "clear_bcc"),
            identity: opt_text(invocation, "identity"),
            body: opt_text(invocation, "body"),
            body_file: opt_text(invocation, "body_file"),
        }),
        "case_draft_show" => case_draft(CaseDraftAction::Show {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
        }),
        "case_draft_review" => case_draft(CaseDraftAction::Review {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
            mode: review_mode(invocation),
        }),
        "case_draft_validate" => case_draft(CaseDraftAction::Validate {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
        }),
        "case_draft_list" => case_draft(CaseDraftAction::List {
            case_ref: text(invocation, "case_ref"),
        }),
        "case_draft_upload" => case_draft(CaseDraftAction::Upload {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
        }),
        "case_draft_send" => case_draft(CaseDraftAction::Send {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
        }),
        "case_draft_attach" => case_draft(CaseDraftAction::Attach {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
            path: text(invocation, "path"),
        }),
        "case_draft_remove" => case_draft(CaseDraftAction::Remove {
            case_ref: text(invocation, "case_ref"),
            draft_name: text(invocation, "draft_name"),
            reason: opt_text(invocation, "reason"),
        }),
        "contact_create" => contact(ContactCommand::Create(ContactCreateArgs {
            name: text(invocation, "name"),
            group: opt_text(invocation, "group"),
            emails: strings(invocation, "email"),
            phones: strings(invocation, "phone"),
            org: opt_text(invocation, "org"),
            role: opt_text(invocation, "role"),
            tags: strings(invocation, "tag"),
            reason: opt_text(invocation, "reason"),
        })),
        "contact_list" => contact(ContactCommand::List {
            group: opt_text(invocation, "group"),
            tag: opt_text(invocation, "tag"),
            org: opt_text(invocation, "org"),
            role: opt_text(invocation, "role"),
        }),
        "contact_show" => contact(ContactCommand::Show {
            contact_ref: text(invocation, "contact_ref"),
        }),
        "contact_move" => contact(ContactCommand::Move {
            contact_ref: text(invocation, "contact_ref"),
            group: text(invocation, "group"),
            reason: opt_text(invocation, "reason"),
        }),
        "contact_rename" => contact(ContactCommand::Rename {
            contact_ref: text(invocation, "contact_ref"),
            name: text(invocation, "name"),
            reason: opt_text(invocation, "reason"),
        }),
        "contact_email_add" => contact(ContactCommand::Email {
            action: ContactEmailAction::Add {
                contact_ref: text(invocation, "contact_ref"),
                email: text(invocation, "email"),
                reason: opt_text(invocation, "reason"),
            },
        }),
        "contact_email_remove" => contact(ContactCommand::Email {
            action: ContactEmailAction::Remove {
                contact_ref: text(invocation, "contact_ref"),
                email: text(invocation, "email"),
                reason: opt_text(invocation, "reason"),
            },
        }),
        "contact_phone_add" => contact(ContactCommand::Phone {
            action: ContactPhoneAction::Add {
                contact_ref: text(invocation, "contact_ref"),
                phone: text(invocation, "phone"),
                reason: opt_text(invocation, "reason"),
            },
        }),
        "contact_phone_remove" => contact(ContactCommand::Phone {
            action: ContactPhoneAction::Remove {
                contact_ref: text(invocation, "contact_ref"),
                phone: text(invocation, "phone"),
                reason: opt_text(invocation, "reason"),
            },
        }),
        "contact_tag" => contact(ContactCommand::Tag {
            contact_ref: text(invocation, "contact_ref"),
            tag: text(invocation, "tag"),
            reason: opt_text(invocation, "reason"),
        }),
        "contact_untag" => contact(ContactCommand::Untag {
            contact_ref: text(invocation, "contact_ref"),
            tag: text(invocation, "tag"),
            reason: opt_text(invocation, "reason"),
        }),
        "contact_delete" => contact(ContactCommand::Delete {
            contact_ref: text(invocation, "contact_ref"),
            allow_notes: flag(invocation, "allow_notes"),
            reason: opt_text(invocation, "reason"),
        }),
        "contact_extract" => contact(ContactCommand::Extract {
            from_triage: flag(invocation, "from_triage"),
            from_case: opt_text(invocation, "from_case"),
            all: flag(invocation, "all"),
            group: opt_text(invocation, "group"),
        }),
        "contact_notes_show" => contact(ContactCommand::Notes {
            action: ContactNotesAction::Show {
                contact_ref: text(invocation, "contact_ref"),
            },
        }),
        "contact_notes_append" => contact(ContactCommand::Notes {
            action: ContactNotesAction::Append {
                contact_ref: text(invocation, "contact_ref"),
                text: text(invocation, "text"),
            },
        }),
        "contact_notes_replace" => contact(ContactCommand::Notes {
            action: ContactNotesAction::Replace {
                contact_ref: text(invocation, "contact_ref"),
                text: text(invocation, "text"),
            },
        }),
        "notification_create" => {
            notification(NotificationCommand::Create(NotificationCreateArgs {
                name: text(invocation, "name"),
                message: opt_text(invocation, "message"),
                summary: opt_text(invocation, "summary"),
                reason: opt_text(invocation, "reason"),
            }))
        }
        "notification_add" => notification(NotificationCommand::Add {
            notification_ref: text(invocation, "notification_ref"),
            message_id: disposition_ids(invocation)?,
            summary: text(invocation, "summary"),
            reason: opt_text(invocation, "reason"),
        }),
        "notification_list" => notification(NotificationCommand::List),
        "notification_show" => notification(NotificationCommand::Show {
            notification_ref: text(invocation, "notification_ref"),
        }),
        "notification_review" => notification(NotificationCommand::Review {
            notification_ref: text(invocation, "notification_ref"),
            mode: review_mode(invocation),
        }),
        "notification_suggestion_set" => notification(NotificationCommand::Suggestion {
            action: NotificationSuggestionAction::Set(NotificationSuggestionSetArgs {
                notification_ref: text(invocation, "notification_ref"),
                notification_review_base_hash: text(invocation, "notification_review_base_hash"),
                reason: text(invocation, "reason"),
                confidence_percent: confidence(invocation)?,
                action: notification_suggestion(invocation),
            }),
        }),
        "notification_suggestion_clear" => notification(NotificationCommand::Suggestion {
            action: NotificationSuggestionAction::Clear {
                notification_ref: text(invocation, "notification_ref"),
                agent_suggestion_uid: text(invocation, "agent_suggestion_uid"),
            },
        }),
        "notification_remove" => notification(NotificationCommand::Remove {
            notification_ref: text(invocation, "notification_ref"),
            message_id: text(invocation, "message_id"),
            reason: opt_text(invocation, "reason"),
        }),
        "notification_move" => notification(NotificationCommand::Move {
            notification_ref: text(invocation, "notification_ref"),
            message_id: text(invocation, "message_id"),
            new_notification_ref: text(invocation, "new_notification_ref"),
            reason: opt_text(invocation, "reason"),
        }),
        "notification_rename" => notification(NotificationCommand::Rename {
            notification_ref: text(invocation, "notification_ref"),
            name: text(invocation, "name"),
            reason: opt_text(invocation, "reason"),
        }),
        "notification_delete" => notification(NotificationCommand::Delete {
            notification_ref: text(invocation, "notification_ref"),
            allow_notes: flag(invocation, "allow_notes"),
            reason: opt_text(invocation, "reason"),
        }),
        "notification_set_summary" => notification(NotificationCommand::SetSummary {
            notification_ref: text(invocation, "notification_ref"),
            message_id: text(invocation, "message_id"),
            summary: text(invocation, "summary"),
            reason: opt_text(invocation, "reason"),
        }),
        "notification_notes_show" => notification(NotificationCommand::Notes {
            action: NotificationNotesAction::Show {
                notification_ref: text(invocation, "notification_ref"),
            },
        }),
        "notification_notes_append" => notification(NotificationCommand::Notes {
            action: NotificationNotesAction::Append {
                notification_ref: text(invocation, "notification_ref"),
                text: text(invocation, "text"),
            },
        }),
        "notification_notes_replace" => notification(NotificationCommand::Notes {
            action: NotificationNotesAction::Replace {
                notification_ref: text(invocation, "notification_ref"),
                text: text(invocation, "text"),
            },
        }),
        "render_refresh" => Command::Render {
            action: RenderAction::Refresh,
        },
        "render_templates" => Command::Render {
            action: RenderAction::Templates {
                force: flag(invocation, "force"),
            },
        },
        "log_list" => Command::Log {
            action: LogAction::List {
                limit: bounded_usize(invocation, "limit", "--limit")?.unwrap_or(50),
            },
        },
        "log_tail" => Command::Log {
            action: LogAction::Tail,
        },
        "log_message" => Command::Log {
            action: LogAction::Message {
                message_id: text(invocation, "message_id"),
            },
        },
        "log_case" => Command::Log {
            action: LogAction::Case {
                case_uid: text(invocation, "case_uid"),
            },
        },
        "log_notification" => Command::Log {
            action: LogAction::Notification {
                notification_uid: text(invocation, "notification_uid"),
            },
        },
        "api_serve" => Command::Api {
            action: ApiAction::Serve {
                mode: match opt_text(invocation, "mode").as_deref() {
                    Some("lan") => ApiMode::Lan,
                    _ => ApiMode::Local,
                },
                port: bounded_u16(invocation, "port", "--port")?.unwrap_or(9417),
                access_token_secret: read_secret_arg(invocation, "access_token_secret")?,
            },
        },
        "api_export" => Command::Api {
            action: ApiAction::Export {
                directory: opt_text(invocation, "directory").unwrap_or_default(),
                force: flag(invocation, "force"),
            },
        },
        "demo_init" => Command::Demo {
            action: DemoAction::Init {
                path: opt_text(invocation, "path").unwrap_or_default(),
                scenario: DemoScenario::FounderInbox,
                imap_port: bounded_u16(invocation, "imap_port", "--imap-port")?.unwrap_or(14300),
                smtp_port: bounded_u16(invocation, "smtp_port", "--smtp-port")?.unwrap_or(14301),
            },
        },
        "demo_serve" => Command::Demo {
            action: DemoAction::Serve {
                verbose: flag(invocation, "verbose"),
            },
        },
        unknown => {
            return Err(ArgumentError::new(
                format!("registered action `{unknown}` has no command builder"),
                "this is a bug in afmail's command registry",
            ));
        }
    };
    Ok(command)
}
