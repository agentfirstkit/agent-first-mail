mod archive;
mod cases;
pub mod contacts;
mod disposition_views;
mod doctor;
mod drafts;
mod messages;
mod purge;
mod push_state;
mod refs;
mod remote_sync;
mod render;
#[cfg(test)]
mod tests;
mod transactions;
mod triage;
mod util;
#[cfg(feature = "ui")]
mod view_model;

use cases::*;
pub use contacts::ContactExtractSource;
use disposition_views::*;
use drafts::*;
use messages::*;
use refs::CaseIndex;
use remote_sync::*;
use render::*;
use util::*;

pub use render::{
    clean_body_text, render_message_section, render_message_section_with_config,
    render_message_section_with_options,
};
pub(crate) use triage::render_triage_view;

use crate::config::{
    ArchiveMessageIndexField, MailConfig, ReasonMode, SpecialUseKind, TemplateLanguage,
};
use crate::error::{AppError, Result};
use crate::frontmatter::{CaseFrontmatter, DraftFrontmatter, TriageFrontmatter};
use crate::markdown::{read_doc, render_frontmatter};
use crate::templates::{language_template_path, MarkdownTemplateRenderer, TemplateKey};
use crate::types::RemoteSyncState;
use crate::types::{
    ArchiveMessageItem, ArchiveMessages, AttachmentRef, CaseMessages, MailDirection,
    MessageCollection, MessageCollectionItem, MessageContact, MessageFile, MessageStatus,
    OutboundAction, PushItem, PushLocation, RemoteLocation, RemoteState, WorkspacePendingPush,
    WorkspacePushState, WorkspaceState, ARCHIVE_NOTIFICATION_SCHEMA_NAME, CASE_SCHEMA_NAME,
    MESSAGE_COLLECTION_SCHEMA_VERSION,
};
use crate::util::{canonical_flags, sha256_fingerprint, write_json_pretty, write_string_atomic};
use chrono::{DateTime, Datelike, Duration, FixedOffset, SecondsFormat, Timelike, Utc};
use sanitize_filename::{sanitize_with_options, Options as SanitizeFilenameOptions};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

const AFMAIL_GITIGNORE_BEGIN: &str = "# BEGIN afmail managed";
const AFMAIL_GITIGNORE_END: &str = "# END afmail managed";
const AFMAIL_AGENT_GITIGNORE_HEADER: &str =
    "# Agent-First Mail workspace skill installed by afmail init.";
const AFMAIL_WORKSPACE_GITIGNORE_BODY: &str = r#"# Local mail evidence and runtime state.
.afmail/push/
.afmail/logs/
.afmail/transactions/
.afmail/workspace.lock
.afmail/workspace.progress.json

# Generated caches and read views; rebuild with afmail render refresh.
messages/*.json
triage/*.md
spam/*.md
trash/*.md
deleted/*.md
cases/*/*/case.md
cases/*/*/views/**/*.md
archive/cases/*/case.md
archive/cases/*/views/**/*.md
archive/notifications/*/archive.md
archive/notifications/*/views/**/*.md
"#;
const AFMAIL_RESERVED_INIT_PATHS: &[&str] = &[
    "triage",
    "cases",
    "archive",
    "messages",
    "templates",
    "spam",
    "trash",
    "deleted",
];
const AFMAIL_AGENTS_BEGIN: &str = "<!-- BEGIN afmail managed -->";
const AFMAIL_AGENTS_END: &str = "<!-- END afmail managed -->";

pub(crate) struct DraftChange<'a> {
    pub(crate) subject: Option<&'a str>,
    pub(crate) to: &'a [String],
    pub(crate) cc: &'a [String],
    pub(crate) clear_cc: bool,
    pub(crate) identity: Option<&'a str>,
    pub(crate) body: Option<&'a str>,
    pub(crate) body_file: Option<&'a str>,
}

pub(crate) struct NewDraft<'a> {
    pub(crate) to: &'a [String],
    pub(crate) cc: &'a [String],
    pub(crate) subject: &'a str,
    pub(crate) identity: Option<&'a str>,
    pub(crate) body: Option<&'a str>,
    pub(crate) body_file: Option<&'a str>,
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
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
                ))
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(AppError::new(
                    "invalid_request",
                    "afmail init PATH must stay under the current directory",
                ))
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
            AFMAIL_WORKSPACE_GITIGNORE_BODY,
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

fn agent_workspace_paths(agent_root: &Path, workspace_ref: &str) -> Result<Vec<String>> {
    let mut paths = read_agent_workspace_paths(agent_root)?;
    if !paths.iter().any(|path| path == workspace_ref) {
        paths.push(workspace_ref.to_string());
    }
    paths.sort();
    Ok(paths)
}

fn read_agent_workspace_paths(agent_root: &Path) -> Result<Vec<String>> {
    let path = agent_root.join("AGENTS.md");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = read_to_string(&path, "read agent instructions")?;
    let Some(begin_pos) = text.find(AFMAIL_AGENTS_BEGIN) else {
        return Ok(Vec::new());
    };
    let after_begin = begin_pos + AFMAIL_AGENTS_BEGIN.len();
    let Some(end_rel) = text[after_begin..].find(AFMAIL_AGENTS_END) else {
        return Ok(Vec::new());
    };
    let body = &text[after_begin..after_begin + end_rel];
    let mut out = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("- `") else {
            continue;
        };
        let Some(end) = rest.find('`') else {
            continue;
        };
        let value = &rest[..end];
        if !value.is_empty() && !out.iter().any(|existing| existing == value) {
            out.push(value.to_string());
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct CaseViewRefresh {
    case_index_count: usize,
    case_message_count: usize,
}

#[derive(Clone, Debug, Default)]
struct ArchiveMessageViewRefresh {
    archive_message_index_count: usize,
    archive_message_count: usize,
}

#[derive(Clone, Debug, Default)]
struct RenderRefreshTotals {
    active_case_count: usize,
    archived_case_count: usize,
    archive_message_category_count: usize,
    case_index_count: usize,
    case_message_count: usize,
    archive_message_index_count: usize,
    archive_message_count: usize,
}

impl RenderRefreshTotals {
    fn add_case(&mut self, refresh: CaseViewRefresh) {
        self.case_index_count += refresh.case_index_count;
        self.case_message_count += refresh.case_message_count;
    }

    fn add_archive_message(&mut self, refresh: ArchiveMessageViewRefresh) {
        self.archive_message_index_count += refresh.archive_message_index_count;
        self.archive_message_count += refresh.archive_message_count;
    }
}

impl Workspace {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { root: path.into() }
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
        Workspace::at(workspace_root).init_with_agent_root(agent_root, &workspace_ref)
    }

    pub fn init(&self) -> Result<Value> {
        self.init_with_agent_root(&self.root, ".")
    }

    fn init_with_agent_root(&self, agent_root: &Path, workspace_ref: &str) -> Result<Value> {
        prepare_init_target(&self.root)?;
        create_dir_all(&self.root.join(".afmail/messages"))?;
        create_dir_all(&self.root.join(".afmail/push"))?;
        create_dir_all(&self.root.join(".afmail/logs"))?;
        create_dir_all(&self.root.join(".afmail/transactions"))?;
        create_dir_all(&self.root.join("templates"))?;
        create_dir_all(&self.root.join("messages"))?;
        create_dir_all(&self.root.join("triage"))?;
        create_dir_all(&self.root.join("cases"))?;
        create_dir_all(&self.root.join("archive/cases"))?;
        create_dir_all(&self.root.join("archive/notifications"))?;
        write_json_if_missing(
            &self.root.join(".afmail/config.json"),
            &serde_json::to_value(crate::config::MailConfig::default())
                .map_err(|e| AppError::json("serialize config", &e))?,
        )?;
        write_string_if_missing(&self.root.join(".afmail/logs/events.jsonl"), "")?;
        let config = MailConfig::load(&self.root)?;
        let language = config.template_language();
        let language_bcp47 = config.resolved_language_bcp47().to_string();
        let timezone_utc_offset = config.resolved_timezone_utc_offset();
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        let workspace_paths = agent_workspace_paths(agent_root, workspace_ref)?;
        let template_context = json!({
            "language": language_bcp47,
            "workspaces": workspace_paths.clone(),
        });

        let workspace_skills_dir = agent_root.join(".codex/skills");
        let workspace_skill =
            crate::skill_admin::install_codex_workspace_skill(&workspace_skills_dir)?;
        let workspace_skill_path = workspace_skill.skill_path.clone();

        let workspace_gitignore_path = self.root.join(".gitignore");
        let agent_gitignore_path = agent_root.join(".gitignore");
        let same_root = self.root == agent_root;
        let workspace_gitignore_change = if same_root {
            ensure_agent_gitignore_ignores_skill_dirs_with_workspace_body(
                agent_root,
                std::slice::from_ref(&workspace_skill.skill_dir),
                true,
            )?
        } else {
            ensure_managed_block_file(
                &workspace_gitignore_path,
                AFMAIL_GITIGNORE_BEGIN,
                AFMAIL_GITIGNORE_END,
                "",
                AFMAIL_WORKSPACE_GITIGNORE_BODY,
            )?
        };
        let agent_gitignore_change = if same_root {
            workspace_gitignore_change
        } else {
            ensure_agent_gitignore_ignores_skill_dirs_with_workspace_body(
                agent_root,
                std::slice::from_ref(&workspace_skill.skill_dir),
                false,
            )?
        };

        let agent_skill_path = agent_root.join("AGENTS.md");
        let rendered_agents = renderer.render(TemplateKey::WorkspaceAgents, &template_context)?;
        let (agent_skill_prefix, agent_skill_body) = managed_block_template_parts(
            &rendered_agents,
            AFMAIL_AGENTS_BEGIN,
            AFMAIL_AGENTS_END,
            &agent_skill_path,
        )?;
        let agent_skill_change = ensure_managed_block_file(
            &agent_skill_path,
            AFMAIL_AGENTS_BEGIN,
            AFMAIL_AGENTS_END,
            &agent_skill_prefix,
            &agent_skill_body,
        )?;

        let do_not_edit_path = self.root.join(".afmail/DO_NOT_EDIT.txt");
        let do_not_edit_created = !do_not_edit_path.exists();
        let do_not_edit_rendered =
            renderer.render(TemplateKey::WorkspaceDoNotEdit, &template_context)?;
        let do_not_edit_updated = fs::read_to_string(&do_not_edit_path)
            .map(|existing| existing != do_not_edit_rendered)
            .unwrap_or(true);
        if do_not_edit_updated {
            write_string(&do_not_edit_path, &do_not_edit_rendered)?;
        }

        Ok(json!({
            "code": "workspace_initialized",
            "workspace_path": path_to_string(&self.root),
            "workspace_ref": workspace_ref,
            "agent_root_path": path_to_string(agent_root),
            "created_rfc3339": now_rfc3339(),
            "gitignore_path": rel_path(&self.root, &workspace_gitignore_path),
            "gitignore_created": workspace_gitignore_change.created,
            "gitignore_updated": workspace_gitignore_change.updated,
            "workspace_gitignore_path": rel_path(&self.root, &workspace_gitignore_path),
            "workspace_gitignore_created": workspace_gitignore_change.created,
            "workspace_gitignore_updated": workspace_gitignore_change.updated,
            "agent_gitignore_path": rel_path(agent_root, &agent_gitignore_path),
            "agent_gitignore_created": agent_gitignore_change.created,
            "agent_gitignore_updated": agent_gitignore_change.updated,
            "agent_skill_path": rel_path(agent_root, &agent_skill_path),
            "agent_skill_created": agent_skill_change.created,
            "agent_skill_updated": agent_skill_change.updated,
            "workspace_skill_path": rel_path(agent_root, &workspace_skill_path),
            "workspace_skill_installed": true,
            "do_not_edit_path": ".afmail/DO_NOT_EDIT.txt",
            "do_not_edit_created": do_not_edit_created,
            "do_not_edit_updated": do_not_edit_updated,
            "workspace_paths": workspace_paths,
            "language_bcp47": config.workspace.language_bcp47.clone(),
            "resolved_language_bcp47": config.resolved_language_bcp47(),
            "timezone_utc_offset": timezone_utc_offset,
            "next_steps": [
                format!("Run afmail commands with workdir set to {}.", workspace_ref),
                "Adjust workspace.language_bcp47 or workspace.timezone_utc_offset with afmail config set if needed.".to_string()
            ]
        }))
    }

    pub fn status(&self) -> Result<Value> {
        self.require_workspace()?;
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
        let archive_messages = self.archive_message_category_items()?;
        let archived_cases = self.archive_case_items()?;
        Ok(json!({
            "code": "status",
            "triage_count": count_files_with_ext(&self.root.join("triage"), "md")?,
            "case_count": cases.len(),
            "cases_by_group": cases_by_group,
            "archive_message_category_count": archive_messages.len(),
            "archived_case_count": archived_cases.len(),
            "message_count": message_paths.len(),
            "message_status": message_status,
            "remote_missing_count": remote_missing_count,
            "remote_effect_pending_message_count": remote_effect_pending_message_count,
            "push_count": count_files_with_ext(&self.root.join(".afmail/push"), "json")?,
            "push_status": push_status
        }))
    }

    pub fn config_show(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let value =
            serde_json::to_value(config).map_err(|e| AppError::json("serialize config", &e))?;
        Ok(json!({
            "code": "config",
            "config": value
        }))
    }

    pub fn config_get(&self, key: &str) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let value = config_value_for_output(key, config.get_key(key)?);
        Ok(json!({
            "code": "config_value",
            "key": key,
            "value": value
        }))
    }

    pub fn config_set(&self, key: &str, values: &[String]) -> Result<Value> {
        self.require_workspace()?;
        let mut config = crate::config::MailConfig::load(&self.root)?;
        config.set_key(key, values)?;
        config.write(&self.root)?;
        let value = config_value_for_output(key, config.get_key(key)?);
        Ok(json!({
            "code": "config_updated",
            "key": key,
            "value": value
        }))
    }

    pub fn remote_test(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?.require_imap()?;
        crate::imap_pull::remote_test(&config)
    }

    pub fn remote_folders(&self) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        let imap = config.require_imap()?;
        crate::imap_pull::remote_folders(&config, &imap)
    }

    pub fn push_with_progress(
        &self,
        confirmed: bool,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<Value> {
        self.require_workspace()?;
        crate::push_queue::push_with_progress(&self.root, confirmed, progress)
    }

    pub fn push_list(&self) -> Result<Value> {
        self.require_workspace()?;
        crate::push_queue::list(&self.root)
    }

    pub fn render_refresh(&self) -> Result<Value> {
        self.require_workspace()?;
        create_dir_all(&self.root.join("archive/cases"))?;
        create_dir_all(&self.root.join("archive/notifications"))?;
        let cache = self.rebuild_message_cache_from_eml()?;
        let triage = self.refresh_triage_views()?;
        let dispositions = self.refresh_disposition_views()?;
        let config = MailConfig::load(&self.root)?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, config.template_language());
        let mut totals = RenderRefreshTotals::default();

        for (_, case_path) in self.case_entries()? {
            let refresh =
                self.refresh_case_message_views_with_renderer(&case_path, &mut renderer)?;
            totals.active_case_count += 1;
            totals.add_case(refresh);
        }
        for entry in self.archived_case_entries()? {
            let refresh =
                self.refresh_case_message_views_with_renderer(&entry.path, &mut renderer)?;
            totals.archived_case_count += 1;
            totals.add_case(refresh);
        }
        for archive_uid in self.archive_message_category_ids()? {
            let refresh = self.refresh_archive_message_category_with_renderer(
                &archive_uid,
                &mut renderer,
                false,
            )?;
            totals.archive_message_category_count += 1;
            totals.add_archive_message(refresh);
        }

        Ok(json!({
            "code": "render_refreshed",
            "active_case_count": totals.active_case_count,
            "archived_case_count": totals.archived_case_count,
            "archive_message_category_count": totals.archive_message_category_count,
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
                "archive-message/archive.md.j2": totals.archive_message_index_count,
                "archive-message/message.md.j2": totals.archive_message_count,
            },
            "template_sources": renderer.stats().to_value(),
        }))
    }

    pub fn render_templates(&self, force: bool) -> Result<Value> {
        self.require_workspace()?;
        let templates_dir = self.root.join("templates");
        let existed_before = templates_dir.exists();
        if existed_before && !templates_dir.is_dir() {
            return Err(AppError::new(
                "template_dir_invalid",
                "templates exists but is not a directory",
            ));
        }

        create_dir_all(&templates_dir)?;

        let mut items = Vec::new();
        let mut exported_count = 0usize;
        let mut overwritten_count = 0usize;
        let mut kept_count = 0usize;
        let builtin_count = 0usize;
        let mut workspace_count = 0usize;

        for language in TemplateLanguage::ALL {
            for key in TemplateKey::ALL {
                let path = templates_dir.join(language_template_path(language, key));
                let existed = path.exists();
                let (source, action) = if force || !existed {
                    if let Some(parent) = path.parent() {
                        create_dir_all(parent)?;
                    }
                    write_string(&path, key.builtin_text(language))?;
                    workspace_count += 1;
                    if existed {
                        overwritten_count += 1;
                        ("workspace", "overwritten")
                    } else {
                        exported_count += 1;
                        ("workspace", "exported")
                    }
                } else {
                    workspace_count += 1;
                    kept_count += 1;
                    ("workspace", "kept")
                };
                items.push(json!({
                    "language": language.as_str(),
                    "template_key": key.as_str(),
                    "path": rel_path(&self.root, &path),
                    "source": source,
                    "action": action,
                }));
            }
        }

        Ok(json!({
            "code": "render_templates",
            "template_dir": "templates",
            "template_dir_created": !existed_before,
            "force": force,
            "exported_count": exported_count,
            "overwritten_count": overwritten_count,
            "kept_count": kept_count,
            "builtin_count": builtin_count,
            "workspace_count": workspace_count,
            "items": items,
        }))
    }

    pub fn log_list(&self, limit: usize) -> Result<Value> {
        let events = self.read_audit_events()?;
        Ok(json!({
            "code": "log_list",
            "count": events.len().min(limit),
            "events": take_last(events, limit)
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

    pub fn log_archive(&self, archive_ref: &str) -> Result<Value> {
        let archive_uid = parse_archive_ref(archive_ref)?;
        self.log_filter("archive", &archive_uid)
    }

    fn log_filter(&self, kind: &str, id: &str) -> Result<Value> {
        let events = self
            .read_audit_events()?
            .into_iter()
            .filter(|event| event_targets_id(event, kind, id))
            .collect::<Vec<_>>();
        Ok(json!({
            "code": "log_filtered",
            "target": {"kind": kind, "id": id},
            "count": events.len(),
            "events": events
        }))
    }
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
        "stale_triage_removed_count",
        "spam_count",
        "spam_written_count",
        "stale_spam_removed_count",
        "trash_count",
        "trash_written_count",
        "stale_trash_removed_count",
        "deleted_count",
        "deleted_written_count",
        "stale_deleted_removed_count",
    ] {
        if let Some(value) = triage_obj.get(key) {
            pull_obj.insert(key.to_string(), value.clone());
        }
    }
}
