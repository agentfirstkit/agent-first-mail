use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseMessages {
    pub schema_name: String,
    pub schema_version: u64,
    pub case_uid: String,
    #[serde(default)]
    pub message_ids: Vec<String>,
}

impl CaseMessages {
    pub fn new(case_uid: &str) -> Self {
        Self {
            schema_name: "case_messages".to_string(),
            schema_version: 1,
            case_uid: case_uid.to_string(),
            message_ids: Vec::new(),
        }
    }

    pub fn merge_ids(&mut self, ids: &[String]) {
        for id in ids {
            if !self.message_ids.contains(id) {
                self.message_ids.push(id.clone());
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveMessageItem {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub archived_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveMessages {
    pub schema_name: String,
    pub schema_version: u64,
    pub archive_uid: String,
    pub archive_name: String,
    #[serde(default)]
    pub items: Vec<ArchiveMessageItem>,
}

impl ArchiveMessages {
    pub fn new(archive_uid: &str, archive_name: &str) -> Self {
        Self {
            schema_name: "archive_messages".to_string(),
            schema_version: 1,
            archive_uid: archive_uid.to_string(),
            archive_name: archive_name.to_string(),
            items: Vec::new(),
        }
    }
}
