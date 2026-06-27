use super::*;
use chrono::{Local, Offset};

pub(super) fn default_schema_name() -> String {
    "config".to_string()
}

pub(super) fn default_imap_port() -> u16 {
    993
}

pub(super) fn default_smtp_port() -> u16 {
    587
}

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_mailbox_configs() -> BTreeMap<String, ImapMailboxConfig> {
    let mut mailboxes = BTreeMap::new();
    mailboxes.insert(
        "inbox".to_string(),
        ImapMailboxConfig::mailbox_name("INBOX"),
    );
    mailboxes.insert(
        "sent".to_string(),
        ImapMailboxConfig::special_use(SpecialUseKind::Sent),
    );
    mailboxes.insert(
        "archive".to_string(),
        ImapMailboxConfig::special_use(SpecialUseKind::Archive),
    );
    mailboxes.insert(
        "junk".to_string(),
        ImapMailboxConfig::special_use(SpecialUseKind::Junk),
    );
    mailboxes.insert(
        "trash".to_string(),
        ImapMailboxConfig::special_use(SpecialUseKind::Trash),
    );
    mailboxes.insert(
        "drafts".to_string(),
        ImapMailboxConfig::special_use(SpecialUseKind::Drafts),
    );
    mailboxes
}

pub(super) fn default_identities() -> Vec<IdentityConfig> {
    vec![IdentityConfig {
        identity: "me".to_string(),
        name: "Me".to_string(),
        email: "me@example.com".to_string(),
        default: true,
    }]
}

pub(super) fn default_timezone_utc_offset_option() -> Option<String> {
    Some(default_timezone_utc_offset())
}

/// Pin the timezone to the system's current UTC offset at config creation, as
/// AFDATA canonical `UTC` or a fixed offset like `+08:00`.
pub(super) fn default_timezone_utc_offset() -> String {
    let offset = Local::now().offset().fix();
    if offset.local_minus_utc() == 0 {
        "UTC".to_string()
    } else {
        offset.to_string()
    }
}

pub(super) fn default_case_group() -> String {
    "open".to_string()
}

pub(super) fn default_reason_mode() -> ReasonMode {
    ReasonMode::Required
}

pub(super) fn default_contact_group() -> String {
    "people".to_string()
}

pub(super) fn default_archive_message_index_fields() -> Vec<ArchiveMessageIndexField> {
    vec![
        ArchiveMessageIndexField::Time,
        ArchiveMessageIndexField::From,
        ArchiveMessageIndexField::Summary,
    ]
}
