use clap::{Args, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ReviewMode {
    /// Open a local review window.
    Local,
    /// Print a one-time review URL for the trusted local network.
    Lan,
}

#[derive(Subcommand, Debug)]
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

#[derive(Subcommand, Debug)]
pub enum RemoteAction {
    /// Test configured IMAP and SMTP access without sending mail.
    Test,
    /// List IMAP folders/mailboxes.
    Folders,
}

#[derive(Subcommand, Debug)]
pub enum TriageAction {
    /// List compact untriaged message locators.
    List {
        /// Maximum number of newest triage messages to return.
        #[arg(long)]
        limit: Option<usize>,
        /// Include message metadata, related message ids, and case refs.
        #[arg(long)]
        details: bool,
    },
    /// Set or clear the current typed Agent suggestion for a message.
    Suggestion {
        #[command(subcommand)]
        action: TriageSuggestionAction,
    },
    /// Open a review session for persisted triage suggestions.
    Review {
        /// Message ids to review. With none, review every current triage suggestion.
        message_id: Vec<String>,
        /// Review presentation mode.
        #[arg(long, value_enum, default_value = "local")]
        mode: ReviewMode,
    },
}

#[derive(Subcommand, Debug)]
pub enum TriageSuggestionAction {
    /// Replace the current suggestion and clear any human review of the prior round.
    Set(TriageSuggestionSetArgs),
    /// Clear exactly the current suggestion round.
    Clear {
        /// Message id whose current suggestion round should be cleared.
        message_id: String,
        /// UID returned by the current suggestion set command.
        #[arg(long)]
        agent_suggestion_uid: String,
    },
}

#[derive(Args, Debug)]
pub struct TriageSuggestionSetArgs {
    /// Message id currently in triage.
    pub message_id: String,
    /// Current hash returned by `afmail message show`.
    #[arg(long)]
    pub message_review_base_hash: String,
    /// Why this complete action is being suggested.
    #[arg(long)]
    pub reason: String,
    /// Optional confidence from 0 through 100.
    #[arg(long)]
    pub confidence_percent: Option<u8>,
    #[command(subcommand)]
    pub action: MessageSuggestionAction,
}

#[derive(Subcommand, Debug)]
pub enum MessageSuggestionAction {
    CaseCreate {
        #[arg(long)]
        name: String,
        #[arg(long)]
        group: String,
    },
    CaseAdd {
        case_ref: String,
    },
    NotificationCreate {
        #[arg(long)]
        name: String,
        #[arg(long)]
        summary: String,
    },
    NotificationAdd {
        notification_ref: String,
        #[arg(long)]
        summary: String,
    },
    Spam,
    Trash,
}

#[derive(Subcommand, Debug)]
pub enum PushAction {
    /// Preview the complete resolved push plan without writing.
    List,
}

#[derive(Subcommand, Debug)]
pub enum DoctorAction {
    /// Repair only unambiguous afmail-generated state.
    Repair {
        /// Apply repair actions. Without this flag, repair is refused.
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PurgeAction {
    /// Permanently delete old local spam records.
    Spam {
        /// Only purge messages marked spam at least this many days ago. Defaults to 30.
        #[arg(long = "older-than-days", value_name = "DAYS")]
        older_than_days: Option<u64>,
    },
    /// Permanently delete old local trash records.
    Trash {
        /// Only purge messages marked trash at least this many days ago. Defaults to 30.
        #[arg(long = "older-than-days", value_name = "DAYS")]
        older_than_days: Option<u64>,
    },
    /// Permanently delete old local records whose remote message disappeared.
    Deleted {
        /// Only purge messages marked remote-deleted at least this many days ago. Defaults to 30.
        #[arg(long = "older-than-days", value_name = "DAYS")]
        older_than_days: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RenderAction {
    /// Rebuild generated case and notification read views.
    Refresh,
    /// Export built-in language templates, keeping existing files unless forced.
    Templates {
        /// Overwrite existing workspace templates with built-in defaults.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum LogAction {
    /// List recent audit events.
    List {
        /// Maximum number of events to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Tail recent audit events as JSON data.
    Tail,
    /// List events for one message id.
    Message { message_id: String },
    /// List events for one case id.
    Case { case_uid: String },
    /// List events for one notification collection.
    Notification { notification_uid: String },
}
