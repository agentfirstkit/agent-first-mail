mod access;
mod actions;
mod archive;
mod defaults;
mod identity;
mod mailbox;
mod validation;
mod workspace;

pub(crate) use actions::default_draft_send_steps;
pub use actions::{
    ActionRule, ActionStep, ActionStepOn, ActionsSection, MailDirection, MessageArchiveAction,
    PullActionSection, PullImportAs, PullMailboxAction,
};
pub use archive::{
    ArchiveMessageIndexField, ArchiveMessageIndexSection, ArchiveMessageIndexSort, ArchiveSection,
};
pub use identity::{IdentityConfig, MessageIdentityMatch, ResolvedIdentity};
pub use mailbox::{
    special_use_kinds, ImapConfig, ImapMailboxConfig, ImapSection, MailboxResolveRule,
    SpecialUseKind, SpecialUseSource, SpecialUseTarget,
};
pub use workspace::{
    AuditSection, CaseSection, ContactSection, ReasonMode, SmtpConfig, SmtpSection,
    TemplateLanguage, WorkspaceSection,
};

use crate::error::{AppError, Result};
use chrono::{FixedOffset, Offset};
use defaults::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use validation::*;

const DEFAULT_LANGUAGE_BCP47: &str = "en-US";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MailConfig {
    pub schema_name: String,
    pub schema_version: u64,
    pub workspace: WorkspaceSection,
    pub imap: ImapSection,
    pub mailboxes: BTreeMap<String, ImapMailboxConfig>,
    pub actions: ActionsSection,
    pub identities: Vec<IdentityConfig>,
    #[serde(default)]
    pub case: CaseSection,
    #[serde(default)]
    pub archive: ArchiveSection,
    pub audit: AuditSection,
    pub smtp: SmtpSection,
    #[serde(default)]
    pub contact: ContactSection,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            schema_name: default_schema_name(),
            schema_version: 1,
            workspace: WorkspaceSection::default(),
            imap: ImapSection::default(),
            mailboxes: default_mailbox_configs(),
            actions: ActionsSection::default(),
            identities: default_identities(),
            case: CaseSection::default(),
            archive: ArchiveSection::default(),
            audit: AuditSection::default(),
            smtp: SmtpSection::default(),
            contact: ContactSection::default(),
        }
    }
}

fn merge_with_defaults(override_value: Value) -> Result<Value> {
    let mut value = MailConfig::default_template_value()?;
    merge_json_value(&mut value, override_value);
    Ok(value)
}

fn merge_json_value(base: &mut Value, override_value: Value) {
    match (base, override_value) {
        (Value::Object(base_map), Value::Object(override_map)) => {
            for (key, value) in override_map {
                if let Some(base_value) = base_map.get_mut(&key) {
                    merge_json_value(base_value, value);
                } else {
                    base_map.insert(key, value);
                }
            }
        }
        (base_value, override_value) => {
            *base_value = override_value;
        }
    }
}

fn sparse_diff(value: &Value, default: &Value) -> Option<Value> {
    if value == default {
        return None;
    }
    match (value, default) {
        (Value::Object(value_map), Value::Object(default_map)) => {
            let mut diff = Map::new();
            for (key, child) in value_map {
                match default_map.get(key) {
                    Some(default_child) => {
                        if let Some(child_diff) = sparse_diff(child, default_child) {
                            diff.insert(key.clone(), child_diff);
                        }
                    }
                    None => {
                        diff.insert(key.clone(), child.clone());
                    }
                }
            }
            if diff.is_empty() {
                None
            } else {
                Some(Value::Object(diff))
            }
        }
        _ => Some(value.clone()),
    }
}

impl MailConfig {
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let path = workspace_root.join(".afmail/config.json");
        let data = fs::read_to_string(&path).map_err(|e| AppError::io("read config", &e))?;
        let raw: Value =
            serde_json::from_str(&data).map_err(|e| AppError::json("parse config", &e))?;
        reject_legacy_config(&raw)?;
        let config: Self = serde_json::from_value(merge_with_defaults(raw)?)
            .map_err(|e| AppError::new("config_invalid", format!("invalid config schema: {e}")))?;
        config.validate()?;
        config.validate_identity_files(workspace_root)?;
        Ok(config)
    }

    pub fn write(&self, workspace_root: &Path) -> Result<()> {
        self.validate()?;
        let path = workspace_root.join(".afmail/config.json");
        crate::util::write_json_pretty(&path, &self.sparse_value()?)
    }

    pub fn default_template_value() -> Result<Value> {
        serde_json::to_value(Self::default()).map_err(|e| AppError::json("serialize config", &e))
    }

    pub fn sparse_value(&self) -> Result<Value> {
        let value =
            serde_json::to_value(self).map_err(|e| AppError::json("serialize config", &e))?;
        let default = Self::default_template_value()?;
        Ok(sparse_diff(&value, &default).unwrap_or_else(|| json!({})))
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_name != "config" || self.schema_version != 1 {
            return Err(AppError::new(
                "config_invalid",
                format!(
                    "unsupported config schema: {} v{}",
                    self.schema_name, self.schema_version
                ),
            ));
        }
        for (id, mailbox) in &self.mailboxes {
            validate_config_id("mailboxes id", id)?;
            mailbox.validate(id)?;
        }
        self.workspace.validate()?;
        self.actions.validate(self)?;
        self.validate_identities()?;
        self.archive.validate()?;
        validate_password_secret_source(
            "imap.password_secret",
            self.imap.password_secret.as_deref(),
            "imap.password_secret_env",
            self.imap.password_secret_env.as_deref(),
        )?;
        validate_password_secret_source(
            "smtp.password_secret",
            self.smtp.password_secret.as_deref(),
            "smtp.password_secret_env",
            self.smtp.password_secret_env.as_deref(),
        )?;
        Ok(())
    }

    pub fn require_imap(&self) -> Result<ImapConfig> {
        self.require_imap_with_mailboxes(Vec::new())
    }

    pub fn require_imap_with_mailboxes(&self, mailboxes: Vec<String>) -> Result<ImapConfig> {
        Ok(ImapConfig {
            host: self
                .imap
                .host
                .clone()
                .ok_or_else(|| AppError::new("config_missing", "imap.host is required"))?,
            port: self.imap.port,
            tls: self.imap.tls,
            username: self
                .imap
                .username
                .clone()
                .ok_or_else(|| AppError::new("config_missing", "imap.username is required"))?,
            password_secret: resolve_password_secret(
                "imap.password_secret",
                self.imap.password_secret.as_deref(),
                "imap.password_secret_env",
                self.imap.password_secret_env.as_deref(),
            )?,
            mailboxes,
        })
    }

    pub fn require_smtp(&self) -> Result<SmtpConfig> {
        Ok(SmtpConfig {
            host: self
                .smtp
                .host
                .clone()
                .ok_or_else(|| AppError::new("config_missing", "smtp.host is required"))?,
            port: self.smtp.port,
            starttls: self.smtp.starttls,
            tls_wrapper: self.smtp.tls_wrapper,
            username: self.smtp.username.clone(),
            password_secret: resolve_optional_password_secret(
                "smtp.password_secret",
                self.smtp.password_secret.as_deref(),
                "smtp.password_secret_env",
                self.smtp.password_secret_env.as_deref(),
            )?,
        })
    }

    pub fn mailbox_ids(&self) -> Vec<String> {
        self.mailboxes.keys().cloned().collect()
    }

    pub fn default_pull_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for id in &self.actions.pull.default_mailbox_ids {
            if self.mailboxes.contains_key(id) && !out.iter().any(|existing| existing == id) {
                out.push(id.clone());
            }
        }
        out
    }

    pub fn selected_pull_ids(&self, ids: &[String]) -> Result<Vec<String>> {
        let selected = if ids.is_empty() {
            self.default_pull_ids()
        } else {
            let mut out = Vec::new();
            for id in ids {
                if !self.mailboxes.contains_key(id) {
                    return Err(AppError::new(
                        "unknown_mailbox_id",
                        format!(
                            "unknown IMAP mailbox id: {id}; available ids: {}",
                            self.mailbox_ids().join(", ")
                        ),
                    ));
                }
                if !out.iter().any(|existing| existing == id) {
                    out.push(id.clone());
                }
            }
            out
        };
        if selected.is_empty() {
            return Err(AppError::new(
                "config_invalid",
                "actions.pull.default_mailbox_ids is empty; pass configured ids explicitly",
            ));
        }
        Ok(selected)
    }

    pub fn mailbox(&self, id: &str) -> Result<&ImapMailboxConfig> {
        self.mailboxes.get(id).ok_or_else(|| {
            AppError::new(
                "unknown_mailbox_id",
                format!(
                    "unknown IMAP mailbox id: {id}; available ids: {}",
                    self.mailbox_ids().join(", ")
                ),
            )
        })
    }

    pub fn offline_mailbox_name(&self, id: &str) -> Result<String> {
        let mailbox = self.mailbox(id)?;
        mailbox.offline_mailbox_name().ok_or_else(|| {
            AppError::new(
                "config_invalid",
                format!("mailboxes.{id} does not resolve to a mailbox name offline"),
            )
        })
    }

    pub fn pull_action(&self, id: &str) -> Result<&PullMailboxAction> {
        self.actions.pull.by_mailbox_id.get(id).ok_or_else(|| {
            AppError::new(
                "config_invalid",
                format!("actions.pull.by_mailbox_id.{id} is missing"),
            )
        })
    }

    pub fn mailbox_id_for_special_use(&self, kind: SpecialUseKind) -> Option<String> {
        self.mailboxes
            .iter()
            .find(|(id, mailbox)| {
                id.as_str() == kind.as_str() || mailbox.contains_special_use(kind)
            })
            .map(|(id, _)| id.clone())
    }

    pub fn offline_mailbox_name_for_special_use(&self, kind: SpecialUseKind) -> Result<String> {
        if let Some(id) = self.mailbox_id_for_special_use(kind) {
            return self.offline_mailbox_name(&id);
        }
        Ok(kind.fallback_names()[0].to_string())
    }

    pub fn special_use_folder(&self, kind: SpecialUseKind) -> Option<String> {
        self.offline_mailbox_name_for_special_use(kind).ok()
    }

    pub fn special_use_flag(&self, kind: SpecialUseKind) -> Option<String> {
        match kind {
            SpecialUseKind::Flagged => Some("\\Flagged".to_string()),
            SpecialUseKind::Junk => Some("$Junk".to_string()),
            _ => None,
        }
    }

    pub fn matching_mailbox_ids_offline(&self, mailbox_name: &str) -> Vec<String> {
        self.mailboxes
            .iter()
            .filter_map(|(id, mailbox)| {
                mailbox
                    .matches_mailbox_offline(mailbox_name)
                    .then_some(id.clone())
            })
            .collect()
    }

    pub fn resolved_language_bcp47(&self) -> &str {
        self.workspace
            .language_bcp47
            .as_deref()
            .unwrap_or(DEFAULT_LANGUAGE_BCP47)
    }

    pub fn template_language(&self) -> TemplateLanguage {
        TemplateLanguage::from_bcp47(self.resolved_language_bcp47())
    }

    pub fn resolved_timezone_utc_offset(&self) -> String {
        self.workspace
            .timezone_utc_offset
            .clone()
            .unwrap_or_else(default_timezone_utc_offset)
    }

    pub fn resolved_timezone_offset(&self) -> FixedOffset {
        fixed_offset_from_utc_offset(&self.resolved_timezone_utc_offset())
            .unwrap_or_else(|| chrono::Utc.fix())
    }
}

#[cfg(test)]
mod tests;
