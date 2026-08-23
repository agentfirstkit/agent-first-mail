use agent_first_ui::UiDeliveryMode;

#[derive(Debug)]
pub enum ConfigAction {
    /// Show effective config after applying built-in defaults.
    Show,
    /// Get one config key.
    Get { key: String },
    /// Set one config key. Multiple values become an array for array keys.
    Set { key: String, values: Vec<String> },
    /// Add an element to a keyed list (e.g. `identities`). FIELDS are key=value.
    Add {
        key: String,
        slug: String,
        fields: Vec<String>,
    },
    /// Remove a keyed-list element by its slug.
    Remove { key: String, slug: String },
}

#[derive(Debug)]
pub enum RemoteAction {
    /// Test configured IMAP and SMTP access without sending mail.
    Test,
    /// List IMAP folders/mailboxes.
    Folders,
}

#[derive(Debug)]
pub enum TriageAction {
    /// List compact untriaged message locators.
    List { limit: Option<usize>, details: bool },
    /// Set or clear the current typed Agent suggestion for a message.
    Suggestion { action: TriageSuggestionAction },
    /// Open a review session for persisted triage suggestions.
    Review {
        message_id: Vec<String>,
        /// `None` when `--mode` was not given: resolved against
        /// `AFUI_DELIVERY`, then `window`, inside the runner.
        mode: Option<UiDeliveryMode>,
    },
}

#[derive(Debug)]
pub enum TriageSuggestionAction {
    /// Replace the current suggestion and clear any human review of the prior round.
    Set(TriageSuggestionSetArgs),
    /// Clear exactly the current suggestion round.
    Clear {
        message_id: String,
        agent_suggestion_uid: String,
    },
}

#[derive(Debug)]
pub struct TriageSuggestionSetArgs {
    pub message_id: String,
    pub message_review_base_hash: String,
    pub reason: String,
    pub confidence_percent: Option<u8>,
    pub action: MessageSuggestionAction,
}

#[derive(Debug)]
pub enum MessageSuggestionAction {
    CaseCreate {
        name: String,
        group: String,
    },
    CaseAdd {
        case_ref: String,
    },
    NotificationCreate {
        name: String,
        summary: String,
    },
    NotificationAdd {
        notification_ref: String,
        summary: String,
    },
    Spam,
    Trash,
}

#[derive(Debug)]
pub enum PushAction {
    /// Preview the complete resolved push plan without writing.
    List,
}

#[derive(Debug)]
pub enum DoctorAction {
    /// Repair only unambiguous afmail-generated state.
    Repair,
}

/// Which discard state a purge targets. With none, every discard state is
/// purged together.
#[derive(Debug, Clone, Copy)]
pub enum PurgeAction {
    /// Permanently delete old local spam records.
    Spam,
    /// Permanently delete old local trash records.
    Trash,
    /// Permanently delete old local records whose remote message disappeared.
    Deleted,
}

#[derive(Debug)]
pub enum RenderAction {
    /// Rebuild generated case and notification read views.
    Refresh,
    /// Export built-in language templates, keeping existing files unless forced.
    Templates { force: bool },
}

#[derive(Debug)]
pub enum LogAction {
    /// List recent audit events.
    List { limit: usize },
    /// Tail recent audit events as JSON data.
    Tail,
    /// List events for one message id.
    Message { message_id: String },
    /// List events for one case id.
    Case { case_uid: String },
    /// List events for one notification collection.
    Notification { notification_uid: String },
}
