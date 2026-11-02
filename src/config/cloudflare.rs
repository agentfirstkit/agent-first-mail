use super::validation::validate_password_secret_source;
use super::{MailDirection, PullImportAs};
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CloudflareSection {
    pub account_id: Option<String>,
    pub sending: CloudflareSendingSection,
    pub spool: SpoolControlSection,
    pub r2: R2Section,
    pub queue: QueueSection,
    pub consumer: ConsumerSection,
    pub import_as: PullImportAs,
    pub direction: MailDirection,
}
impl Default for CloudflareSection {
    fn default() -> Self {
        Self {
            account_id: None,
            sending: CloudflareSendingSection::default(),
            spool: SpoolControlSection::default(),
            r2: R2Section::default(),
            queue: QueueSection::default(),
            consumer: ConsumerSection::default(),
            import_as: PullImportAs::Triage,
            direction: MailDirection::Inbound,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct QueueSection {
    pub queue_id: Option<String>,
    pub api_token_secret: Option<String>,
    pub endpoint_url: String,
    pub batch_size: usize,
    pub visibility_timeout_s: u64,
}
impl Default for QueueSection {
    fn default() -> Self {
        Self {
            queue_id: None,
            api_token_secret: None,
            endpoint_url: "https://api.cloudflare.com/client/v4".into(),
            batch_size: 20,
            visibility_timeout_s: 300,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ConsumerSection {
    pub command: Vec<String>,
    pub command_timeout_s: u64,
    pub retry_delay_s: u64,
    pub poll_interval_s: u64,
    pub reconcile_interval_s: u64,
}
impl Default for ConsumerSection {
    fn default() -> Self {
        Self {
            command: vec![],
            command_timeout_s: 300,
            retry_delay_s: 60,
            poll_interval_s: 5,
            reconcile_interval_s: 300,
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CloudflareSendingSection {
    pub api_token_secret: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SpoolControlSection {
    pub control_url: Option<String>,
    pub token_secret: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct R2Section {
    pub bucket: Option<String>,
    pub prefix: String,
    pub imported_prefix: Option<String>,
    pub endpoint_url: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key_secret: Option<String>,
}
impl Default for R2Section {
    fn default() -> Self {
        Self {
            bucket: None,
            prefix: "inbox/".into(),
            imported_prefix: None,
            endpoint_url: None,
            access_key_id: None,
            secret_access_key_secret: None,
        }
    }
}
impl CloudflareSection {
    pub fn is_configured(&self) -> bool {
        self.r2
            .bucket
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    }
    pub(super) fn validate(&self) -> Result<()> {
        validate_password_secret_source(
            "cloudflare.queue.api_token_secret",
            self.queue.api_token_secret.as_deref(),
        )?;
        if !(1..=100).contains(&self.queue.batch_size)
            || !(1..=43_200).contains(&self.queue.visibility_timeout_s)
            || !(1..=86_400).contains(&self.consumer.command_timeout_s)
            || !(1..=43_200).contains(&self.consumer.retry_delay_s)
            || !(1..=3600).contains(&self.consumer.poll_interval_s)
            || !(1..=86_400).contains(&self.consumer.reconcile_interval_s)
            || self
                .consumer
                .command
                .first()
                .is_some_and(|value| value.trim().is_empty())
            || self
                .consumer
                .command
                .iter()
                .any(|value| value.contains('\0'))
        {
            return Err(AppError::new(
                "config_invalid",
                "invalid Cloudflare queue batch, timeout, retry delay, or consumer command",
            ));
        }
        validate_password_secret_source(
            "cloudflare.spool.token_secret",
            self.spool.token_secret.as_deref(),
        )?;
        validate_password_secret_source(
            "cloudflare.sending.api_token_secret",
            self.sending.api_token_secret.as_deref(),
        )?;
        validate_password_secret_source(
            "cloudflare.r2.secret_access_key_secret",
            self.r2.secret_access_key_secret.as_deref(),
        )?;
        if self.is_configured()
            && self
                .account_id
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            && self
                .r2
                .endpoint_url
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AppError::new(
                "config_invalid",
                "cloudflare.account_id or cloudflare.r2.endpoint_url is required when cloudflare.r2.bucket is configured",
            ));
        }
        if self.r2.prefix.is_empty() || !self.r2.prefix.ends_with('/') {
            return Err(AppError::new(
                "config_invalid",
                "cloudflare.r2.prefix must be nonempty and end with /",
            ));
        }
        if let Some(imported) = &self.r2.imported_prefix
            && (imported.is_empty()
                || !imported.ends_with('/')
                || imported.starts_with(&self.r2.prefix)
                || self.r2.prefix.starts_with(imported))
        {
            return Err(AppError::new(
                "config_invalid",
                "cloudflare.r2.imported_prefix must end with / and must not overlap cloudflare.r2.prefix",
            ));
        }
        Ok(())
    }
}
