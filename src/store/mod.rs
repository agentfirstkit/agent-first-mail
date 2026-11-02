mod cases;
pub mod contacts;
mod disposition_views;
mod doctor;
mod drafts;
mod idempotency;
mod messages;
mod migration;
mod notification;
mod purge;
mod push_state;
mod readiness;
mod refs;
mod remote_sync;
mod render;
mod render_cache;
mod storage_usage;
mod suggestions;
#[cfg(test)]
mod tests;
mod transactions;
mod triage;
mod util;

use cases::*;
pub use contacts::ContactExtractSource;
use disposition_views::*;
use drafts::*;
use messages::*;
use refs::CaseIndex;
use remote_sync::*;
use render::*;
use render_cache::*;
use storage_usage::workspace_storage_usage;
use util::*;

pub use render::{
    clean_body_text, render_message_section, render_message_section_with_config,
    render_message_section_with_options,
};
#[cfg(any(feature = "review", test))]
pub(crate) use suggestions::{
    CaseHumanSuggestionInput, MessageHumanSuggestionInput, NotificationHumanSuggestionInput,
};
#[cfg(feature = "review")]
pub(crate) use suggestions::{
    case_review_base_hash_from_documents, notification_review_base_hash_from_documents,
};
pub(crate) use triage::render_triage_view;

use crate::config::{
    MailConfig, ReasonMode, ResolvedIdentity, SpecialUseKind, TemplateLanguage,
    match_message_identity_with_profiles,
};
use crate::draft_content::{
    DraftContent, DraftContentError, parse_draft_content, replace_authored_text,
};
use crate::error::{AppError, Result};
use crate::frontmatter::{CaseFrontmatter, DraftFrontmatter};
use crate::layout;
use crate::markdown::{edit_frontmatter, read_doc, render_frontmatter};
use crate::render_document::{
    CollectionLocation, CollectionMessageRenderDocument, CollectionRenderDocument,
    CollectionShowDocument, CollectionShowLocator, ContactLocator, ContactRenderDocument,
    ContentProvenance, ContentSource, ContentTrust, DraftAttachmentAnalysis, DraftLocator,
    DraftRenderAnalysis, DraftRenderDocument, DraftReplyRenderDocument, DraftSourceDocument,
    EmptyRenderDocument, MessageBodyAnalysis, MessageBodySegment, MessageBodySegmentKind,
    MessageContextDocument, MessageRenderAnalysis, MessageRenderDocument, MessageSecurityAnalysis,
    MessageShowDocument, NotesMergeRenderDocument, NotesRenderDocument,
    RENDER_DOCUMENT_SCHEMA_VERSION, RelatedMessageIssue, RenderMessageAction,
    RenderMessageDirection, TextRange, TriageRenderDocument, collection_document_schema_name,
    collection_message_document_schema_name, draft_document_schema_name,
    message_document_schema_name, notes_document_schema_name, triage_document_schema_name,
};
use crate::templates::{
    TemplateKey, TemplateRenderOptions, TemplateRenderer, ensure_template_path_safe,
    language_template_path,
};
use crate::types::RemoteSyncState;
use crate::types::{
    AttachmentRef, CASE_SCHEMA_NAME, CaseMessages, CaseRelationshipSuggestion, ContactFrontmatter,
    DraftCompleteness, DraftStateProjection, MESSAGE_COLLECTION_SCHEMA_VERSION,
    MESSAGE_SCHEMA_VERSION, MailDirection, MessageActionPush, MessageCollection,
    MessageCollectionItem, MessageFile, MessageMetadata, MessagePushAction, MessageStatus,
    NOTIFICATION_SCHEMA_NAME, NotificationMessageItem, NotificationMessages, OutboundAction,
    PushItem, PushLocalStage, PushLocation, RemoteLocation, RemoteState,
    SUGGESTIBLE_COLLECTION_SCHEMA_VERSION, WorkspacePendingPush, WorkspacePushState,
    WorkspaceState,
};
use crate::util::{
    blake3_fingerprint, blake3_fingerprint_str, canonical_flags, write_json_pretty,
    write_string_atomic,
};
use chrono::{DateTime, Duration, FixedOffset, SecondsFormat, Utc};
use sanitize_filename::{Options as SanitizeFilenameOptions, sanitize_with_options};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const AFMAIL_GITIGNORE_BEGIN: &str = "# BEGIN afmail managed";
const AFMAIL_GITIGNORE_END: &str = "# END afmail managed";
const AFMAIL_AGENT_GITIGNORE_HEADER: &str =
    "# Agent-First Mail workspace skill installed by afmail skill install.";
fn afmail_workspace_gitignore_body() -> String {
    format!(
        r#"# This workspace is meant to live in a PRIVATE repository: pulled mail bodies,
# attachments, and .afmail/config.json (which may hold an inline imap/smtp
# password_secret) are intentionally NOT ignored, so they are committed as part
# of your private mailbox state. If this repo could ever be public or shared, point
# password_secret at an env:NAME_SECRET or file:PATH#KEY source outside the
# workspace instead of storing the credential inline in config.

# Machine-local state. Only .afmail/cache/ is safe to delete, and only while no
# afmail process is running: it holds the workspace lock file.
.{afmail}/{push}/
.{afmail}/{transactions}/
.{afmail}/{requests}/
.{afmail}/{logs}/
.{afmail}/{cache}/

# Generated caches and read views; rebuild with afmail render refresh.
{messages}/*.json
{triage}/*.md
{spam}/*.md
{trash}/*.md
{deleted}/*.md
{cases}/*/*/case.md
{cases}/*/*/views/**/*.md
{archived_cases}/*/*/case.md
{archived_cases}/*/*/views/**/*.md
{notifications}/*/notification.md
{notifications}/*/views/**/*.md
"#,
        messages = layout::MESSAGES,
        triage = layout::TRIAGE,
        spam = layout::SPAM,
        trash = layout::TRASH,
        deleted = layout::DELETED,
        cases = layout::CASES,
        archived_cases = layout::ARCHIVED_CASES,
        notifications = layout::NOTIFICATIONS,
        afmail = &layout::AFMAIL[1..],
        push = layout::AFMAIL_PUSH,
        transactions = layout::AFMAIL_TRANSACTIONS,
        requests = layout::AFMAIL_REQUESTS,
        logs = layout::AFMAIL_LOGS,
        cache = layout::AFMAIL_CACHE,
    )
}
const AFMAIL_RESERVED_INIT_PATHS: &[&str] = &[
    layout::TRIAGE,
    layout::CASES,
    layout::NOTIFICATIONS,
    layout::ARCHIVED_CASES,
    layout::MESSAGES,
    layout::TEMPLATES,
    layout::SPAM,
    layout::TRASH,
    layout::DELETED,
];
pub(crate) struct DraftChange<'a> {
    pub(crate) subject: Option<&'a str>,
    pub(crate) to: &'a [String],
    pub(crate) cc: &'a [String],
    pub(crate) clear_cc: bool,
    pub(crate) bcc: &'a [String],
    pub(crate) clear_bcc: bool,
    pub(crate) identity: Option<&'a str>,
    pub(crate) body: Option<&'a str>,
    pub(crate) body_file: Option<&'a str>,
}

fn review_document_hash<T: Serialize + ?Sized>(label: &str, document: &T) -> Result<String> {
    let operation = format!("serialize {label} review base");
    let bytes = serde_json::to_vec(document).map_err(|error| AppError::json(&operation, &error))?;
    Ok(blake3_fingerprint(&bytes))
}

pub(crate) struct NewDraft<'a> {
    pub(crate) to: &'a [String],
    pub(crate) cc: &'a [String],
    pub(crate) bcc: &'a [String],
    pub(crate) subject: &'a str,
    pub(crate) identity: Option<&'a str>,
    pub(crate) body: Option<&'a str>,
    pub(crate) body_file: Option<&'a str>,
}

pub struct ReplyDraft<'a> {
    pub message_id: &'a str,
    pub reply_all: bool,
    pub bcc: &'a [String],
    pub identity: Option<&'a str>,
    pub body: Option<&'a str>,
    pub body_file: Option<&'a str>,
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn new_audit_event_id() -> String {
    new_event_id()
}

fn resolve_init_target(agent_root: &Path, path: Option<&str>) -> Result<(PathBuf, String)> {
    let raw = path.unwrap_or(".");
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return Err(AppError::new(
            "invalid_request",
            "afmail init PATH must be relative to the current directory",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in rel.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                return Err(AppError::new(
                    "invalid_request",
                    "afmail init PATH must not contain ..",
                ));
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(AppError::new(
                    "invalid_request",
                    "afmail init PATH must stay under the current directory",
                ));
            }
        }
    }
    let workspace_ref = if normalized.as_os_str().is_empty() {
        ".".to_string()
    } else {
        let mut value = path_to_string(&normalized);
        if !value.ends_with('/') {
            value.push('/');
        }
        value
    };
    let target = if normalized.as_os_str().is_empty() {
        agent_root.to_path_buf()
    } else {
        agent_root.join(normalized)
    };
    Ok((target, workspace_ref))
}

pub(crate) fn ensure_agent_gitignore_ignores_skill_dirs(
    agent_root: &Path,
    skill_dirs: &[PathBuf],
) -> Result<()> {
    let include_workspace_body = agent_gitignore_should_include_workspace_body(agent_root)?;
    let _ = ensure_agent_gitignore_ignores_skill_dirs_with_workspace_body(
        agent_root,
        skill_dirs,
        include_workspace_body,
    )?;
    Ok(())
}

fn ensure_agent_gitignore_ignores_skill_dirs_with_workspace_body(
    agent_root: &Path,
    skill_dirs: &[PathBuf],
    include_workspace_body: bool,
) -> Result<ManagedBlockChange> {
    let gitignore_path = agent_root.join(".gitignore");
    let existing_body = existing_gitignore_managed_body(&gitignore_path)?;
    let mut patterns = BTreeSet::new();
    if let Some(body) = existing_body.as_deref() {
        patterns.extend(existing_agent_skill_gitignore_patterns(body));
    }
    patterns.extend(
        skill_dirs
            .iter()
            .filter_map(|dir| agent_skill_dir_gitignore_pattern(agent_root, dir)),
    );

    if patterns.is_empty() && !include_workspace_body {
        return Ok(ManagedBlockChange {
            created: false,
            updated: false,
        });
    }

    let mut block_body = if patterns.is_empty() {
        String::new()
    } else {
        render_agent_gitignore_body(&patterns)
    };
    if include_workspace_body {
        if !block_body.is_empty() {
            block_body.push('\n');
        }
        block_body.push_str(trim_surrounding_line_endings(
            &afmail_workspace_gitignore_body(),
        ));
    }

    ensure_managed_block_file(
        &gitignore_path,
        AFMAIL_GITIGNORE_BEGIN,
        AFMAIL_GITIGNORE_END,
        "",
        &block_body,
    )
}

fn agent_gitignore_should_include_workspace_body(agent_root: &Path) -> Result<bool> {
    if agent_root.join(".afmail").is_dir() {
        return Ok(true);
    }
    Ok(
        existing_gitignore_managed_body(&agent_root.join(".gitignore"))?
            .is_some_and(|body| body.contains(".afmail/push/") || body.contains("messages/*.json")),
    )
}

fn existing_gitignore_managed_body(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let existing = read_to_string(path, "read managed block file")?;
    let Some(begin_pos) = existing.find(AFMAIL_GITIGNORE_BEGIN) else {
        return Ok(None);
    };
    let after_begin = begin_pos + AFMAIL_GITIGNORE_BEGIN.len();
    let end_rel = existing[after_begin..]
        .find(AFMAIL_GITIGNORE_END)
        .ok_or_else(|| {
            AppError::new(
                "managed_block_invalid",
                format!(
                    "{} has an afmail managed BEGIN marker without a matching END marker",
                    path_to_string(path)
                ),
            )
        })?;
    let end_pos = after_begin + end_rel;
    Ok(Some(
        trim_surrounding_line_endings(&existing[after_begin..end_pos]).to_string(),
    ))
}

fn existing_agent_skill_gitignore_patterns(block_body: &str) -> BTreeSet<String> {
    block_body
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with(".agents/")
                || !line.ends_with("/agent-first-mail/")
                || !line.contains("/skills/agent-first-mail/")
            {
                return None;
            }
            Some(line.to_string())
        })
        .collect()
}

fn agent_skill_dir_gitignore_pattern(agent_root: &Path, skill_dir: &Path) -> Option<String> {
    let absolute = if skill_dir.is_absolute() {
        skill_dir.to_path_buf()
    } else {
        agent_root.join(skill_dir)
    };
    let rel = absolute.strip_prefix(agent_root).ok()?;
    if rel.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    let mut pattern = path_to_string(rel);
    if pattern.is_empty() {
        return None;
    }
    if !pattern.ends_with('/') {
        pattern.push('/');
    }
    Some(pattern)
}

fn render_agent_gitignore_body(patterns: &BTreeSet<String>) -> String {
    let mut out = String::new();
    out.push_str(AFMAIL_AGENT_GITIGNORE_HEADER);
    out.push('\n');
    for pattern in patterns {
        out.push_str(pattern);
        out.push('\n');
    }
    out
}

fn prepare_init_target(root: &Path) -> Result<()> {
    if root.exists() && !root.is_dir() {
        return Err(AppError::new(
            "invalid_request",
            format!(
                "afmail init target is not a directory: {}",
                path_to_string(root)
            ),
        ));
    }
    if !root.exists() {
        create_dir_all(root)?;
    }
    let afmail_dir = root.join(".afmail");
    if afmail_dir.exists() && !afmail_dir.is_dir() {
        return Err(AppError::new(
            "invalid_request",
            format!(
                "afmail init target has a non-directory .afmail path: {}",
                path_to_string(&afmail_dir)
            ),
        ));
    }
    if afmail_dir.is_dir() {
        return Ok(());
    }
    let conflicts = reserved_init_conflicts(root);
    if conflicts.is_empty() {
        return Ok(());
    }
    Err(AppError::new(
        "invalid_request",
        format!(
            "afmail init target already contains afmail-reserved paths: {}",
            conflicts.join(", ")
        ),
    )
    .with_hint("Choose a different empty mail workspace directory, or run init in an existing directory that already contains .afmail.")
    .with_details(json!({ "reserved_paths": conflicts })))
}

fn reserved_init_conflicts(root: &Path) -> Vec<String> {
    AFMAIL_RESERVED_INIT_PATHS
        .iter()
        .filter_map(|name| {
            if root.join(name).exists() {
                Some(format!("{name}/"))
            } else {
                None
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    read_snapshot: Option<ReadSnapshot>,
}

#[derive(Clone, Debug, Default)]
struct CaseViewRefresh {
    case_index_count: usize,
    case_message_count: usize,
    case_index_unchanged_count: usize,
    case_message_unchanged_count: usize,
}

#[derive(Clone, Debug, Default)]
struct NotificationViewRefresh {
    notification_index_count: usize,
    notification_message_count: usize,
    notification_index_unchanged_count: usize,
    notification_message_unchanged_count: usize,
}

#[derive(Clone, Debug, Default)]
struct RenderRefreshTotals {
    active_case_count: usize,
    archived_case_count: usize,
    notification_count: usize,
    case_index_count: usize,
    case_message_count: usize,
    case_index_unchanged_count: usize,
    case_message_unchanged_count: usize,
    notification_index_count: usize,
    notification_message_count: usize,
    notification_index_unchanged_count: usize,
    notification_message_unchanged_count: usize,
}

impl RenderRefreshTotals {
    fn add_case(&mut self, refresh: CaseViewRefresh) {
        self.case_index_count += refresh.case_index_count;
        self.case_message_count += refresh.case_message_count;
        self.case_index_unchanged_count += refresh.case_index_unchanged_count;
        self.case_message_unchanged_count += refresh.case_message_unchanged_count;
    }

    fn add_notification(&mut self, refresh: NotificationViewRefresh) {
        self.notification_index_count += refresh.notification_index_count;
        self.notification_message_count += refresh.notification_message_count;
        self.notification_index_unchanged_count += refresh.notification_index_unchanged_count;
        self.notification_message_unchanged_count += refresh.notification_message_unchanged_count;
    }
}

fn render_done_fields(stage: &str, started: Instant, mut fields: Value) -> Value {
    if let Value::Object(map) = &mut fields {
        map.insert("render_stage".to_string(), json!(stage));
        map.insert(
            "elapsed_ms".to_string(),
            json!(started.elapsed().as_millis() as u64),
        );
        return fields;
    }
    json!({
        "render_stage": stage,
        "elapsed_ms": started.elapsed().as_millis() as u64,
    })
}

fn render_case_done_fields(totals: &RenderRefreshTotals) -> Value {
    json!({
        "active_case_count": totals.active_case_count,
        "archived_case_count": totals.archived_case_count,
        "case_view_count": totals.case_index_count + totals.case_index_unchanged_count,
        "case_view_written_count": totals.case_index_count + totals.case_message_count,
        "case_view_unchanged_count": totals.case_index_unchanged_count + totals.case_message_unchanged_count,
        "case_index_written_count": totals.case_index_count,
        "case_index_unchanged_count": totals.case_index_unchanged_count,
        "case_message_written_count": totals.case_message_count,
        "case_message_unchanged_count": totals.case_message_unchanged_count,
    })
}

fn render_notification_done_fields(totals: &RenderRefreshTotals) -> Value {
    json!({
        "notification_count": totals.notification_count,
        "notification_view_written_count": totals.notification_index_count + totals.notification_message_count,
        "notification_view_unchanged_count": totals.notification_index_unchanged_count + totals.notification_message_unchanged_count,
        "notification_index_written_count": totals.notification_index_count,
        "notification_index_unchanged_count": totals.notification_index_unchanged_count,
        "notification_message_written_count": totals.notification_message_count,
        "notification_message_unchanged_count": totals.notification_message_unchanged_count,
    })
}

impl Workspace {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            root: path.into(),
            read_snapshot: None,
        }
    }

    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let mut current = start.as_ref().to_path_buf();
        loop {
            if current.join(".afmail").is_dir() {
                return Ok(Self::at(current));
            }
            if !current.pop() {
                return Err(AppError::new(
                    "workspace_not_found",
                    "no .afmail directory found in current directory or parents",
                ));
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn init_command(agent_root: &Path, path: Option<&str>) -> Result<Value> {
        let (workspace_root, workspace_ref) = resolve_init_target(agent_root, path)?;
        Workspace::at(workspace_root).init_with_workspace_ref(&workspace_ref)
    }

    #[cfg(feature = "demo")]
    pub(crate) fn init_empty_command(agent_root: &Path, path: &str) -> Result<(Self, Value)> {
        if Path::new(path)
            .components()
            .find_map(|component| match component {
                std::path::Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .is_some_and(|first| AFMAIL_RESERVED_INIT_PATHS.contains(&first))
        {
            return Err(AppError::new(
                "invalid_request",
                format!("demo init path uses an afmail-reserved name: {path}"),
            ));
        }
        let (workspace_root, workspace_ref) = resolve_init_target(agent_root, Some(path))?;
        if workspace_root.is_dir() {
            let mut entries = fs::read_dir(&workspace_root)
                .map_err(|error| AppError::io("inspect init target", &error))?;
            if entries
                .next()
                .transpose()
                .map_err(|error| AppError::io("inspect init target entry", &error))?
                .is_some()
            {
                return Err(AppError::new(
                    "invalid_request",
                    format!(
                        "demo init target must be empty: {}",
                        path_to_string(&workspace_root)
                    ),
                )
                .with_hint(
                    "Choose a new empty relative path; demo init never replaces existing files.",
                ));
            }
        }
        let workspace = Workspace::at(workspace_root);
        let result = workspace.init_with_workspace_ref(&workspace_ref)?;
        Ok((workspace, result))
    }

    pub fn init(&self) -> Result<Value> {
        self.init_with_workspace_ref(".")
    }

    fn init_with_workspace_ref(&self, workspace_ref: &str) -> Result<Value> {
        prepare_init_target(&self.root)?;
        let managed_paths = [
            ".afmail",
            ".afmail/workspace.json",
            ".afmail/config.json",
            ".afmail/DO_NOT_EDIT.txt",
            ".afmail/messages",
            ".afmail/pull",
            ".afmail/cache",
            ".afmail/push",
            ".afmail/logs",
            ".afmail/logs/events.jsonl",
            ".afmail/transactions",
            ".afmail/requests",
            "templates",
            "messages",
            "triage",
            "cases",
            "archived-cases",
            "notifications",
            ".gitignore",
        ];
        let missing_before = managed_paths
            .iter()
            .filter(|path| !self.root.join(path).exists())
            .copied()
            .collect::<BTreeSet<_>>();
        let _lock = crate::workspace_lock::WorkspaceLock::acquire(
            &self.root,
            crate::workspace_lock::LockMode::Exclusive,
        )?;
        prepare_init_target(&self.root)?;
        create_dir_all(&self.root.join(".afmail"))?;
        let identity_state = crate::workspace_identity::workspace_identity_for_init(&self.root)?;
        let migration = migration::WorkspaceMigration::preflight(&self.root)?;
        let workspace_upgraded = identity_state.workspace_upgraded || migration.has_legacy_inputs();
        let migration_counts = migration.apply()?;
        create_dir_all(&self.root.join(".afmail/messages"))?;
        create_dir_all(&self.root.join(layout::afmail_path(layout::AFMAIL_PULL)))?;
        create_dir_all(&self.root.join(layout::afmail_path(layout::AFMAIL_CACHE)))?;
        create_dir_all(
            &self
                .root
                .join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH)),
        )?;
        create_dir_all(
            &self
                .root
                .join(crate::layout::afmail_path(crate::layout::AFMAIL_LOGS)),
        )?;
        create_dir_all(&self.root.join(crate::layout::afmail_path(
            crate::layout::AFMAIL_TRANSACTIONS,
        )))?;
        create_dir_all(
            &self
                .root
                .join(crate::layout::afmail_path(crate::layout::AFMAIL_REQUESTS)),
        )?;
        create_dir_all(&self.root.join(layout::TEMPLATES))?;
        create_dir_all(&self.root.join(layout::MESSAGES))?;
        create_dir_all(&self.root.join(layout::TRIAGE))?;
        create_dir_all(&self.root.join(layout::CASES))?;
        create_dir_all(&self.root.join(layout::ARCHIVED_CASES))?;
        create_dir_all(&self.root.join(layout::NOTIFICATIONS))?;
        write_json_if_missing(
            &self.root.join(".afmail/config.json"),
            &crate::config::MailConfig::default().sparse_value()?,
        )?;
        write_string_if_missing(
            &self
                .root
                .join(layout::afmail_path(layout::AFMAIL_AUDIT_LOG)),
            "",
        )?;
        crate::workspace_identity::write_workspace_identity(&self.root, &identity_state.identity)?;
        let config = MailConfig::load(&self.root)?;
        let language = config.template_language();
        let timezone_utc_offset = config.resolved_timezone_utc_offset();
        let mut renderer = TemplateRenderer::new(&self.root, language);
        let workspace_gitignore_path = self.root.join(".gitignore");
        let workspace_gitignore_change = ensure_managed_block_file(
            &workspace_gitignore_path,
            AFMAIL_GITIGNORE_BEGIN,
            AFMAIL_GITIGNORE_END,
            "",
            &afmail_workspace_gitignore_body(),
        )?;

        let do_not_edit_path = self.root.join(".afmail/DO_NOT_EDIT.txt");
        let do_not_edit_created = !do_not_edit_path.exists();
        let do_not_edit_rendered = renderer.render(
            TemplateKey::WorkspaceDoNotEdit,
            &EmptyRenderDocument {
                schema_name: "afmail_workspace_warning_render_document".to_string(),
                schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
            },
        )?;
        let do_not_edit_updated = fs::read_to_string(&do_not_edit_path)
            .map(|existing| existing != do_not_edit_rendered)
            .unwrap_or(true);
        if do_not_edit_updated {
            write_string(&do_not_edit_path, &do_not_edit_rendered)?;
        }

        migration.remove_legacy_message_files()?;
        // Init is also the restart point for an interrupted upgrade. Always
        // rebuild projections so a prior run that committed canonical v2 state
        // but stopped during rendering can safely finish on the next run.
        self.render_refresh()?;

        Ok(json!({
            "workspace_path": path_to_string(&self.root),
            "workspace_ref": workspace_ref,
            "workspace_uid": identity_state.identity.workspace_uid,
            "created_rfc3339": identity_state.identity.created_rfc3339,
            "workspace_upgraded": workspace_upgraded,
            "layout_version_before": identity_state.layout_version_before,
            "layout_version_after": crate::workspace_identity::WORKSPACE_LAYOUT_VERSION,
            "message_metadata_migrated_count": migration_counts.message_metadata_migrated_count,
            "case_migrated_count": migration_counts.case_migrated_count,
            "notification_migrated_count": migration_counts.notification_migrated_count,
            "disposition_migrated_count": migration_counts.disposition_migrated_count,
            "gitignore_path": rel_path(&self.root, &workspace_gitignore_path),
            "gitignore_created": workspace_gitignore_change.created,
            "gitignore_updated": workspace_gitignore_change.updated,
            "workspace_gitignore_path": rel_path(&self.root, &workspace_gitignore_path),
            "workspace_gitignore_created": workspace_gitignore_change.created,
            "workspace_gitignore_updated": workspace_gitignore_change.updated,
            "created_files": managed_paths.iter().filter(|path| missing_before.contains(**path)).collect::<Vec<_>>(),
            "changed_files": [".afmail/DO_NOT_EDIT.txt", ".gitignore"],
            "do_not_edit_path": ".afmail/DO_NOT_EDIT.txt",
            "do_not_edit_created": do_not_edit_created,
            "do_not_edit_updated": do_not_edit_updated,
            "language_bcp47": config.workspace.language_bcp47.clone(),
            "resolved_language_bcp47": config.resolved_language_bcp47(),
            "timezone_utc_offset": timezone_utc_offset,
            "next_steps": [
                format!("Run afmail commands with workdir set to {}.", workspace_ref),
                "Install the agent skill explicitly: afmail skill install --agent all --scope workspace".to_string(),
                "Set IMAP fields with afmail config set imap.host HOST, imap.username USER, and imap.password_secret SOURCE (env:NAME_SECRET, file:PATH#KEY, or the literal password).".to_string(),
                "Set SMTP fields with afmail config set smtp.host HOST, smtp.username USER, and smtp.password_secret SOURCE.".to_string(),
                "Replace the placeholder identity with afmail config add identities SLUG name=NAME email=ADDRESS default=true, then afmail config remove identities me, before drafting mail.".to_string(),
                "Run afmail status for local readiness, then afmail remote test before the first pull.".to_string()
            ]
        }))
    }

    pub fn status(&self) -> Result<Value> {
        self.require_workspace()?;
        let workspace_identity = crate::workspace_identity::load_workspace_identity(&self.root)?;
        let config = MailConfig::load(&self.root)?;
        let cases = self.active_case_items()?;
        let mut cases_by_group: BTreeMap<String, usize> = BTreeMap::new();
        for case in &cases {
            let group = case
                .get("group")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            *cases_by_group.entry(group).or_insert(0) += 1;
        }
        let mut message_status: BTreeMap<String, usize> = BTreeMap::new();
        let message_paths = message_json_paths(&self.root)?;
        let mut remote_missing_count = 0usize;
        let mut remote_effect_pending_message_count = 0usize;
        for path in &message_paths {
            let message = read_message(path)?;
            *message_status
                .entry(message.workspace.status.clone())
                .or_insert(0) += 1;
            if message_remote_missing(&message) {
                remote_missing_count += 1;
            }
            if message_remote_effect_pending(&message) {
                remote_effect_pending_message_count += 1;
            }
        }
        let push_status = serde_json::to_value(crate::push_queue::push_status(&self.root)?)
            .map_err(|e| AppError::json("serialize push status", &e))?;
        let notifications = self.notification_items()?;
        let archived_cases = self.archived_case_items()?;
        let archiving_case_count = archived_cases
            .iter()
            .filter(|case| case["status"] == "archiving")
            .count();
        let archived_case_count = archived_cases.len() - archiving_case_count;
        let storage = workspace_storage_usage(&self.root)?;
        let consumer = crate::sources::cloudflare_journal::Journal::snapshot(&self.root)
            .unwrap_or_else(|error| error.to_error_value());
        Ok(json!({
            "workspace_uid": workspace_identity.workspace_uid,
            "workspace_path": path_to_string(&self.root),
            "triage_count": count_files_with_ext(&self.root.join(layout::TRIAGE), "md")?,
            "case_count": cases.len() + archiving_case_count,
            "active_case_count": cases.len(),
            "archiving_case_count": archiving_case_count,
            "cases_by_group": cases_by_group,
            "notification_count": notifications.len(),
            "archived_case_count": archived_case_count,
            "message_count": message_paths.len(),
            "message_status": message_status,
            "remote_missing_count": remote_missing_count,
            "remote_effect_pending_message_count": remote_effect_pending_message_count,
            "push_count": count_files_with_ext(&self.root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH)), "json")?,
            "push_status": push_status,
            "storage": storage,
            "cloudflare_consumer": consumer,
            "readiness": readiness::readiness(&config)
        }))
    }

    pub fn config_show(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let value =
            serde_json::to_value(config).map_err(|e| AppError::json("serialize config", &e))?;
        // Not redacted here: every event leaves through a `CliEmitter` built on
        // afdata's default redactor, which walks `*_secret` field names itself.
        // A local pre-pass could only agree with it or be overwritten by it —
        // and it was overwritten, which is how an unset `password_secret` came
        // out as `"***"` and told readers a credential was configured.
        Ok(json!({ "config": value }))
    }

    pub fn config_get(&self, key: &str) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let value = config_value_for_output(key, config.get_key(key)?);
        Ok(json!({
            "key": key,
            "value": value
        }))
    }

    pub fn config_set(&self, key: &str, values: &[String]) -> Result<Value> {
        self.require_workspace()?;
        let mut doc = self.open_config_document()?;
        crate::config::MailConfig::apply_set(&mut doc, key, values)?;
        crate::config::MailConfig::validate_document(&doc, &self.root)?;
        doc.save().map_err(config_write_error)?;
        let value = config_value_for_output(key, self.config_get_dense(key)?);
        Ok(json!({
            "key": key,
            "value": value
        }))
    }

    pub fn config_add(&self, key: &str, slug: &str, fields: &[String]) -> Result<Value> {
        self.require_workspace()?;
        let parsed = parse_field_assignments(fields)?;
        let mut doc = self.open_config_document()?;
        crate::config::MailConfig::apply_add(&mut doc, key, slug, &parsed)?;
        crate::config::MailConfig::validate_document(&doc, &self.root)?;
        doc.save().map_err(config_write_error)?;
        let value = config_value_for_output(key, self.config_get_dense(key)?);
        Ok(json!({
            "key": key,
            "slug": slug,
            "value": value
        }))
    }

    pub fn config_remove(&self, key: &str, slug: &str) -> Result<Value> {
        self.require_workspace()?;
        let mut doc = self.open_config_document()?;
        crate::config::MailConfig::apply_remove(&mut doc, key, slug)?;
        crate::config::MailConfig::validate_document(&doc, &self.root)?;
        doc.save().map_err(config_write_error)?;
        let value = config_value_for_output(key, self.config_get_dense(key)?);
        Ok(json!({
            "key": key,
            "slug": slug,
            "value": value
        }))
    }

    /// Open the workspace's sparse `.afmail/config.json` for source-preserving
    /// editing. afdata's document editor grows the sparse file in place —
    /// creating parent objects for new keys — so afmail no longer hand-writes
    /// a re-derived sparse config.
    fn open_config_document(&self) -> Result<agent_first_data::document::DocumentFile> {
        agent_first_data::document::DocumentFile::open(
            self.root.join(".afmail/config.json"),
            Some(agent_first_data::document::Format::Json),
        )
        .map_err(|e| AppError::new("config_invalid", format!("open config: {e}")))
    }

    /// The dense (defaults-filled) value at `key`, for command output — mirrors
    /// what a subsequent `config get` would report.
    fn config_get_dense(&self, key: &str) -> Result<Value> {
        crate::config::MailConfig::load(&self.root)?.get_key(key)
    }

    pub fn remote_test(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let sending_configured = config.cloudflare.sending.api_token_secret.is_some();
        if crate::sources::configured(&config).is_empty()
            && config.smtp.host.is_none()
            && !sending_configured
        {
            return Err(AppError::new(
                "config_missing",
                "configure IMAP, Cloudflare R2, Cloudflare sending, or SMTP before remote test",
            ));
        }
        let imap = if config.imap.host.is_some() {
            let mut value = crate::sources::imap::remote_test(&config.require_imap()?)?;
            value["status"] = json!("succeeded");
            value
        } else {
            json!({"status":"skipped","reason":"imap_not_configured"})
        };
        let cloudflare = if config.cloudflare.is_configured() {
            crate::sources::cloudflare::remote_test(&config)?
        } else {
            json!({"status":"skipped","reason":"cloudflare_not_configured"})
        };
        let smtp = if config.smtp.host.is_some() {
            crate::senders::smtp::remote_test(&config.require_smtp()?)?
        } else {
            json!({"status":"skipped","reason":"smtp_not_configured","message_sent":false})
        };
        let mut result = json!({"imap":imap,"cloudflare":cloudflare,"smtp":smtp});
        if sending_configured {
            result["cloudflare_sending"] = crate::senders::cloudflare::check_config(&config)?;
        }
        Ok(result)
    }

    pub fn remote_folders(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let imap = config.require_imap().map_err(|error| {
            error.with_hint(
                "Cloudflare sources have no folders; configure IMAP to list remote folders.",
            )
        })?;
        crate::sources::imap::remote_folders(&config, &imap)
    }

    pub fn push_with_progress(
        &self,
        confirm_plan_id: Option<&str>,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<crate::types::CommandResult> {
        self.require_workspace()?;
        crate::push_queue::push_with_progress(&self.root, confirm_plan_id, progress)
    }

    pub fn push_recover(
        &self,
        push_id: &str,
        confirm_plan_id: Option<&str>,
        settle: Option<crate::cli::SendSettlement>,
    ) -> Result<Value> {
        self.require_workspace()?;
        crate::push_queue::recovery::recover(&self.root, push_id, confirm_plan_id, settle)
    }

    pub fn push_list(&self) -> Result<Value> {
        self.require_workspace()?;
        crate::push_queue::list(&self.root)
    }

    pub fn render_refresh(&self) -> Result<Value> {
        self.render_refresh_with_progress(None)
    }

    pub fn render_refresh_with_progress(
        &self,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let mut progress = progress;
        crate::progress::emit(&mut progress, "render_start", json!({}));
        create_dir_all(&self.root.join(layout::ARCHIVED_CASES))?;
        create_dir_all(&self.root.join(layout::NOTIFICATIONS))?;

        crate::progress::emit(
            &mut progress,
            "render_materialize_start",
            json!({"render_stage": "materialize"}),
        );
        let step_started = Instant::now();
        let static_inputs = RenderStaticInputs::load(self, MailConfig::load(&self.root)?)?;
        let preparation = self.prepare_render_context(static_inputs)?;
        let cache = &preparation.materialization;
        crate::progress::emit(
            &mut progress,
            "render_materialize_done",
            render_done_fields(
                "materialize",
                step_started,
                json!({
                    "message_cache_rebuilt_count": cache.rebuilt_count,
                    "text_cache_removed_count": cache.removed_text_cache_count,
                    "validated_eml_count": cache.validated_eml_count,
                    "materialized_cache_written_count": cache.materialized_cache_written_count,
                }),
            ),
        );

        let mut render_cache = self.read_render_cache()?;
        crate::progress::emit(
            &mut progress,
            "render_triage_start",
            json!({"render_stage": "triage"}),
        );
        let step_started = Instant::now();
        let triage = self.refresh_triage_views_cached_with_progress(
            &mut render_cache,
            progress.as_deref_mut(),
            "render_triage_progress",
            &preparation.context,
        )?;
        crate::progress::emit(
            &mut progress,
            "render_triage_done",
            render_done_fields("triage", step_started, triage.clone()),
        );

        crate::progress::emit(
            &mut progress,
            "render_dispositions_start",
            json!({"render_stage": "dispositions"}),
        );
        let step_started = Instant::now();
        let dispositions =
            self.refresh_disposition_views_cached(&mut render_cache, &preparation.context)?;
        crate::progress::emit(
            &mut progress,
            "render_dispositions_done",
            render_done_fields("dispositions", step_started, dispositions.clone()),
        );

        let config = preparation.context.config();
        let mut renderer = TemplateRenderer::new(&self.root, config.template_language());
        let mut totals = RenderRefreshTotals::default();

        crate::progress::emit(
            &mut progress,
            "render_cases_start",
            json!({"render_stage": "cases"}),
        );
        let step_started = Instant::now();
        let active_cases = self.case_entries()?;
        let archived_cases = self.archived_case_entries()?;
        let total_case_count = active_cases.len() + archived_cases.len();
        let mut processed_case_count = 0usize;
        for (_, case_path) in active_cases {
            let refresh = self.refresh_case_message_views_with_cache_context(
                &case_path,
                &mut renderer,
                &mut render_cache,
                Some(&preparation.context),
            )?;
            totals.active_case_count += 1;
            totals.add_case(refresh);
            processed_case_count += 1;
            if processed_case_count == total_case_count || processed_case_count.is_multiple_of(25) {
                crate::progress::emit(
                    &mut progress,
                    "render_cases_progress",
                    json!({
                        "render_stage": "cases",
                        "processed_count": processed_case_count,
                        "total_count": total_case_count,
                        "active_case_count": totals.active_case_count,
                        "archived_case_count": totals.archived_case_count,
                        "case_view_written_count": totals.case_index_count + totals.case_message_count,
                        "case_view_unchanged_count": totals.case_index_unchanged_count + totals.case_message_unchanged_count,
                    }),
                );
            }
        }
        for entry in archived_cases {
            let refresh = self.refresh_case_message_views_with_cache_context(
                &entry.path,
                &mut renderer,
                &mut render_cache,
                Some(&preparation.context),
            )?;
            totals.archived_case_count += 1;
            totals.add_case(refresh);
            processed_case_count += 1;
            if processed_case_count == total_case_count || processed_case_count.is_multiple_of(25) {
                crate::progress::emit(
                    &mut progress,
                    "render_cases_progress",
                    json!({
                        "render_stage": "cases",
                        "processed_count": processed_case_count,
                        "total_count": total_case_count,
                        "active_case_count": totals.active_case_count,
                        "archived_case_count": totals.archived_case_count,
                        "case_view_written_count": totals.case_index_count + totals.case_message_count,
                        "case_view_unchanged_count": totals.case_index_unchanged_count + totals.case_message_unchanged_count,
                    }),
                );
            }
        }
        crate::progress::emit(
            &mut progress,
            "render_cases_done",
            render_done_fields("cases", step_started, render_case_done_fields(&totals)),
        );

        crate::progress::emit(
            &mut progress,
            "render_notifications_start",
            json!({"render_stage": "notifications"}),
        );
        let step_started = Instant::now();
        let notification_ids = self.notification_ids()?;
        let total_notification_count = notification_ids.len();
        for (index, notification_uid) in notification_ids.into_iter().enumerate() {
            let refresh = self.refresh_notification_with_cache(
                &notification_uid,
                &mut renderer,
                &mut render_cache,
                &preparation.context,
            )?;
            totals.notification_count += 1;
            totals.add_notification(refresh);
            let processed_count = index + 1;
            if processed_count == total_notification_count || processed_count % 25 == 0 {
                crate::progress::emit(
                    &mut progress,
                    "render_notifications_progress",
                    json!({
                        "render_stage": "notifications",
                        "processed_count": processed_count,
                        "total_count": total_notification_count,
                        "notification_count": totals.notification_count,
                        "notification_view_written_count": totals.notification_index_count + totals.notification_message_count,
                        "notification_view_unchanged_count": totals.notification_index_unchanged_count + totals.notification_message_unchanged_count,
                    }),
                );
            }
        }
        crate::progress::emit(
            &mut progress,
            "render_notifications_done",
            render_done_fields(
                "notifications",
                step_started,
                render_notification_done_fields(&totals),
            ),
        );

        crate::progress::emit(
            &mut progress,
            "render_cache_write_start",
            json!({"render_stage": "cache_write"}),
        );
        let step_started = Instant::now();
        self.write_render_cache_if_changed(&mut render_cache)?;
        crate::progress::emit(
            &mut progress,
            "render_cache_write_done",
            render_done_fields("cache_write", step_started, json!({})),
        );

        let result = json!({
            "active_case_count": totals.active_case_count,
            "archived_case_count": totals.archived_case_count,
            "notification_count": totals.notification_count,
            "message_cache_rebuilt_count": cache.rebuilt_count,
            "text_cache_removed_count": cache.removed_text_cache_count,
            "triage_count": triage.get("triage_count").and_then(Value::as_u64).unwrap_or(0),
            "triage_written_count": triage.get("triage_written_count").and_then(Value::as_u64).unwrap_or(0),
            "stale_triage_removed_count": triage.get("stale_triage_removed_count").and_then(Value::as_u64).unwrap_or(0),
            "spam_count": dispositions.get("spam_count").and_then(Value::as_u64).unwrap_or(0),
            "spam_written_count": dispositions.get("spam_written_count").and_then(Value::as_u64).unwrap_or(0),
            "stale_spam_removed_count": dispositions.get("stale_spam_removed_count").and_then(Value::as_u64).unwrap_or(0),
            "trash_count": dispositions.get("trash_count").and_then(Value::as_u64).unwrap_or(0),
            "trash_written_count": dispositions.get("trash_written_count").and_then(Value::as_u64).unwrap_or(0),
            "stale_trash_removed_count": dispositions.get("stale_trash_removed_count").and_then(Value::as_u64).unwrap_or(0),
            "deleted_count": dispositions.get("deleted_count").and_then(Value::as_u64).unwrap_or(0),
            "deleted_written_count": dispositions.get("deleted_written_count").and_then(Value::as_u64).unwrap_or(0),
            "stale_deleted_removed_count": dispositions.get("stale_deleted_removed_count").and_then(Value::as_u64).unwrap_or(0),
            "generated": {
                "triage/view.md.j2": triage.get("triage_written_count").and_then(Value::as_u64).unwrap_or(0),
                "status/index.md.j2": dispositions.get("index_written_count").and_then(Value::as_u64).unwrap_or(0),
                "status/message.md.j2": dispositions.get("message_written_count").and_then(Value::as_u64).unwrap_or(0),
                "case/case.md.j2": totals.case_index_count,
                "case/message.md.j2": totals.case_message_count,
                "notification/notification.md.j2": totals.notification_index_count,
                "notification/message.md.j2": totals.notification_message_count,
            },
            "template_sources": renderer.stats().to_value(),
        });
        crate::progress::emit(&mut progress, "render_done", result.clone());
        Ok(result)
    }

    pub fn render_templates(&self, force: bool) -> Result<Value> {
        self.require_workspace()?;
        let templates_dir = self.root.join(layout::TEMPLATES);
        let existed_before = templates_dir.exists();
        if existed_before && !templates_dir.is_dir() {
            return Err(AppError::new(
                "template_dir_invalid",
                "templates exists but is not a directory",
            ));
        }

        create_dir_all(&templates_dir)?;

        let mut items = Vec::new();
        let mut counts = ExportCounts::default();

        // Content only. The Review UI is exported into the frontend
        // directories below, because that is the one place afmail reads it
        // from; writing it here too would leave files that look like an
        // override and change nothing.
        for (language, key) in export_targets().filter(|(_, key)| !key.is_review_ui()) {
            let builtin_language = language.unwrap_or(TemplateLanguage::EnUs);
            let path = templates_dir.join(language_template_path(builtin_language, key));
            ensure_template_path_safe(&templates_dir, &path)?;
            let action = export_template(&path, key, builtin_language, force)?;
            counts.record(action);
            counts.workspace += 1;
            items.push(json!({
                "language": language.map(|language| language.as_str()),
                "template_key": key.as_str(),
                "path": rel_path(&self.root, &path),
                "source": "workspace",
                "action": action,
            }));
        }

        #[cfg(feature = "review")]
        for ui_kind in crate::review_frontend::UI_KINDS {
            // Only into a frontend the person already created with
            // `afui frontend init`. Creating one here would install an override
            // nobody asked for, and would make every workspace look like it has
            // a custom Review UI.
            let frontend_root =
                crate::review_frontend::workspace_frontend_root(&self.root, ui_kind)?;
            if !frontend_root.join("frontend.json").is_file() {
                continue;
            }
            let frontend_templates = frontend_root.join(layout::TEMPLATES);
            for (language, key) in export_targets().filter(|(_, key)| key.is_review_ui()) {
                let builtin_language = language.unwrap_or(TemplateLanguage::EnUs);
                let path = frontend_templates.join(language_template_path(builtin_language, key));
                ensure_template_path_safe(&frontend_templates, &path)?;
                let action = export_template(&path, key, builtin_language, force)?;
                counts.record(action);
                counts.frontend += 1;
                items.push(json!({
                    "language": language.map(|language| language.as_str()),
                    "template_key": key.as_str(),
                    "path": rel_path(&self.root, &path),
                    "source": "frontend",
                    "ui_kind": ui_kind,
                    "action": action,
                }));
            }
        }

        Ok(json!({
            "template_dir": "templates",
            "template_dir_created": !existed_before,
            "force": force,
            "exported_count": counts.exported,
            "overwritten_count": counts.overwritten,
            "kept_count": counts.kept,
            "builtin_count": 0,
            "workspace_count": counts.workspace,
            "frontend_count": counts.frontend,
            "items": items,
        }))
    }

    pub fn log_list(&self, limit: usize) -> Result<Value> {
        let audit = self.read_audit_log()?;
        Ok(json!({
            "count": audit.events.len().min(limit),
            "events": take_last(audit.events, limit),
            "warning_count": audit.warnings.len(),
            "warnings": audit.warnings,
        }))
    }

    pub fn log_tail(&self) -> Result<Value> {
        self.log_list(20).map(|mut value| {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("code".to_string(), json!("log_tail"));
            }
            value
        })
    }

    pub fn log_message(&self, message_id: &str) -> Result<Value> {
        validate_id("message_id", message_id)?;
        self.log_filter("message", message_id)
    }

    pub fn log_case(&self, case_ref: &str) -> Result<Value> {
        let case_uid = parse_case_ref(case_ref)?;
        self.log_filter("case", &case_uid)
    }

    pub fn log_notification(&self, notification_ref: &str) -> Result<Value> {
        let notification_uid = parse_notification_ref(notification_ref)?;
        self.log_filter("notification", &notification_uid)
    }

    fn log_filter(&self, kind: &str, id: &str) -> Result<Value> {
        let audit = self.read_audit_log()?;
        let events = audit
            .events
            .into_iter()
            .filter(|event| event_targets_id(event, kind, id))
            .collect::<Vec<_>>();
        Ok(json!({
            "target": {"kind": kind, "id": id},
            "count": events.len(),
            "events": events,
            "warning_count": audit.warnings.len(),
            "warnings": audit.warnings,
        }))
    }
}

/// What one `render templates` run did, per destination and per file.
#[derive(Default)]
struct ExportCounts {
    exported: usize,
    overwritten: usize,
    kept: usize,
    workspace: usize,
    frontend: usize,
}

impl ExportCounts {
    fn record(&mut self, action: &str) {
        match action {
            "overwritten" => self.overwritten += 1,
            "kept" => self.kept += 1,
            _ => self.exported += 1,
        }
    }
}

/// Write one built-in template out, and say what that did.
fn export_template(
    path: &Path,
    key: TemplateKey,
    language: TemplateLanguage,
    force: bool,
) -> Result<&'static str> {
    let existed = path.exists();
    if !force && existed {
        return Ok("kept");
    }
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    write_string(path, key.builtin_text(language))?;
    Ok(if existed { "overwritten" } else { "exported" })
}

/// Every built-in template, once per language pack it varies by.
///
/// `None` is a template that does not vary by language and is exported once
/// into the shared tree.
fn export_targets() -> impl Iterator<Item = (Option<TemplateLanguage>, TemplateKey)> {
    TemplateLanguage::ALL
        .into_iter()
        .flat_map(|language| {
            TemplateKey::ALL
                .into_iter()
                .filter(|key| key.is_language_scoped())
                .map(move |key| (Some(language), key))
        })
        .chain(
            TemplateKey::ALL
                .into_iter()
                .filter(|key| !key.is_language_scoped())
                .map(|key| (None, key)),
        )
}

fn merge_reconciliation_into_pull(pull: &mut Value, reconciliation: &Value) {
    let Some(pull_obj) = pull.as_object_mut() else {
        return;
    };
    let Some(reconcile_obj) = reconciliation.as_object() else {
        return;
    };
    for key in [
        "checked_location_count",
        "missing_location_count",
        "deleted_remote_message_count",
        "deleted_remote_message_ids",
        "tombstoned_message_count",
        "tombstoned_message_ids",
        "kept_message_count",
        "kept_message_ids",
    ] {
        if let Some(value) = reconcile_obj.get(key) {
            pull_obj.insert(key.to_string(), value.clone());
        }
    }
}

fn merge_triage_refresh_into_pull(pull: &mut Value, triage: &Value) {
    let Some(pull_obj) = pull.as_object_mut() else {
        return;
    };
    let Some(triage_obj) = triage.as_object() else {
        return;
    };
    for key in [
        "triage_count",
        "triage_written_count",
        "triage_unchanged_count",
        "stale_triage_removed_count",
        "spam_count",
        "spam_written_count",
        "spam_unchanged_count",
        "stale_spam_removed_count",
        "trash_count",
        "trash_written_count",
        "trash_unchanged_count",
        "stale_trash_removed_count",
        "deleted_count",
        "deleted_written_count",
        "deleted_unchanged_count",
        "stale_deleted_removed_count",
        "index_written_count",
        "index_unchanged_count",
        "message_written_count",
        "message_unchanged_count",
    ] {
        if let Some(value) = triage_obj.get(key) {
            pull_obj.insert(key.to_string(), value.clone());
        }
    }
}
