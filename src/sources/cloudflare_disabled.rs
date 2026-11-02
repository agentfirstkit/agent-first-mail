use super::imap::PullOptions;
use crate::config::MailConfig;
use crate::error::{AppError, Result};
use crate::progress::ProgressCallback;
use serde_json::Value;
use std::path::Path;
fn disabled() -> AppError {
    AppError::new(
        "feature_disabled",
        "Cloudflare support requires the cloudflare Cargo feature",
    )
}
pub(crate) fn remote_test(_: &MailConfig) -> Result<Value> {
    Err(disabled())
}
pub(crate) fn pull(
    _: &Path,
    _: &MailConfig,
    _: PullOptions,
    _: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    Err(disabled())
}
pub(crate) fn scaffold(_: &Path) -> Result<Value> {
    Err(disabled())
}
pub(crate) fn consume(_: &Path, _: bool) -> Result<Value> {
    Err(disabled())
}
