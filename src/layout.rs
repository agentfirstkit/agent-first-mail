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
//! Per-object subdirectories (`views/`, `data/`, `drafts/`, `files/`) are
//! intentionally out of scope.

/// Machine-managed metadata root.
pub(crate) const AFMAIL: &str = ".afmail";
/// Durable raw message evidence under [`AFMAIL`].
pub(crate) const AFMAIL_MESSAGES: &str = "messages";
/// Durable pull synchronization state under [`AFMAIL`].
pub(crate) const AFMAIL_PULL: &str = "pull";
/// Machine-local push intent under [`AFMAIL`].
pub(crate) const AFMAIL_PUSH: &str = "push";
/// Machine-local transaction sentinels under [`AFMAIL`].
pub(crate) const AFMAIL_TRANSACTIONS: &str = "transactions";
/// Machine-local idempotency records under [`AFMAIL`].
pub(crate) const AFMAIL_REQUESTS: &str = "requests";
/// Machine-local audit history under [`AFMAIL`].
pub(crate) const AFMAIL_LOGS: &str = "logs";
/// Freely rebuildable machine-local state under [`AFMAIL`].
pub(crate) const AFMAIL_CACHE: &str = "cache";

pub(crate) const AFMAIL_PULL_CURSORS: &str = "pull/cursors.json";
pub(crate) const AFMAIL_RENDER_CACHE: &str = "cache/render-cache.json";
pub(crate) const AFMAIL_PROGRESS: &str = "cache/progress.json";
pub(crate) const AFMAIL_AUDIT_LOG: &str = "logs/events.jsonl";

/// Root-level files written by older pre-release layouts.
pub(crate) const LEGACY_AFMAIL_FILES: &[&str] = &[
    "pull-cursors.json",
    "workspace.progress.json",
    "render-cache.json",
    "config.example.json",
    "workspace.lock",
];

pub(crate) fn afmail_path(relative: &str) -> std::path::PathBuf {
    std::path::Path::new(AFMAIL).join(relative)
}

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
