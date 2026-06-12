use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum ArchiveAction {
    /// List compact archived cases and/or direct-message archive categories.
    List {
        #[command(subcommand)]
        target: Option<ArchiveListAction>,
    },
    /// Operate on a direct-message archive category.
    Message {
        #[command(subcommand)]
        action: ArchiveMessageCommand,
    },
    /// Operate on an archived case.
    Case {
        #[command(subcommand)]
        action: ArchiveCaseAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ArchiveListAction {
    /// List compact archived cases.
    Cases,
    /// List compact direct-message archive categories.
    Messages,
}

#[derive(Subcommand, Debug)]
pub enum ArchiveMessageCommand {
    /// Create a direct-message archive category and optionally file one message.
    Create(ArchiveMessageCreateArgs),
    /// Show archive category index and entries.
    Show {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
    },
    /// Restore a direct archived message to triage.
    Restore {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
        message_id: String,
        /// Why this message needs active triage again; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Move a direct archived message to another archive category.
    Move {
        /// Source direct-message archive category ref.
        archive_ref: String,
        message_id: String,
        /// Destination archive category ref.
        new_archive_ref: String,
        /// Why this category is better; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Rename this archive category's human-readable name without changing its UID.
    Rename {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
        /// New human-readable archive name.
        #[arg(long)]
        name: String,
        /// Why this name better represents the category; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Set or replace one direct archive entry summary.
    SetSummary {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
        message_id: String,
        #[arg(long)]
        summary: String,
        /// Why this summary is useful; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show or edit archive category notes.
    Notes {
        #[command(subcommand)]
        action: ArchiveMessageNotesAction,
    },
}

#[derive(Args, Debug, Clone)]
pub struct ArchiveMessageCreateArgs {
    /// Human-readable archive category name used in metadata and the directory suffix.
    #[arg(long)]
    pub name: String,
    /// Optional message to immediately file into the new archive category.
    #[arg(long)]
    pub message: Option<String>,
    /// Required when --message is supplied.
    #[arg(long)]
    pub summary: Option<String>,
    /// Why this archive category is being created.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ArchiveCaseAction {
    /// Show the archived case.
    Show {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
    },
    /// Restore an archived case to an active case group.
    Restore {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Active case group to restore into.
        #[arg(long)]
        group: String,
        /// Why this case needs active attention again; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Rename this archived case's human-readable name without changing its UID.
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
    /// Show or edit archived case notes.
    Notes {
        #[command(subcommand)]
        action: ArchiveCaseNotesAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ArchiveMessageNotesAction {
    /// Show notes markdown.
    Show {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
    },
    /// Append text to notes markdown.
    Append {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
        /// Markdown text to append.
        #[arg(long)]
        text: String,
    },
    /// Replace notes markdown with text.
    Replace {
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
        /// Markdown text to write.
        #[arg(long)]
        text: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ArchiveCaseNotesAction {
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
