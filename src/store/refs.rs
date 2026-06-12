use super::{case_messages_json_path, case_status, read_case_messages, Workspace};
use crate::error::Result;
use std::collections::BTreeSet;

#[derive(Debug)]
pub(super) struct CaseIndex {
    entries: Vec<CaseIndexEntry>,
}

#[derive(Debug)]
struct CaseIndexEntry {
    status: String,
    message_ids: BTreeSet<String>,
}

impl CaseIndex {
    pub(super) fn build(workspace: &Workspace) -> Result<Self> {
        let mut entries = Vec::new();
        for (case_uid, case_path) in workspace.all_case_entries()? {
            let messages_path = case_messages_json_path(&case_path);
            if !messages_path.exists() {
                continue;
            }
            let messages = read_case_messages(&messages_path, &case_uid)?;
            entries.push(CaseIndexEntry {
                status: case_status(&case_path)?,
                message_ids: messages.message_ids.into_iter().collect(),
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

    /// True if a non-archived (still active) case lists this message.
    pub(super) fn has_active_reference(&self, message_id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.status != "archived" && entry.message_ids.contains(message_id))
    }

    /// True if an archived case lists this message.
    pub(super) fn has_archived_reference(&self, message_id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.status == "archived" && entry.message_ids.contains(message_id))
    }
}
