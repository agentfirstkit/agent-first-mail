use agent_first_ui::UiDeliveryMode;

#[derive(Debug)]
pub enum NotificationCommand {
    /// Create a notification collection and optionally file one message.
    Create(NotificationCreateArgs),
    /// File an existing message into this notification collection.
    Add {
        notification_ref: String,
        message_id: Vec<String>,
        summary: String,
        reason: Option<String>,
    },
    /// List compact notification collections.
    List,
    /// Show the exact typed notification document used by Markdown and Review.
    Show { notification_ref: String },
    /// Open a review session for this notification and its persisted suggestion.
    Review {
        notification_ref: String,
        mode: Option<UiDeliveryMode>,
    },
    /// Set or clear the current typed Agent suggestion for this collection.
    Suggestion {
        action: NotificationSuggestionAction,
    },
    /// Remove a notification message from this collection and return it to triage.
    Remove {
        notification_ref: String,
        message_id: String,
        reason: Option<String>,
    },
    /// Move a notification message to another notification collection.
    Move {
        notification_ref: String,
        message_id: String,
        new_notification_ref: String,
        reason: Option<String>,
    },
    /// Rename this notification collection's human-readable name without changing its UID.
    Rename {
        notification_ref: String,
        name: String,
        reason: Option<String>,
    },
    /// Delete an empty notification collection.
    Delete {
        notification_ref: String,
        allow_notes: bool,
        reason: Option<String>,
    },
    /// Set or replace one notification entry summary.
    SetSummary {
        notification_ref: String,
        message_id: String,
        summary: String,
        reason: Option<String>,
    },
    /// Show or edit notification collection notes.
    Notes { action: NotificationNotesAction },
}

#[derive(Debug)]
pub enum NotificationSuggestionAction {
    /// Replace the current suggestion and clear any human review of the prior round.
    Set(NotificationSuggestionSetArgs),
    /// Clear exactly the current suggestion round.
    Clear {
        notification_ref: String,
        agent_suggestion_uid: String,
    },
}

#[derive(Debug)]
pub struct NotificationSuggestionSetArgs {
    pub notification_ref: String,
    pub notification_review_base_hash: String,
    pub reason: String,
    pub confidence_percent: Option<u8>,
    pub action: NotificationSuggestionActionKind,
}

#[derive(Debug)]
pub enum NotificationSuggestionActionKind {
    Rename { name: String },
    Delete { allow_notes: bool },
}

#[derive(Debug, Clone)]
pub struct NotificationCreateArgs {
    /// Human-readable notification name used in metadata and the directory suffix.
    pub name: String,
    /// Optional message to immediately file into the new notification collection.
    pub message: Option<String>,
    /// Required when `message` is supplied.
    pub summary: Option<String>,
    /// Why this notification collection is being created.
    pub reason: Option<String>,
}

#[derive(Debug)]
pub enum NotificationNotesAction {
    /// Show notes markdown.
    Show { notification_ref: String },
    /// Append text to notes markdown.
    Append {
        notification_ref: String,
        text: String,
    },
    /// Replace notes markdown with text.
    Replace {
        notification_ref: String,
        text: String,
    },
}
