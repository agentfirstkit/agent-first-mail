use super::MailConfig;
use super::validation::{validate_config_id, validate_steps};
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
}

pub(crate) fn default_draft_send_steps() -> Vec<ActionStep> {
    vec![
        ActionStep::append_to_mailbox_id("drafts"),
        ActionStep::smtp_send(),
        ActionStep::append_to_mailbox_id("sent"),
        ActionStep::add_flags_on(
            vec!["\\Seen".to_string(), "\\Answered".to_string()],
            ActionStepOn::ReplyToMessage,
        ),
    ]
}

fn default_message_unmark_discard_steps() -> Vec<ActionStep> {
    vec![ActionStep::move_to_mailbox_id("inbox")]
}

fn default_message_unmark_discard_rule() -> ActionRule {
    ActionRule {
        steps: default_message_unmark_discard_steps(),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionsSection {
    pub pull: PullActionSection,
    pub case: CaseActions,
    pub draft: DraftActions,
    pub message: MessageActions,
}

impl ActionsSection {
    pub(super) fn validate(&self, config: &MailConfig) -> Result<()> {
        self.pull.validate(config)?;
        validate_steps(config, "actions.case.add.steps", &self.case.add.steps)?;
        validate_steps(config, "actions.draft.save.steps", &self.draft.save.steps)?;
        validate_steps(config, "actions.draft.send.steps", &self.draft.send.steps)?;
        validate_steps(
            config,
            "actions.message.spam.steps",
            &self.message.spam.steps,
        )?;
        validate_steps(
            config,
            "actions.message.trash.steps",
            &self.message.trash.steps,
        )?;
        validate_steps(
            config,
            "actions.message.unspam.steps",
            &self.message.unspam.steps,
        )?;
        validate_steps(
            config,
            "actions.message.untrash.steps",
            &self.message.untrash.steps,
        )?;
        for (id, rule) in &self.message.notification.by_source_mailbox_id {
            validate_config_id("actions.message.notification source id", id)?;
            if !config.mailboxes.contains_key(id) {
                return Err(AppError::new(
                    "config_invalid",
                    format!(
                        "actions.message.notification.by_source_mailbox_id.{id} references unknown mailbox id"
                    ),
                ));
            }
            validate_steps(
                config,
                &format!("actions.message.notification.by_source_mailbox_id.{id}.steps"),
                &rule.steps,
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseActions {
    pub add: ActionRule,
}

impl Default for CaseActions {
    fn default() -> Self {
        Self {
            add: ActionRule { steps: Vec::new() },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DraftActions {
    pub save: ActionRule,
    pub send: ActionRule,
}

impl Default for DraftActions {
    fn default() -> Self {
        Self {
            save: ActionRule {
                steps: vec![ActionStep::append_to_mailbox_id("drafts")],
            },
            send: ActionRule {
                steps: default_draft_send_steps(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageActions {
    pub spam: ActionRule,
    pub trash: ActionRule,
    #[serde(default = "default_message_unmark_discard_rule")]
    pub unspam: ActionRule,
    #[serde(default = "default_message_unmark_discard_rule")]
    pub untrash: ActionRule,
    pub notification: MessageNotificationAction,
}

impl Default for MessageActions {
    fn default() -> Self {
        Self {
            spam: ActionRule {
                steps: vec![
                    ActionStep::add_flags(vec!["\\Seen".to_string(), "$Junk".to_string()]),
                    ActionStep::move_to_mailbox_id("junk"),
                ],
            },
            trash: ActionRule {
                steps: vec![ActionStep::move_to_mailbox_id("trash")],
            },
            unspam: default_message_unmark_discard_rule(),
            untrash: default_message_unmark_discard_rule(),
            notification: MessageNotificationAction::default(),
        }
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
// `default` tolerates partial objects: the document layer's generic
// `set actions.pull.by_mailbox_id.<id>.<field>` creates a new entry with only the
// one field present, so the sibling field must fall back to Default.
#[serde(default, deny_unknown_fields)]
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
pub struct MessageNotificationAction {
    #[serde(default)]
    pub by_source_mailbox_id: BTreeMap<String, ActionRule>,
}

impl Default for MessageNotificationAction {
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
