use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum ContactCommand {
    /// Create a new contact record.
    Create(ContactCreateArgs),
    /// List contacts, optionally filtered by group, tag, org, or role.
    List {
        /// Filter to a specific group.
        #[arg(long)]
        group: Option<String>,
        /// Filter by tag.
        #[arg(long)]
        tag: Option<String>,
        /// Filter by organization.
        #[arg(long)]
        org: Option<String>,
        /// Filter by role.
        #[arg(long)]
        role: Option<String>,
    },
    /// Show a contact's full file content.
    Show {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
    },
    /// Move a contact to a different group.
    Move {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Destination group.
        #[arg(long)]
        group: String,
        /// Why this contact should move groups; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Rename a contact's display name.
    Rename {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// New display name.
        #[arg(long)]
        name: String,
        /// Why this name better represents the contact; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Add or remove email addresses for a contact.
    Email {
        #[command(subcommand)]
        action: ContactEmailAction,
    },
    /// Add or remove phone numbers for a contact.
    Phone {
        #[command(subcommand)]
        action: ContactPhoneAction,
    },
    /// Add a tag to a contact.
    Tag {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Tag to add.
        tag: String,
        /// Why this tag is useful; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove a tag from a contact.
    Untag {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Tag to remove.
        tag: String,
        /// Why this tag should be removed; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show or edit contact notes.
    Notes {
        #[command(subcommand)]
        action: ContactNotesAction,
    },
    /// Delete a contact.
    Delete {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Also delete the contact body when it contains user-authored notes.
        #[arg(long)]
        allow_notes: bool,
        /// Why this contact should be deleted; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Extract stub contacts from message senders not yet in the contact list.
    Extract {
        /// Extract from triage messages only.
        #[arg(long, conflicts_with_all = ["from_case", "all"])]
        from_triage: bool,
        /// Extract from a specific case's messages.
        #[arg(long = "from-case", value_name = "CASE_REF", conflicts_with_all = ["from_triage", "all"])]
        from_case: Option<String>,
        /// Extract from all triage and case messages.
        #[arg(long, conflicts_with_all = ["from_triage", "from_case"])]
        all: bool,
        /// Destination group for created contacts. Defaults to contact.default_group.
        #[arg(long)]
        group: Option<String>,
    },
}

#[derive(Args, Debug, Clone)]
pub struct ContactCreateArgs {
    /// Display name for the contact.
    #[arg(long)]
    pub name: String,
    /// Group. Defaults to the configured default contact group.
    #[arg(long)]
    pub group: Option<String>,
    /// Email address. Repeatable.
    #[arg(long = "email")]
    pub emails: Vec<String>,
    /// Phone number. Repeatable.
    #[arg(long = "phone")]
    pub phones: Vec<String>,
    /// Organization name.
    #[arg(long)]
    pub org: Option<String>,
    /// Role or title.
    #[arg(long)]
    pub role: Option<String>,
    /// Tag. Repeatable.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Why this contact is being created; required by default.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ContactEmailAction {
    /// Add an email address to the contact.
    Add {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Email address to add.
        email: String,
        /// Why this email should be associated with the contact; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove an email address from the contact.
    Remove {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Email address to remove.
        email: String,
        /// Why this email should be removed; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ContactPhoneAction {
    /// Add a phone number to the contact.
    Add {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Phone number to add.
        phone: String,
        /// Why this phone number should be associated with the contact; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Remove a phone number from the contact.
    Remove {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Phone number to remove.
        phone: String,
        /// Why this phone number should be removed; required by default.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ContactNotesAction {
    /// Show notes markdown.
    Show {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
    },
    /// Append text to notes markdown.
    Append {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Markdown text to append.
        #[arg(long)]
        text: String,
    },
    /// Replace notes markdown with text.
    Replace {
        /// Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix.
        contact_ref: String,
        /// Markdown text to write.
        #[arg(long)]
        text: String,
    },
}
