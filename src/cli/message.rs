#[derive(Debug)]
pub enum MessageAction {
    /// Show the exact typed message document used by Markdown and Review templates.
    Show { message_id: String },
    /// List messages by sender, subject, date, or workspace scope.
    List {
        sender: Option<String>,
        subject_contains: Option<String>,
        since: Option<String>,
        scope: MessageListScope,
    },
    /// Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move.
    Spam {
        message_id: Vec<String>,
        reason: Option<String>,
    },
    /// Explicitly discard this message locally, show it under trash/, and queue a Trash move.
    Trash {
        message_id: Vec<String>,
        reason: Option<String>,
    },
    /// Mark a spam false positive as triage and queue a move out of Junk when needed.
    Unspam {
        message_id: Vec<String>,
        reason: Option<String>,
    },
    /// Mark a trash false positive as triage and queue a move out of Trash when needed.
    Untrash {
        message_id: Vec<String>,
        reason: Option<String>,
    },
    /// Fetch an attachment for this message.
    Attachment { action: MessageAttachmentAction },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Debug)]
pub enum MessageAttachmentAction {
    /// Fetch one attachment by MIME part id, or all attachments when omitted.
    Fetch {
        message_id: String,
        part_id: Option<String>,
    },
}
