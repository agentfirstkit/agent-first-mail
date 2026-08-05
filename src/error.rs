use serde_json::{Map, Value};
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

    pub fn to_protocol_value(&self, duration_ms: u64) -> Value {
        let mut builder = agent_first_data::json_error(self.error_code, &self.message)
            .retryable_if(self.retryable)
            .trace(serde_json::json!({"duration_ms": duration_ms}));
        if let Some(hint) = self.resolved_hint() {
            builder = builder.hint(hint);
        }
        if let Value::Object(fields) = self.protocol_fields() {
            for (name, value) in fields {
                if name != "retryable" {
                    builder = builder.field(&name, value);
                }
            }
        }
        builder
            .build()
            .map(Into::into)
            .unwrap_or_else(|_| serde_json::json!({}))
    }

    pub(crate) fn protocol_fields(&self) -> Value {
        let fields = match &self.details {
            Some(Value::Object(details)) => details.clone(),
            Some(details) => {
                let mut fields = Map::new();
                fields.insert("details".to_string(), details.clone());
                fields
            }
            None => Map::new(),
        };
        Value::Object(fields)
    }

    pub(crate) fn resolved_hint(&self) -> Option<&str> {
        self.hint
            .as_deref()
            .or_else(|| default_hint(self.error_code))
    }

    pub fn to_error_value(&self) -> Value {
        let mut fields = Map::new();
        fields.insert(
            "code".to_string(),
            Value::String(self.error_code.to_string()),
        );
        fields.insert("message".to_string(), Value::String(self.message.clone()));
        fields.insert("retryable".to_string(), Value::Bool(self.retryable));
        if let Some(hint) = self.resolved_hint() {
            fields.insert("hint".to_string(), Value::String(hint.to_string()));
        }
        if let Value::Object(details) = self.protocol_fields() {
            fields.extend(details);
        }
        let mut item = Map::new();
        item.insert("error".to_string(), Value::Object(fields));
        Value::Object(item)
    }
}

#[cfg(any(
    not(feature = "api"),
    not(feature = "demo"),
    not(feature = "review"),
    test
))]
pub(crate) fn feature_disabled(feature: &'static str, command: &'static str) -> AppError {
    AppError::new(
        "feature_disabled",
        format!("`afmail {command}` is unavailable because this binary was built without the `{feature}` feature"),
    )
    .with_hint(format!(
        "Rebuild afmail with `cargo build --features {feature}` or use the default feature set."
    ))
    .with_details(serde_json::json!({
        "feature": feature,
        "command": command,
    }))
}

fn default_hint(error_code: &str) -> Option<&'static str> {
    match error_code {
        "invalid_request" => Some(
            "Run the nearest `afmail ... --help` command and retry with the documented action-first syntax.",
        ),
        "reason_required" => Some("Repeat the command with `--reason TEXT`."),
        "confirm_required" => Some(
            "Review the dry-run or status output, then rerun with `--confirm` when you want to apply changes.",
        ),
        "transaction_incomplete" => Some(
            "Run `afmail doctor` to inspect incomplete transactions before making more workspace changes.",
        ),
        "config_missing" => {
            Some("Run `afmail config show` and set the missing config key before retrying.")
        }
        "config_invalid" => Some(
            "Run `afmail config show`, fix `.afmail/config.json`, or update the key with `afmail config set`.",
        ),
        "unknown_mailbox_id" => Some(
            "Run `afmail config show` to inspect configured mailbox ids, then retry with one of those ids.",
        ),
        "workspace_locked" => Some("Wait for the running afmail command to finish, then retry."),
        "case_not_found" => Some(
            "Check the case ref and whether it is active or archived with `afmail status` and `afmail case list --all`.",
        ),
        "notification_not_found" | "notification_entry_not_found" => Some(
            "Check notification refs with `afmail notification list`, then retry with the correct ref and message id.",
        ),
        "draft_not_found" => Some(
            "List the case drafts directory or recreate the draft with `afmail case draft reply` / `afmail case draft new`.",
        ),
        "draft_invalid" => Some(
            "Fix the draft markdown/frontmatter, then run `afmail case draft validate CASE_REF DRAFT_NAME`.",
        ),
        "imap_connect_failed"
        | "imap_greeting_failed"
        | "imap_login_failed"
        | "imap_tls_failed"
        | "imap_select_failed"
        | "imap_fetch_failed"
        | "imap_search_failed"
        | "imap_list_failed"
        | "imap_capability_failed"
        | "imap_create_failed"
        | "imap_append_failed"
        | "imap_copy_failed"
        | "imap_expunge_failed"
        | "imap_move_failed"
        | "imap_store_failed"
        | "imap_uid_missing"
        | "imap_uidvalidity_changed" => Some(
            "Check IMAP config with `afmail config show`; use `afmail remote test` and `afmail remote folders` to diagnose.",
        ),
        "imap_uidplus_required" => Some(
            "This IMAP server does not support MOVE or UIDPLUS. Manually archive the message or use a server with one of those capabilities before retrying.",
        ),
        "smtp_connect_failed" | "smtp_send_failed" => Some(
            "Check SMTP config with `afmail config show`; preview queued mail with `afmail push --dry-run` before retrying.",
        ),
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

#[cfg(test)]
mod tests {
    use super::{AppError, feature_disabled};

    #[test]
    fn protocol_error_flattens_object_details() {
        let value = AppError::retryable("case_archived", "archived")
            .with_details(serde_json::json!({
                "archived_group": "support",
                "suggested_command": "afmail case unarchive c1 --group support --reason TEXT"
            }))
            .to_protocol_value(12);
        assert_eq!(value["error"]["archived_group"], "support");
        assert_eq!(value["error"]["retryable"], true);
        assert!(value["error"].get("details").is_none());
        assert_eq!(value["trace"]["duration_ms"], 12);
    }

    #[test]
    fn protocol_error_keeps_non_object_details_as_named_field() {
        let value = AppError::new("invalid_request", "invalid")
            .with_details(serde_json::json!("specific detail"))
            .to_protocol_value(1);
        assert_eq!(value["error"]["details"], "specific detail");
        assert!(value["error"]["hint"].is_string());
    }

    #[test]
    fn disabled_feature_error_is_self_describing_protocol_data() {
        let value = feature_disabled("demo", "demo").to_protocol_value(3);
        assert_eq!(value["kind"], "error");
        assert_eq!(value["error"]["code"], "feature_disabled");
        assert_eq!(value["error"]["feature"], "demo");
        assert_eq!(value["error"]["command"], "demo");
        assert!(
            value["error"]["hint"]
                .as_str()
                .is_some_and(|hint| hint.contains("--features demo"))
        );
    }
}
