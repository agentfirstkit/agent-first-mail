use super::validation::{validate_config_id, validate_steps};
use super::MailConfig;
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullImportAs {
    #[default]
    Triage,
    Spam,
    Trashed,
}

impl PullImportAs {
    pub fn as_str(self) -> &'static str {
        match self {
            PullImportAs::Triage => "triage",
            PullImportAs::Spam => "spam",
            PullImportAs::Trashed => "trashed",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "triage" => Ok(Self::Triage),
            "spam" => Ok(Self::Spam),
            "trashed" => Ok(Self::Trashed),
            _ => Err(AppError::new(
                "invalid_request",
                "actions.pull.by_mailbox_id.<id>.import_as expects one of: triage, spam, trashed",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MailDirection {
    #[default]
    Inbound,
    Outbound,
}

impl MailDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            MailDirection::Inbound => "inbound",
            MailDirection::Outbound => "outbound",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "inbound" => Ok(Self::Inbound),
            "outbound" => Ok(Self::Outbound),
            _ => Err(AppError::new(
                "invalid_request",
                "direction expects inbound or outbound",
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionsSection {
    pub pull: PullActionSection,
    #[serde(rename = "case.add")]
    pub case_add: ActionRule,
    #[serde(rename = "draft.save")]
    pub draft_save: ActionRule,
    #[serde(rename = "draft.send")]
    pub draft_send: ActionRule,
    #[serde(rename = "message.spam")]
    pub message_spam: ActionRule,
    #[serde(rename = "message.trash")]
    pub message_trash: ActionRule,
    #[serde(rename = "message.archive")]
    pub message_archive: MessageArchiveAction,
}

impl Default for ActionsSection {
    fn default() -> Self {
        Self {
            pull: PullActionSection::default(),
            case_add: ActionRule { steps: Vec::new() },
            draft_save: ActionRule {
                steps: vec![ActionStep::append_to_mailbox_id("drafts")],
            },
            draft_send: ActionRule {
                steps: vec![
                    ActionStep::smtp_send(),
                    ActionStep::append_to_mailbox_id("sent"),
                    ActionStep::add_flags_on(
                        vec!["\\Seen".to_string(), "\\Answered".to_string()],
                        ActionStepOn::ReplyToMessage,
                    ),
                ],
            },
            message_spam: ActionRule {
                steps: vec![
                    ActionStep::add_flags(vec!["\\Seen".to_string(), "$Junk".to_string()]),
                    ActionStep::move_to_mailbox_id("junk"),
                ],
            },
            message_trash: ActionRule {
                steps: vec![ActionStep::move_to_mailbox_id("trash")],
            },
            message_archive: MessageArchiveAction::default(),
        }
    }
}

impl ActionsSection {
    pub(super) fn validate(&self, config: &MailConfig) -> Result<()> {
        self.pull.validate(config)?;
        validate_steps(config, "actions.case.add.steps", &self.case_add.steps)?;
        validate_steps(config, "actions.draft.save.steps", &self.draft_save.steps)?;
        validate_steps(config, "actions.draft.send.steps", &self.draft_send.steps)?;
        validate_steps(
            config,
            "actions.message.spam.steps",
            &self.message_spam.steps,
        )?;
        validate_steps(
            config,
            "actions.message.trash.steps",
            &self.message_trash.steps,
        )?;
        for (id, rule) in &self.message_archive.by_source_mailbox_id {
            validate_config_id("actions.message.archive source id", id)?;
            if !config.mailboxes.contains_key(id) {
                return Err(AppError::new(
                    "config_invalid",
                    format!("actions.message.archive.by_source_mailbox_id.{id} references unknown mailbox id"),
                ));
            }
            validate_steps(
                config,
                &format!("actions.message.archive.by_source_mailbox_id.{id}.steps"),
                &rule.steps,
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PullActionSection {
    pub default_mailbox_ids: Vec<String>,
    pub by_mailbox_id: BTreeMap<String, PullMailboxAction>,
}

impl Default for PullActionSection {
    fn default() -> Self {
        let mut by_mailbox_id = BTreeMap::new();
        by_mailbox_id.insert(
            "inbox".to_string(),
            PullMailboxAction {
                import_as: PullImportAs::Triage,
                direction: MailDirection::Inbound,
            },
        );
        by_mailbox_id.insert(
            "sent".to_string(),
            PullMailboxAction {
                import_as: PullImportAs::Triage,
                direction: MailDirection::Outbound,
            },
        );
        by_mailbox_id.insert(
            "archive".to_string(),
            PullMailboxAction {
                import_as: PullImportAs::Triage,
                direction: MailDirection::Inbound,
            },
        );
        by_mailbox_id.insert(
            "junk".to_string(),
            PullMailboxAction {
                import_as: PullImportAs::Spam,
                direction: MailDirection::Inbound,
            },
        );
        by_mailbox_id.insert(
            "trash".to_string(),
            PullMailboxAction {
                import_as: PullImportAs::Trashed,
                direction: MailDirection::Inbound,
            },
        );
        by_mailbox_id.insert(
            "drafts".to_string(),
            PullMailboxAction {
                import_as: PullImportAs::Triage,
                direction: MailDirection::Outbound,
            },
        );
        Self {
            // Keep no-arg `afmail pull` broad enough to refresh active work surfaces.
            // Narrowing this to only inbox silently stops syncing sent replies and archived mail.
            default_mailbox_ids: vec![
                "inbox".to_string(),
                "sent".to_string(),
                "archive".to_string(),
                "junk".to_string(),
                "trash".to_string(),
            ],
            by_mailbox_id,
        }
    }
}

impl PullActionSection {
    pub(super) fn validate(&self, config: &MailConfig) -> Result<()> {
        for id in &self.default_mailbox_ids {
            validate_config_id("actions.pull.default_mailbox_ids id", id)?;
            if !config.mailboxes.contains_key(id) {
                return Err(AppError::new(
                    "config_invalid",
                    format!("actions.pull.default_mailbox_ids references unknown mailbox id: {id}"),
                ));
            }
            if !self.by_mailbox_id.contains_key(id) {
                return Err(AppError::new(
                    "config_invalid",
                    format!("actions.pull.by_mailbox_id.{id} is required by default_mailbox_ids"),
                ));
            }
        }
        for id in self.by_mailbox_id.keys() {
            validate_config_id("actions.pull.by_mailbox_id id", id)?;
            if !config.mailboxes.contains_key(id) {
                return Err(AppError::new(
                    "config_invalid",
                    format!("actions.pull.by_mailbox_id.{id} references unknown mailbox id"),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PullMailboxAction {
    pub import_as: PullImportAs,
    pub direction: MailDirection,
}

impl Default for PullMailboxAction {
    fn default() -> Self {
        Self {
            import_as: PullImportAs::Triage,
            direction: MailDirection::Inbound,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionRule {
    #[serde(default)]
    pub steps: Vec<ActionStep>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageArchiveAction {
    #[serde(default)]
    pub by_source_mailbox_id: BTreeMap<String, ActionRule>,
}

impl Default for MessageArchiveAction {
    fn default() -> Self {
        let mut by_source_mailbox_id = BTreeMap::new();
        by_source_mailbox_id.insert(
            "inbox".to_string(),
            ActionRule {
                steps: vec![ActionStep::move_to_mailbox_id("archive")],
            },
        );
        for id in ["sent", "archive", "junk", "trash", "drafts"] {
            by_source_mailbox_id.insert(id.to_string(), ActionRule::default());
        }
        Self {
            by_source_mailbox_id,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionStep {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_to_mailbox_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_to_mailbox_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_send: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<ActionStepOn>,
}

impl ActionStep {
    pub fn add_flags(flags: Vec<String>) -> Self {
        Self {
            add_flags: flags,
            ..Self::default()
        }
    }

    pub fn add_flags_on(flags: Vec<String>, on: ActionStepOn) -> Self {
        Self {
            add_flags: flags,
            on: Some(on),
            ..Self::default()
        }
    }

    pub fn move_to_mailbox_id(id: impl Into<String>) -> Self {
        Self {
            move_to_mailbox_id: Some(id.into()),
            ..Self::default()
        }
    }

    pub fn append_to_mailbox_id(id: impl Into<String>) -> Self {
        Self {
            append_to_mailbox_id: Some(id.into()),
            ..Self::default()
        }
    }

    pub fn smtp_send() -> Self {
        Self {
            smtp_send: Some(BTreeMap::new()),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStepOn {
    ReplyToMessage,
}
