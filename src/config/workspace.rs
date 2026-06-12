use super::defaults::{
    default_case_group, default_reason_mode, default_smtp_port, default_timezone_utc_offset,
    default_timezone_utc_offset_option, default_true,
};
use super::validation::validate_language_bcp47;
use crate::error::{AppError, Result};
use agent_first_data::normalize_utc_offset;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseSection {
    #[serde(default = "default_case_group")]
    pub default_group: String,
}

impl Default for CaseSection {
    fn default() -> Self {
        Self {
            default_group: default_case_group(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditSection {
    #[serde(default = "default_reason_mode")]
    pub reason_mode: ReasonMode,
}

impl Default for AuditSection {
    fn default() -> Self {
        Self {
            reason_mode: default_reason_mode(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasonMode {
    Required,
    Optional,
}

impl ReasonMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasonMode::Required => "required",
            ReasonMode::Optional => "optional",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "required" => Ok(Self::Required),
            "optional" => Ok(Self::Optional),
            _ => Err(AppError::new(
                "invalid_request",
                "audit.reason_mode expects required or optional",
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SmtpSection {
    pub host: Option<String>,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    #[serde(default = "default_true")]
    pub starttls: bool,
    #[serde(default)]
    pub tls_wrapper: bool,
    pub username: Option<String>,
    pub password_secret: Option<String>,
    pub password_secret_env: Option<String>,
    pub from: Option<String>,
}

impl Default for SmtpSection {
    fn default() -> Self {
        Self {
            host: None,
            port: default_smtp_port(),
            starttls: true,
            tls_wrapper: false,
            username: None,
            password_secret: None,
            password_secret_env: None,
            from: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSection {
    #[serde(default)]
    pub language_bcp47: Option<String>,
    #[serde(default = "default_timezone_utc_offset_option")]
    pub timezone_utc_offset: Option<String>,
}

impl Default for WorkspaceSection {
    fn default() -> Self {
        Self {
            language_bcp47: None,
            timezone_utc_offset: Some(default_timezone_utc_offset()),
        }
    }
}

impl WorkspaceSection {
    pub(super) fn validate(&self) -> Result<()> {
        if let Some(language) = self.language_bcp47.as_deref() {
            validate_language_bcp47("workspace.language_bcp47", language, "config_invalid")?;
        }
        if let Some(offset) = self.timezone_utc_offset.as_deref() {
            let normalized = normalize_utc_offset(offset).ok_or_else(|| {
                AppError::new(
                    "config_invalid",
                    "workspace.timezone_utc_offset expects UTC or a fixed offset like +08:00",
                )
            })?;
            if normalized != offset {
                return Err(AppError::new(
                    "config_invalid",
                    "workspace.timezone_utc_offset must be canonical UTC or ±HH:MM",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemplateLanguage {
    #[default]
    EnUs,
    ZhCn,
}

impl TemplateLanguage {
    pub const ALL: [Self; 2] = [Self::EnUs, Self::ZhCn];

    pub fn as_str(self) -> &'static str {
        match self {
            TemplateLanguage::EnUs => "en-US",
            TemplateLanguage::ZhCn => "zh-CN",
        }
    }

    pub fn from_bcp47(value: &str) -> Self {
        let lower = value.trim().to_ascii_lowercase();
        if lower == "zh" || lower.starts_with("zh-") {
            Self::ZhCn
        } else {
            Self::EnUs
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub starttls: bool,
    pub tls_wrapper: bool,
    pub username: Option<String>,
    pub password_secret: Option<String>,
    pub from: String,
}
