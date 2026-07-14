use clap::{Subcommand, ValueEnum};

#[derive(Subcommand, Debug)]
pub enum MessageAction {
    /// Show the full local message metadata, body text, and attachment metadata.
    Show {
        /// Message id, for example message_inbox_607146690_22.
        message_id: String,
        /// Show metadata only and omit large body/conversation fields.
        #[arg(long)]
        meta: bool,
    },
    /// List messages by sender, subject, date, or workspace scope.
    List {
        /// Only include messages whose sender or from line contains this text.
        #[arg(long)]
        sender: Option<String>,
        /// Only include messages whose subject contains this text.
        #[arg(long = "subject-contains")]
        subject_contains: Option<String>,
        /// Only include messages on or after this date or RFC3339 timestamp.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Limit results to one workspace scope.
        #[arg(long = "in", value_enum, default_value_t = MessageListScope::All)]
        scope: MessageListScope,
    },
    /// Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move.
    Spam {
        /// Message ids, for example message_inbox_607146690_22.
        #[arg(required = true)]
        message_id: Vec<String>,
        /// Why this disposition is correct; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Explicitly discard this message locally, show it under trash/, and queue a Trash move.
    Trash {
        /// Message ids, for example message_inbox_607146690_22.
        #[arg(required = true)]
        message_id: Vec<String>,
        /// Why this disposition is correct; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark a spam false positive as triage and queue a move out of Junk when needed.
    Unspam {
        /// Message ids, for example message_inbox_607146690_22.
        #[arg(required = true)]
        message_id: Vec<String>,
        /// Why this message is not spam; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark a trash false positive as triage and queue a move out of Trash when needed.
    Untrash {
        /// Message ids, for example message_inbox_607146690_22.
        #[arg(required = true)]
        message_id: Vec<String>,
        /// Why this message should not be trashed; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Fetch an attachment for this message.
    Attachment {
        #[command(subcommand)]
        action: MessageAttachmentAction,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum MessageListScope {
    Triage,
    Case,
    Notification,
    All,
}

impl MessageListScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Case => "case",
            Self::Notification => "notification",
            Self::All => "all",
        }
    }
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
