use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum MessageAction {
    /// Show the full local message metadata, body text, and attachment metadata.
    Show {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
    },
    /// File this message into a direct-message archive category and queue eligible IMAP moves.
    Archive {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix.
        archive_ref: String,
        /// Human/agent-authored summary for this archive entry.
        #[arg(long)]
        summary: String,
        /// Why this disposition is correct; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move.
    Spam {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this disposition is correct; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Undo a local spam mark before queued remote effects are pushed.
    Unspam {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this spam mark should be undone; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Explicitly discard this message locally, show it under trash/, and queue a Trash move.
    Trash {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this disposition is correct; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Undo a local trash mark before queued remote effects are pushed.
    Untrash {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this trash mark should be undone; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Restore a directly archived message to triage.
    Unarchive {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this archive should be undone; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Fetch an attachment for this message.
    Attachment {
        #[command(subcommand)]
        action: MessageAttachmentAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum MessageAttachmentAction {
    /// Fetch one attachment by MIME part id, or all attachments when omitted.
    Fetch {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        part_id: Option<String>,
    },
}
