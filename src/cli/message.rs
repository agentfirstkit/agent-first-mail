use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum MessageAction {
    /// Show the full local message metadata, body text, and attachment metadata.
    Show {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
    },
    /// Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move.
    Spam {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this disposition is correct; required by default.
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
    /// Restore a message from spam, trash, or direct archive back to triage.
    Restore {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Why this restore is correct; required by default.
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
