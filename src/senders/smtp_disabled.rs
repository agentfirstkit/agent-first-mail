use crate::config::{MailConfig, SmtpConfig};
use crate::error::{Result, feature_disabled};

pub fn prepare_send(_: &MailConfig, _: &str, _: &[String]) -> Result<()> {
    Err(feature_disabled("smtp", "push"))
}

pub fn send_raw_message(_: &MailConfig, _: &str, _: &[String], _: &[u8]) -> Result<()> {
    Err(feature_disabled("smtp", "push"))
}

pub(crate) fn remote_test(_: &SmtpConfig) -> Result<serde_json::Value> {
    Err(feature_disabled("smtp", "remote test"))
}
