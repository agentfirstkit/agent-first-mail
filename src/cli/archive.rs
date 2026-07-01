use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum ArchiveAction {
    /// List compact archived cases.
    List {
        #[command(subcommand)]
        target: Option<ArchiveListAction>,
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
    /// Move every message in this archived case to local trash and queue Trash moves.
    Trash {
        /// Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix.
        case_ref: String,
        /// Why this whole archived case should be discarded; required by default.
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
