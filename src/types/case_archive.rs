use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{
    CaseAgentSuggestion, CaseHumanSuggestion, NotificationAgentSuggestion,
    NotificationHumanSuggestion,
};

pub const CASE_SCHEMA_NAME: &str = "case";
pub const NOTIFICATION_SCHEMA_NAME: &str = "notification";
pub const MESSAGE_COLLECTION_SCHEMA_VERSION: u64 = 2;
pub const SUGGESTIBLE_COLLECTION_SCHEMA_VERSION: u64 = 3;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageCollectionItem {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub added_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageCollection {
    pub schema_name: String,
    pub schema_version: u64,
    pub collection_uid: String,
    pub collection_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_rfc3339: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_rfc3339: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_rfc3339: Option<String>,
    #[serde(default)]
    pub items: Vec<MessageCollectionItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_suggestion: Option<CollectionAgentSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_suggestion: Option<CollectionHumanSuggestion>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum CollectionAgentSuggestion {
    Case(CaseAgentSuggestion),
    Notification(NotificationAgentSuggestion),
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum CollectionHumanSuggestion {
    Case(CaseHumanSuggestion),
    Notification(NotificationHumanSuggestion),
}

pub type CaseMessages = MessageCollection;
pub type NotificationMessages = MessageCollection;
pub type NotificationMessageItem = MessageCollectionItem;

impl MessageCollection {
    pub fn new(schema_name: &str, collection_uid: &str, collection_name: &str) -> Self {
        Self {
            schema_name: schema_name.to_string(),
            schema_version: MESSAGE_COLLECTION_SCHEMA_VERSION,
            collection_uid: collection_uid.to_string(),
            collection_name: collection_name.to_string(),
            status: String::new(),
            tags: Vec::new(),
            created_rfc3339: None,
            updated_rfc3339: None,
            archived_rfc3339: None,
            items: Vec::new(),
            agent_suggestion: None,
            human_suggestion: None,
        }
    }

    pub fn new_case(case_uid: &str, case_name: &str, now_rfc3339: &str) -> Self {
        let mut collection = Self::new(CASE_SCHEMA_NAME, case_uid, case_name);
        collection.schema_version = SUGGESTIBLE_COLLECTION_SCHEMA_VERSION;
        collection.created_rfc3339 = Some(now_rfc3339.to_string());
        collection.updated_rfc3339 = Some(now_rfc3339.to_string());
        collection
    }

    pub fn new_notification(
        notification_uid: &str,
        notification_name: &str,
        now_rfc3339: &str,
    ) -> Self {
        let mut collection = Self::new(
            NOTIFICATION_SCHEMA_NAME,
            notification_uid,
            notification_name,
        );
        collection.schema_version = SUGGESTIBLE_COLLECTION_SCHEMA_VERSION;
        collection.created_rfc3339 = Some(now_rfc3339.to_string());
        collection.updated_rfc3339 = Some(now_rfc3339.to_string());
        collection
    }

    pub fn message_ids(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|item| item.message_id.clone())
            .collect()
    }

    pub fn contains_message(&self, message_id: &str) -> bool {
        self.items.iter().any(|item| item.message_id == message_id)
    }

    pub(crate) fn duplicate_message_id(&self) -> Option<&str> {
        let mut seen = std::collections::BTreeSet::new();
        self.items
            .iter()
            .map(|item| item.message_id.as_str())
            .find(|message_id| !seen.insert(*message_id))
    }

    pub fn upsert_item(
        &mut self,
        message_id: &str,
        summary: Option<&str>,
        added_rfc3339: &str,
    ) -> bool {
        let summary = clean_summary(summary);
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.message_id == message_id)
        {
            if summary.is_some() {
                item.summary = summary;
            }
            return false;
        }
        self.items.push(MessageCollectionItem {
            message_id: message_id.to_string(),
            summary,
            added_rfc3339: added_rfc3339.to_string(),
        });
        true
    }

    pub fn merge_items(&mut self, items: &[MessageCollectionItem]) {
        for item in items {
            if self.contains_message(&item.message_id) {
                continue;
            }
            self.items.push(item.clone());
        }
    }
}

fn clean_summary(summary: Option<&str>) -> Option<String> {
    summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
