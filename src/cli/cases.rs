use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum CaseCommand {
    /// Create a new case and return its stable UID/ref.
    Create(CaseCreateArgs),
    /// List compact active case locators.
    List,
    /// Show the active case's case.md without changing workspace state.
    Show {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
    },
    /// Add a message to this existing case.
    Add {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Message id to add.
        message_id: String,
        /// Why this message belongs in this case; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Move this case to another group.
    Move {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Destination group.
        group: String,
    },
    /// Rename this active case's human-readable name without changing its UID.
    Rename {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// New human-readable case name.
        #[arg(long)]
        name: String,
        /// Why this name better represents the case; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show or edit active case notes.
    Notes {
        #[command(subcommand)]
        action: CaseNotesAction,
    },
    /// Archive this active case.
    Archive {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Why this case is ready to archive; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Reopen this case as active work without changing its tags.
    Reopen {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Why this case should be reopened; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Add a case organization tag.
    Tag {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Case organization tag.
        tag: String,
        /// Why this tag is useful; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove a case organization tag.
    Untag {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Case organization tag.
        tag: String,
        /// Why this tag should be removed; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Create, validate, attach to, or remove local case drafts.
    Draft {
        #[command(subcommand)]
        action: CaseDraftAction,
    },
    /// Compose an existing case draft into the local push queue.
    Compose {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Draft markdown file under the case drafts directory.
        draft_name: String,
    },
    /// Scaffold a reply draft to a message, prefilled and quoting the original.
    Reply {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Message id in this case to reply to.
        message_id: String,
        /// Reply to all original recipients (To and Cc), not just the sender.
        #[arg(long)]
        all: bool,
    },
    /// Merge another case into this case.
    Merge {
        /// Primary case ref.
        case_ref: String,
        /// Case ref to merge into the primary case.
        other_case_ref: String,
        /// Why these cases should be merged; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Args, Debug, Clone)]
pub struct CaseCreateArgs {
    /// Human-readable case name used in case.md and the directory suffix.
    #[arg(long)]
    pub name: String,
    /// Destination group. Defaults to the configured default group.
    #[arg(long)]
    pub group: Option<String>,
    /// Optional first message to add to the case.
    #[arg(long)]
    pub message: Option<String>,
    /// Why this case is being created.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum CaseNotesAction {
    /// Show notes markdown.
    Show {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
    },
    /// Append text to notes markdown.
    Append {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Markdown text to append.
        #[arg(long)]
        text: String,
    },
    /// Replace notes markdown with text.
    Replace {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Markdown text to write.
        #[arg(long)]
        text: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum CaseDraftAction {
    /// Scaffold a new outbound draft (not a reply) in this case.
    New {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Recipient address. Repeatable.
        #[arg(long = "to")]
        to: Vec<String>,
        /// Cc address. Repeatable.
        #[arg(long = "cc")]
        cc: Vec<String>,
        /// Draft subject.
        #[arg(long)]
        subject: Option<String>,
    },
    /// Validate a draft under the case drafts directory.
    Validate {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Draft markdown file under the case drafts directory.
        draft_name: String,
    },
    /// Copy or reference a file and add it to a draft's attachments.
    Attach {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Draft markdown file under the case drafts directory.
        draft_name: String,
        /// Local file path to attach.
        path: String,
    },
    /// Remove a local draft and any queued outbound item for it.
    Remove {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Draft markdown file under the case drafts directory.
        draft_name: String,
        /// Why this draft should be removed; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
}
