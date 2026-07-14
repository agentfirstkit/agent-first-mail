use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum NotificationCommand {
    /// Create a notification collection and optionally file one message.
    Create(NotificationCreateArgs),
    /// File an existing message into this notification collection.
    Add {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        /// Message ids to file.
        #[arg(required = true)]
        message_id: Vec<String>,
        /// Human/agent-authored summary for this notification entry.
        #[arg(long)]
        summary: String,
        /// Why this message belongs in this notification; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// List compact notification collections.
    List,
    /// Show notification collection index and entries.
    Show {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
    },
    /// Remove a notification message from this collection and return it to triage.
    Remove {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        message_id: String,
        /// Why this message does not belong in this notification; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Move a notification message to another notification collection.
    Move {
        /// Source notification collection ref.
        notification_ref: String,
        message_id: String,
        /// Destination notification collection ref.
        new_notification_ref: String,
        /// Why this collection is better; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Rename this notification collection's human-readable name without changing its UID.
    Rename {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        /// New human-readable notification name.
        #[arg(long)]
        name: String,
        /// Why this name better represents the collection; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Delete an empty notification collection.
    Delete {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        /// Also delete notes.md when it contains user-authored notes.
        #[arg(long)]
        allow_notes: bool,
        /// Why this notification collection should be deleted; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Set or replace one notification entry summary.
    SetSummary {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        message_id: String,
        #[arg(long)]
        summary: String,
        /// Why this summary is useful; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show or edit notification collection notes.
    Notes {
        #[command(subcommand)]
        action: NotificationNotesAction,
    },
}

#[derive(Args, Debug, Clone)]
pub struct NotificationCreateArgs {
    /// Human-readable notification name used in metadata and the directory suffix.
    #[arg(long)]
    pub name: String,
    /// Optional message to immediately file into the new notification collection.
    #[arg(long)]
    pub message: Option<String>,
    /// Required when --message is supplied.
    #[arg(long)]
    pub summary: Option<String>,
    /// Why this notification collection is being created.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum NotificationNotesAction {
    /// Show notes markdown.
    Show {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
    },
    /// Append text to notes markdown.
    Append {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        /// Markdown text to append.
        #[arg(long)]
        text: String,
    },
    /// Replace notes markdown with text.
    Replace {
        /// Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix.
        notification_ref: String,
        /// Markdown text to write.
        #[arg(long)]
        text: String,
    },
}
