use super::{case_json_path, read_case_messages, Workspace};
use crate::error::Result;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct CaseIndex {
    entries: Vec<CaseIndexEntry>,
}

#[derive(Debug)]
struct CaseIndexEntry {
    path: PathBuf,
    is_archived: bool,
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
            entries.push(CaseIndexEntry {
                path: case_path,
                is_archived: false,
                message_ids: messages.message_ids().into_iter().collect(),
            });
        }
        for entry in workspace.archived_case_entries()? {
            if !case_json_path(&entry.path).exists() {
                continue;
            }
            let messages = read_case_messages(&entry.path, &entry.case_uid)?;
            entries.push(CaseIndexEntry {
                path: entry.path,
                is_archived: true,
                message_ids: messages.message_ids().into_iter().collect(),
            });
        }
        Ok(Self { entries })
    }

    /// True if any case lists this message.
    pub(super) fn has_any_reference(&self, message_id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.message_ids.contains(message_id))
    }

    /// True if an archived case lists this message.
    pub(super) fn has_archived_reference(&self, message_id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.is_archived && entry.message_ids.contains(message_id))
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
