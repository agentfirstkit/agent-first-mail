use super::defaults::{
    default_contact_group, default_reason_mode, default_smtp_port, default_timezone_utc_offset,
    default_timezone_utc_offset_option, default_true,
};
use super::validation::validate_language_bcp47;
use crate::error::{AppError, Result};
use agent_first_data::normalize_utc_offset;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseSection {}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContactSection {
    #[serde(default = "default_contact_group")]
    pub default_group: String,
}

impl Default for ContactSection {
    fn default() -> Self {
        Self {
            default_group: default_contact_group(),
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
    DeDe,
    EsEs,
    FrFr,
    JaJp,
    KoKr,
    PtBr,
    ZhCn,
    ZhTw,
}

impl TemplateLanguage {
    pub const ALL: [Self; 9] = [
        Self::EnUs,
        Self::DeDe,
        Self::EsEs,
        Self::FrFr,
        Self::JaJp,
        Self::KoKr,
        Self::PtBr,
        Self::ZhCn,
        Self::ZhTw,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TemplateLanguage::EnUs => "en-US",
            TemplateLanguage::DeDe => "de-DE",
            TemplateLanguage::EsEs => "es-ES",
            TemplateLanguage::FrFr => "fr-FR",
            TemplateLanguage::JaJp => "ja-JP",
            TemplateLanguage::KoKr => "ko-KR",
            TemplateLanguage::PtBr => "pt-BR",
            TemplateLanguage::ZhCn => "zh-CN",
            TemplateLanguage::ZhTw => "zh-TW",
        }
    }

    pub fn from_bcp47(value: &str) -> Self {
        let lower = value.trim().to_ascii_lowercase();
        if lower == "de" || lower.starts_with("de-") {
            return Self::DeDe;
        }
        if lower == "es" || lower.starts_with("es-") {
            return Self::EsEs;
        }
        if lower == "fr" || lower.starts_with("fr-") {
            return Self::FrFr;
        }
        if lower == "ja" || lower.starts_with("ja-") {
            return Self::JaJp;
        }
        if lower == "ko" || lower.starts_with("ko-") {
            return Self::KoKr;
        }
        if lower == "pt" || lower.starts_with("pt-") {
            return Self::PtBr;
        }
        if lower == "zh" || lower.starts_with("zh-") {
            // Traditional script (Hant) or a Traditional-using region wins;
            // bare `zh` and Simplified markers fall through to zh-CN.
            if lower.contains("hant")
                || lower.starts_with("zh-tw")
                || lower.starts_with("zh-hk")
                || lower.starts_with("zh-mo")
            {
                return Self::ZhTw;
            }
            return Self::ZhCn;
        }
        Self::EnUs
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
}

#[cfg(test)]
mod tests {
    use super::TemplateLanguage::{self, *};

    #[test]
    fn from_bcp47_resolves_language_script_and_region() {
        // Plain language subtags and language-region tags.
        assert_eq!(TemplateLanguage::from_bcp47("en"), EnUs);
        assert_eq!(TemplateLanguage::from_bcp47("en-US"), EnUs);
        assert_eq!(TemplateLanguage::from_bcp47("de"), DeDe);
        assert_eq!(TemplateLanguage::from_bcp47("de-DE"), DeDe);
        assert_eq!(TemplateLanguage::from_bcp47("fr-CA"), FrFr);
        assert_eq!(TemplateLanguage::from_bcp47("pt-BR"), PtBr);

        // Chinese: bare `zh` and Simplified markers (script Hans, CN/SG regions)
        // resolve to Simplified; Traditional script (Hant) or a Traditional-using
        // region (TW/HK/MO) resolves to Traditional. Script outranks region.
        assert_eq!(TemplateLanguage::from_bcp47("zh"), ZhCn);
        assert_eq!(TemplateLanguage::from_bcp47("zh-CN"), ZhCn);
        assert_eq!(TemplateLanguage::from_bcp47("zh-Hans"), ZhCn);
        assert_eq!(TemplateLanguage::from_bcp47("zh-SG"), ZhCn);
        assert_eq!(TemplateLanguage::from_bcp47("zh-TW"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("zh-HK"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("zh-MO"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("zh-Hant"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("zh-Hant-TW"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("zh-Hans-TW"), ZhCn);

        // BCP-47 matching is case-insensitive; surrounding whitespace is ignored.
        assert_eq!(TemplateLanguage::from_bcp47("ZH-TW"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("zh-hant"), ZhTw);
        assert_eq!(TemplateLanguage::from_bcp47("  zh-TW  "), ZhTw);

        // Unrecognized or malformed tags fall back to the default (English).
        assert_eq!(TemplateLanguage::from_bcp47("xx-YY"), EnUs);
        assert_eq!(TemplateLanguage::from_bcp47(""), EnUs);
    }

    #[test]
    fn as_str_round_trips_through_from_bcp47() {
        for language in TemplateLanguage::ALL {
            assert_eq!(TemplateLanguage::from_bcp47(language.as_str()), language);
        }
    }
}
