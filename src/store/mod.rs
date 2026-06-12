mod archive;
mod cases;
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

use cases::*;
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
    ArchiveMessageItem, ArchiveMessages, AttachmentRef, CaseMessages, MailDirection, MessageFile,
    MessageStatus, PushItem, PushLocation, RemoteLocation, RemoteState, WorkspacePendingPush,
    WorkspacePushState, WorkspaceState,
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
const AFMAIL_GITIGNORE_BODY: &str = r#"# Local mail evidence and runtime state.
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
const AFMAIL_AGENTS_BEGIN: &str = "<!-- BEGIN afmail managed -->";
const AFMAIL_AGENTS_END: &str = "<!-- END afmail managed -->";

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
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

    pub fn init(&self) -> Result<Value> {
        create_dir_all(&self.root.join(".afmail/messages"))?;
        create_dir_all(&self.root.join(".afmail/push"))?;
        create_dir_all(&self.root.join(".afmail/logs"))?;
        create_dir_all(&self.root.join(".afmail/transactions"))?;
        create_dir_all(&self.root.join("messages"))?;
        create_dir_all(&self.root.join("triage"))?;
        create_dir_all(&self.root.join("spam"))?;
        create_dir_all(&self.root.join("trash"))?;
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
        let template_context = json!({"language": language_bcp47});
        let gitignore_change = ensure_managed_block_file(
            &self.root.join(".gitignore"),
            AFMAIL_GITIGNORE_BEGIN,
            AFMAIL_GITIGNORE_END,
            "",
            AFMAIL_GITIGNORE_BODY,
        )?;
        let agent_skill_path = self.root.join("AGENTS.md");
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
        let do_not_edit_created = if do_not_edit_path.exists() {
            false
        } else {
            write_string(
                &do_not_edit_path,
                &renderer.render(TemplateKey::WorkspaceDoNotEdit, &template_context)?,
            )?;
            true
        };
        Ok(json!({
            "code": "workspace_initialized",
            "workspace_path": path_to_string(&self.root),
            "created_rfc3339": now_rfc3339(),
            "gitignore_path": ".gitignore",
            "gitignore_created": gitignore_change.created,
            "gitignore_updated": gitignore_change.updated,
            "agent_skill_path": "AGENTS.md",
            "agent_skill_created": agent_skill_change.created,
            "agent_skill_updated": agent_skill_change.updated,
            "do_not_edit_path": ".afmail/DO_NOT_EDIT.txt",
            "do_not_edit_created": do_not_edit_created,
            "language_bcp47": config.workspace.language_bcp47.clone(),
            "resolved_language_bcp47": config.resolved_language_bcp47(),
            "timezone_utc_offset": timezone_utc_offset,
            "next_steps": [
                "Adjust workspace.language_bcp47 or workspace.timezone_utc_offset with afmail config set if needed."
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

    pub fn push(&self, mode: crate::push_queue::PushMode, dry_run: bool) -> Result<Value> {
        self.push_with_progress(mode, dry_run, None)
    }

    pub fn push_with_progress(
        &self,
        mode: crate::push_queue::PushMode,
        dry_run: bool,
        progress: Option<&mut crate::progress::ProgressCallback<'_>>,
    ) -> Result<Value> {
        self.require_workspace()?;
        crate::push_queue::push_with_progress(&self.root, mode, dry_run, progress)
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
        let templates_dir = self.root.join(".afmail/templates");
        let existed_before = templates_dir.exists();
        if existed_before && !templates_dir.is_dir() {
            return Err(AppError::new(
                "template_dir_invalid",
                ".afmail/templates exists but is not a directory",
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
            "template_dir": ".afmail/templates",
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
