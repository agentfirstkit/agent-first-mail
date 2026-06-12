use serde_json::{json, Value};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq)]
pub struct AppError {
    pub error_code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub hint: Option<String>,
    pub details: Option<Value>,
}

impl AppError {
    pub fn new(error_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
            retryable: false,
            hint: None,
            details: None,
        }
    }

    pub fn retryable(error_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
            retryable: true,
            hint: None,
            details: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn io(context: &str, err: &std::io::Error) -> Self {
        Self::new("store_error", format!("{context}: {err}"))
    }

    pub fn json(context: &str, err: &serde_json::Error) -> Self {
        Self::new("store_error", format!("{context}: {err}"))
    }

    pub fn to_value(&self) -> serde_json::Value {
        let mut value = json!({
            "code": "error",
            "error_code": self.error_code,
            "error": self.message,
            "retryable": self.retryable,
            "trace": {"duration_ms": 0}
        });
        if let Value::Object(map) = &mut value {
            if let Some(hint) = self
                .hint
                .as_deref()
                .or_else(|| default_hint(self.error_code))
            {
                map.insert("hint".to_string(), json!(hint));
            }
            if let Some(details) = &self.details {
                map.insert("details".to_string(), details.clone());
            }
        }
        value
    }
}

fn default_hint(error_code: &str) -> Option<&'static str> {
    match error_code {
        "invalid_request" => {
            Some("Run the nearest `afmail ... --help` command and retry with the documented action-first syntax.")
        }
        "reason_required" => Some("Repeat the command with `--reason TEXT`, or change `audit.reason_mode` if this workspace should not require reasons."),
        "confirm_required" => Some("Review the dry-run or status output, then rerun with `--confirm` when you want to apply changes."),
        "transaction_incomplete" => Some("Run `afmail doctor` to inspect incomplete transactions before making more workspace changes."),
        "config_missing" => Some("Run `afmail config show` and set the missing config key before retrying."),
        "config_invalid" => Some("Run `afmail config show`, fix `.afmail/config.json`, or update the key with `afmail config set`."),
        "unknown_mailbox_id" => Some("Run `afmail config show` to inspect configured mailbox ids, then retry with one of those ids."),
        "workspace_locked" => Some("Wait for the running afmail command to finish, then retry."),
        "case_not_found" => Some("Check the case ref and whether it is active or archived with `afmail status` and `afmail archive list cases`."),
        "archive_not_found" | "archive_entry_not_found" => {
            Some("Check archive refs with `afmail archive list messages`, then retry with the correct ref and message id.")
        }
        "draft_not_found" => Some("List the case drafts directory or recreate the draft with `afmail case reply` / `afmail case draft new`."),
        "draft_invalid" => Some("Fix the draft markdown/frontmatter, then run `afmail case draft validate CASE_REF DRAFT_NAME`."),
        "draft_validation_required" | "draft_changed_since_validation"
        | "draft_changed_since_compose" => {
            Some("Run `afmail case draft validate CASE_REF DRAFT_NAME`, then retry compose.")
        }
        "imap_connect_failed" | "imap_greeting_failed" | "imap_login_failed" | "imap_tls_failed"
        | "imap_select_failed" | "imap_fetch_failed" | "imap_search_failed" | "imap_list_failed"
        | "imap_capability_failed" | "imap_create_failed" | "imap_append_failed"
        | "imap_move_failed" | "imap_store_failed" | "imap_uid_missing" => {
            Some("Check IMAP config with `afmail config show`; use `afmail remote test` and `afmail remote folders` to diagnose.")
        }
        "smtp_connect_failed" | "smtp_send_failed" => {
            Some("Check SMTP config with `afmail config show`; preview queued mail with `afmail push drafts-send --dry-run` before retrying.")
        }
        _ => None,
    }
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code, self.message)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
