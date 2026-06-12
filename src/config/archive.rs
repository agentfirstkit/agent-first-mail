use super::defaults::default_archive_message_index_fields;
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveSection {
    #[serde(default)]
    pub message_index: ArchiveMessageIndexSection,
}

impl ArchiveSection {
    pub(super) fn validate(&self) -> Result<()> {
        if self.message_index.item_fields.is_empty() {
            return Err(AppError::new(
                "config_invalid",
                "archive.message_index.item_fields must contain at least one field",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveMessageIndexSection {
    #[serde(default = "default_archive_message_index_fields")]
    pub item_fields: Vec<ArchiveMessageIndexField>,
    #[serde(default)]
    pub sort: ArchiveMessageIndexSort,
}

impl Default for ArchiveMessageIndexSection {
    fn default() -> Self {
        Self {
            item_fields: default_archive_message_index_fields(),
            sort: ArchiveMessageIndexSort::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveMessageIndexField {
    Time,
    From,
    To,
    Subject,
    Summary,
    MessageId,
    ArchiveTime,
    Link,
}

impl ArchiveMessageIndexField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::From => "from",
            Self::To => "to",
            Self::Subject => "subject",
            Self::Summary => "summary",
            Self::MessageId => "message_id",
            Self::ArchiveTime => "archive_time",
            Self::Link => "link",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "time" => Ok(Self::Time),
            "from" => Ok(Self::From),
            "to" => Ok(Self::To),
            "subject" => Ok(Self::Subject),
            "summary" => Ok(Self::Summary),
            "message_id" => Ok(Self::MessageId),
            "archive_time" => Ok(Self::ArchiveTime),
            "link" => Ok(Self::Link),
            _ => Err(AppError::new(
                "invalid_request",
                format!("archive.message_index.item_fields contains unsupported field: {value}"),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveMessageIndexSort {
    #[default]
    DateDesc,
}

impl ArchiveMessageIndexSort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DateDesc => "date_desc",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "date_desc" => Ok(Self::DateDesc),
            _ => Err(AppError::new(
                "invalid_request",
                "archive.message_index.sort expects date_desc",
            )),
        }
    }
}
