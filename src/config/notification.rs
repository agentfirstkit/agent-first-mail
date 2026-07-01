use super::defaults::default_notification_index_fields;
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotificationSection {
    #[serde(default)]
    pub message_index: NotificationIndexSection,
}

impl NotificationSection {
    pub(super) fn validate(&self) -> Result<()> {
        if self.message_index.item_fields.is_empty() {
            return Err(AppError::new(
                "config_invalid",
                "notification.message_index.item_fields must contain at least one field",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NotificationIndexSection {
    #[serde(default = "default_notification_index_fields")]
    pub item_fields: Vec<NotificationIndexField>,
    #[serde(default)]
    pub sort: NotificationIndexSort,
}

impl Default for NotificationIndexSection {
    fn default() -> Self {
        Self {
            item_fields: default_notification_index_fields(),
            sort: NotificationIndexSort::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationIndexField {
    Time,
    From,
    To,
    Subject,
    Summary,
    MessageId,
    NotificationTime,
    Link,
}

impl NotificationIndexField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::From => "from",
            Self::To => "to",
            Self::Subject => "subject",
            Self::Summary => "summary",
            Self::MessageId => "message_id",
            Self::NotificationTime => "notification_time",
            Self::Link => "link",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationIndexSort {
    #[default]
    DateDesc,
}

impl NotificationIndexSort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DateDesc => "date_desc",
        }
    }
}
