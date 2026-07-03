//! Single source of truth for the on-disk workspace directory layout.
//!
//! Every top-level workspace directory name is defined here exactly once. Path
//! construction, the managed `.gitignore` body, the reserved-init-path list,
//! disposition view specs, and agent-facing path templates all reference these
//! constants instead of repeating string literals, so the layout lives in one
//! place.
//!
//! These are structural identifiers, not localizable labels: they are part of
//! afmail's CLI and skill contract and stay English. Human-facing localization
//! happens in generated Markdown via per-language template packs, not here. The
//! `.afmail/` metadata root and per-object subdirectories (`views/`, `data/`,
//! `drafts/`, `files/`) are intentionally out of scope.

/// New mail awaiting a decision.
pub(crate) const TRIAGE: &str = "triage";
/// Active cases, grouped as `cases/<group>/<case_uid>-<name>/`.
pub(crate) const CASES: &str = "cases";
/// Archived case workspaces, grouped like active cases.
pub(crate) const ARCHIVED_CASES: &str = "archived-cases";
/// Notification collections.
pub(crate) const NOTIFICATIONS: &str = "notifications";
/// Generated review view for locally spammed mail.
pub(crate) const SPAM: &str = "spam";
/// Generated review view for locally trashed mail.
pub(crate) const TRASH: &str = "trash";
/// Generated review view for remote-deleted mail retained locally.
pub(crate) const DELETED: &str = "deleted";
/// Contact cards, grouped.
pub(crate) const CONTACTS: &str = "contacts";
/// Optional persona override Markdown, one file per configured identity.
pub(crate) const IDENTITIES: &str = "identities";
/// Parsed message JSON cache, rebuilt from `.afmail/messages/` evidence.
pub(crate) const MESSAGES: &str = "messages";
/// User-editable per-language Markdown template overrides.
pub(crate) const TEMPLATES: &str = "templates";
