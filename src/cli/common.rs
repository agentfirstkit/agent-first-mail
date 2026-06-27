use clap::Subcommand;

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
    /// Test IMAP login using local config.
    Test,
    /// List IMAP folders/mailboxes.
    Folders,
}

#[derive(Subcommand, Debug)]
pub enum TriageAction {
    /// List compact untriaged message locators.
    List,
}

#[derive(Subcommand, Debug)]
pub enum PushAction {
    /// List queued local push items.
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
    /// Rebuild generated case and direct-message archive read views.
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
    /// List events for one archive category.
    Archive { archive_uid: String },
}
