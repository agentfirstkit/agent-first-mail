use super::SendReceipt;
use crate::config::MailConfig;
use crate::error::{Result, feature_disabled};
use crate::types::PreparedOutbound;

pub fn prepare_send(_: &MailConfig, _: &PreparedOutbound) -> Result<()> {
    Err(feature_disabled("cloudflare", "push"))
}

pub fn send_raw_message(_: &MailConfig, _: &PreparedOutbound) -> Result<SendReceipt> {
    Err(feature_disabled("cloudflare", "push"))
}

pub(crate) fn check_config(_: &MailConfig) -> Result<serde_json::Value> {
    Err(feature_disabled("cloudflare", "remote test"))
}
