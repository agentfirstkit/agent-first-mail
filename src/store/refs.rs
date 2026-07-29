use super::{Workspace, case_json_path, read_case_messages, rel_path};
use crate::error::Result;
use crate::render_document::CaseReferenceDocument;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct CaseIndex {
    root: PathBuf,
    entries: Vec<CaseIndexEntry>,
    entries_by_message: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug)]
struct CaseIndexEntry {
    case_uid: String,
    group: String,
    path: PathBuf,
    status: String,
    message_ids: BTreeSet<String>,
}

impl CaseIndex {
    pub(super) fn build(workspace: &Workspace) -> Result<Self> {
        let mut entries = Vec::new();
        for (case_uid, case_path) in workspace.case_entries()? {
            if !case_json_path(&case_path).exists() {
                continue;
            }
            let messages = read_case_messages(&case_path, &case_uid)?;
            if !messages.status.is_empty() {
                return Err(crate::error::AppError::new(
                    "case_metadata_invalid",
                    format!(
                        "active case {case_uid} must have an empty canonical status, found {:?}",
                        messages.status
                    ),
                ));
            }
            let group = case_path
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            entries.push(CaseIndexEntry {
                case_uid,
                group,
                path: case_path,
                status: messages.status.clone(),
                message_ids: messages.message_ids().into_iter().collect(),
            });
        }
        for entry in workspace.archived_case_entries()? {
            if !case_json_path(&entry.path).exists() {
                continue;
            }
            let messages = read_case_messages(&entry.path, &entry.case_uid)?;
            if !matches!(messages.status.as_str(), "archiving" | "archived") {
                return Err(crate::error::AppError::new(
                    "case_metadata_invalid",
                    format!(
                        "case {} stored under archived-cases has invalid canonical status {:?}",
                        entry.case_uid, messages.status
                    ),
                ));
            }
            entries.push(CaseIndexEntry {
                case_uid: entry.case_uid,
                group: entry.group,
                path: entry.path,
                status: messages.status.clone(),
                message_ids: messages.message_ids().into_iter().collect(),
            });
        }
        let mut entries_by_message: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (entry_index, entry) in entries.iter().enumerate() {
            for message_id in &entry.message_ids {
                entries_by_message
                    .entry(message_id.clone())
                    .or_default()
                    .push(entry_index);
            }
        }
        Ok(Self {
            root: workspace.root().to_path_buf(),
            entries,
            entries_by_message,
        })
    }

    /// True if any case lists this message.
    pub(super) fn has_any_reference(&self, message_id: &str) -> bool {
        self.entries_by_message.contains_key(message_id)
    }

    /// True if an active case lists this message.
    pub(super) fn has_active_reference(&self, message_id: &str) -> bool {
        self.entries_for_message(message_id)
            .any(|entry| entry.status.is_empty())
    }

    /// True if an archived case lists this message.
    pub(super) fn has_archived_reference(&self, message_id: &str) -> bool {
        self.entries_for_message(message_id)
            .any(|entry| matches!(entry.status.as_str(), "archiving" | "archived"))
    }

    pub(super) fn case_refs_for_message(&self, message_id: &str) -> Vec<CaseReferenceDocument> {
        self.entries_for_message(message_id)
            .map(|entry| CaseReferenceDocument {
                case_uid: entry.case_uid.clone(),
                group: entry.group.clone(),
                case_path: rel_path(&self.root, &entry.path),
            })
            .collect()
    }

    fn entries_for_message(&self, message_id: &str) -> impl Iterator<Item = &CaseIndexEntry> {
        self.entries_by_message
            .get(message_id)
            .into_iter()
            .flat_map(|indices| indices.iter())
            .filter_map(|index| self.entries.get(*index))
    }

    pub(super) fn case_paths_for_messages(&self, message_ids: &[String]) -> Vec<PathBuf> {
        let ids = message_ids.iter().collect::<BTreeSet<_>>();
        let mut paths = Vec::new();
        for entry in &self.entries {
            if entry
                .message_ids
                .iter()
                .any(|message_id| ids.contains(message_id))
            {
                paths.push(entry.path.clone());
            }
        }
        paths
    }
}
