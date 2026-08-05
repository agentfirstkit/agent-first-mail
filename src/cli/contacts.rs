#[derive(Debug)]
pub enum ContactCommand {
    /// Create a new contact record.
    Create(ContactCreateArgs),
    /// List contacts, optionally filtered by group, tag, org, or role.
    List {
        group: Option<String>,
        tag: Option<String>,
        org: Option<String>,
        role: Option<String>,
    },
    /// Show the exact typed contact document used by its Markdown template.
    Show { contact_ref: String },
    /// Move a contact to a different group.
    Move {
        contact_ref: String,
        group: String,
        reason: Option<String>,
    },
    /// Rename a contact's display name.
    Rename {
        contact_ref: String,
        name: String,
        reason: Option<String>,
    },
    /// Add or remove email addresses for a contact.
    Email { action: ContactEmailAction },
    /// Add or remove phone numbers for a contact.
    Phone { action: ContactPhoneAction },
    /// Add a tag to a contact.
    Tag {
        contact_ref: String,
        tag: String,
        reason: Option<String>,
    },
    /// Remove a tag from a contact.
    Untag {
        contact_ref: String,
        tag: String,
        reason: Option<String>,
    },
    /// Show or edit contact notes.
    Notes { action: ContactNotesAction },
    /// Delete a contact.
    Delete {
        contact_ref: String,
        allow_notes: bool,
        reason: Option<String>,
    },
    /// Extract stub contacts from message senders not yet in the contact list.
    Extract {
        from_triage: bool,
        from_case: Option<String>,
        all: bool,
        group: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ContactCreateArgs {
    /// Display name for the contact.
    pub name: String,
    /// Group. Defaults to the configured default contact group.
    pub group: Option<String>,
    /// Email addresses.
    pub emails: Vec<String>,
    /// Phone numbers.
    pub phones: Vec<String>,
    /// Organization name.
    pub org: Option<String>,
    /// Role or title.
    pub role: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
    /// Why this contact is being created; required by default.
    pub reason: Option<String>,
}

#[derive(Debug)]
pub enum ContactEmailAction {
    /// Add an email address to the contact.
    Add {
        contact_ref: String,
        email: String,
        reason: Option<String>,
    },
    /// Remove an email address from the contact.
    Remove {
        contact_ref: String,
        email: String,
        reason: Option<String>,
    },
}

#[derive(Debug)]
pub enum ContactPhoneAction {
    /// Add a phone number to the contact.
    Add {
        contact_ref: String,
        phone: String,
        reason: Option<String>,
    },
    /// Remove a phone number from the contact.
    Remove {
        contact_ref: String,
        phone: String,
        reason: Option<String>,
    },
}

#[derive(Debug)]
pub enum ContactNotesAction {
    /// Show the typed contact document with raw notes Markdown.
    Show { contact_ref: String },
    /// Append text to notes markdown.
    Append { contact_ref: String, text: String },
    /// Replace notes markdown with text.
    Replace { contact_ref: String, text: String },
}
