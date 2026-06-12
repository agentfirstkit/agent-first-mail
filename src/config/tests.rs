use super::*;

#[test]
fn config_defaults_pull_and_tls() {
    let cfg = MailConfig::default();
    assert_eq!(
        cfg.default_pull_ids(),
        vec!["inbox", "sent", "archive", "junk", "trash"]
    );
    assert!(cfg.imap.tls);
    assert!(cfg.smtp.starttls);
    assert_eq!(cfg.case.default_group, "open");
    assert_eq!(cfg.audit.reason_mode, ReasonMode::Required);
    assert_eq!(cfg.workspace.language_bcp47, None);
    assert_eq!(cfg.template_language(), TemplateLanguage::EnUs);
    assert!(cfg.workspace.timezone_utc_offset.is_some());
    assert_eq!(
        cfg.actions
            .message_archive
            .by_source_mailbox_id
            .get("inbox")
            .map(|m| first_move_to_mailbox_id(&m.steps)),
        Some(Some("archive"))
    );
    assert_eq!(
        cfg.pull_action("sent").ok().map(|a| a.direction),
        Some(MailDirection::Outbound)
    );
    assert_eq!(
        cfg.pull_action("sent").ok().map(|a| a.import_as),
        Some(PullImportAs::Triage)
    );
    assert_eq!(
        cfg.pull_action("junk").ok().map(|a| a.import_as),
        Some(PullImportAs::Spam)
    );
}

#[test]
fn default_config_serializes_explicit_mailboxes_and_actions() {
    let cfg = MailConfig::default();
    let value = serde_json::to_value(&cfg).unwrap_or(Value::Null);
    assert_eq!(value["schema_name"], json!("config"));
    assert_eq!(value["schema_version"], json!(1));
    assert!(value.get("code").is_none());
    assert_eq!(
        value["actions"]["pull"]["by_mailbox_id"]["sent"],
        json!({
            "import_as": "triage",
            "direction": "outbound"
        })
    );
    assert_eq!(
        value["actions"]["draft.save"]["steps"],
        json!([{"append_to_mailbox_id": "drafts"}])
    );
    assert_eq!(
        value["actions"]["pull"]["default_mailbox_ids"],
        json!(["inbox", "sent", "archive", "junk", "trash"])
    );
    assert_eq!(value["mailboxes"]["sent"]["mailbox_name"], Value::Null);
    assert_eq!(value["mailboxes"]["sent"]["special_use"], json!("\\Sent"));
    assert!(value["mailboxes"].get("all").is_none());
    assert!(value["mailboxes"].get("flagged").is_none());
    assert!(value.get("imap_mailboxes").is_none());
    assert!(value.get("pull").is_none());
    assert!(value.get("push").is_none());
    assert!(value.get("ui").is_none());
    assert!(value.get("timezone").is_none());
    assert_eq!(value["workspace"]["language_bcp47"], Value::Null);
    assert!(value["workspace"].get("timezone_utc_offset").is_some());
}

#[test]
fn config_rejects_legacy_schema() {
    let raw = serde_json::json!({"schema_name":"config","schema_version":1,"imap_host":"imap.example.com"});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
    let raw =
        serde_json::json!({"schema_name":"config","schema_version":1,"pull":{"folders":["INBOX"]}});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
    let raw = serde_json::json!({"schema_name":"config","schema_version":1,"special_use":{}});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
    let raw = serde_json::json!({"schema_name":"config","schema_version":1,"imap_mailboxes":{}});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
    let raw = serde_json::json!({"schema_name":"config","schema_version":1,"push":{}});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
    let raw =
        serde_json::json!({"schema_name":"config","schema_version":1,"ui":{"language":"en-US"}});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
    let raw = serde_json::json!({"schema_name":"config","schema_version":1,"timezone":"UTC"});
    let err = reject_legacy_config(&raw);
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
}

#[test]
fn config_set_and_get_key() {
    let mut cfg = MailConfig::default();
    assert!(cfg.set_key("imap.tls", &["false".to_string()]).is_ok());
    assert_eq!(cfg.get_key("imap.tls"), Ok(json!(false)));
    assert!(cfg
        .set_key("mailboxes.extra.mailbox_name", &["Extra".to_string()])
        .is_ok());
    assert!(cfg
        .set_key(
            "actions.pull.by_mailbox_id.extra.direction",
            &["outbound".to_string()]
        )
        .is_ok());
    assert_eq!(
        cfg.get_key("actions.pull.by_mailbox_id.extra.direction"),
        Ok(json!("outbound"))
    );
    assert!(cfg
        .set_key(
            "actions.pull.by_mailbox_id.junk.import_as",
            &["spam".to_string()]
        )
        .is_ok());
    assert_eq!(
        cfg.get_key("actions.pull.by_mailbox_id.junk.import_as"),
        Ok(json!("spam"))
    );
    assert!(cfg
        .set_key(
            "actions.message.archive.by_source_mailbox_id.inbox.move_to_mailbox_id",
            &["archive".to_string()]
        )
        .is_ok());
    assert_eq!(
        cfg.get_key("actions.message.archive.by_source_mailbox_id.inbox.move_to_mailbox_id"),
        Ok(json!("archive"))
    );
    assert!(cfg
        .set_key(
            "archive.message_index.item_fields",
            &["summary".to_string(), "link".to_string()]
        )
        .is_ok());
    assert_eq!(
        cfg.get_key("archive.message_index.item_fields"),
        Ok(json!(["summary", "link"]))
    );
    assert!(cfg
        .set_key("audit.reason_mode", &["optional".to_string()])
        .is_ok());
    assert_eq!(cfg.get_key("audit.reason_mode"), Ok(json!("optional")));
    assert!(cfg
        .set_key("workspace.language_bcp47", &["zh-CN".to_string()])
        .is_ok());
    assert_eq!(cfg.get_key("workspace.language_bcp47"), Ok(json!("zh-CN")));
    assert!(cfg
        .set_key("workspace.language_bcp47", &["fr-FR".to_string()])
        .is_ok());
    assert_eq!(cfg.template_language(), TemplateLanguage::EnUs);
    assert!(cfg
        .set_key("workspace.language_bcp47", &["not a tag".to_string()])
        .is_err());
    assert!(cfg
        .set_key("workspace.language_bcp47", &["null".to_string()])
        .is_ok());
    assert_eq!(cfg.get_key("workspace.language_bcp47"), Ok(Value::Null));
    assert!(cfg
        .set_key("imap.password_secret", &["secret".to_string()])
        .is_ok());
    assert_eq!(cfg.get_key("imap.password_secret"), Ok(json!("secret")));
    assert!(cfg
        .set_key(
            "imap.password_secret_env",
            &["AFMAIL_IMAP_PASSWORD_SECRET".to_string()]
        )
        .is_ok());
    assert_eq!(
        cfg.get_key("imap.password_secret_env"),
        Ok(json!("AFMAIL_IMAP_PASSWORD_SECRET"))
    );
    assert_eq!(cfg.get_key("imap.password_secret"), Ok(Value::Null));
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn workspace_timezone_parses_sets_and_resolves() {
    let mut cfg = MailConfig::default();
    cfg.workspace.timezone_utc_offset = Some("+08:00".to_string());
    assert_eq!(cfg.resolved_timezone_offset().local_minus_utc(), 8 * 3600);
    cfg.workspace.timezone_utc_offset = Some("-05:30".to_string());
    assert_eq!(
        cfg.resolved_timezone_offset().local_minus_utc(),
        -(5 * 3600 + 30 * 60)
    );
    cfg.workspace.timezone_utc_offset = Some("UTC".to_string());
    assert_eq!(cfg.resolved_timezone_offset().local_minus_utc(), 0);
    assert!(cfg
        .set_key("workspace.timezone_utc_offset", &["+0530".to_string()])
        .is_ok());
    assert_eq!(
        cfg.get_key("workspace.timezone_utc_offset"),
        Ok(json!("+05:30"))
    );
    assert!(cfg
        .set_key("workspace.timezone_utc_offset", &["+00:00".to_string()])
        .is_ok());
    assert_eq!(
        cfg.get_key("workspace.timezone_utc_offset"),
        Ok(json!("UTC"))
    );
    assert!(cfg
        .set_key("workspace.timezone_utc_offset", &["null".to_string()])
        .is_ok());
    assert_eq!(
        cfg.get_key("workspace.timezone_utc_offset"),
        Ok(Value::Null)
    );
    assert!(cfg
        .set_key(
            "workspace.timezone_utc_offset",
            &["Asia/Shanghai".to_string()]
        )
        .is_err());
    cfg.workspace.timezone_utc_offset = Some("+0530".to_string());
    assert!(cfg.validate().is_err());
}

#[test]
fn invalid_mailbox_config_fails() {
    let mut cfg = MailConfig::default();
    assert!(cfg
        .set_key(
            "actions.pull.by_mailbox_id.junk.import_as",
            &["bogus".to_string()]
        )
        .is_err());
    assert!(cfg
        .set_key(
            "actions.pull.by_mailbox_id.sent.direction",
            &["sideways".to_string()]
        )
        .is_err());
    if let Some(entry) = cfg
        .actions
        .message_archive
        .by_source_mailbox_id
        .get_mut("inbox")
    {
        entry.steps = vec![ActionStep::move_to_mailbox_id("missing")];
    }
    let err = cfg.validate();
    assert!(err.is_err());
    assert_eq!(err.err().map(|e| e.error_code), Some("config_invalid"));
}

#[test]
fn empty_mailboxes_cannot_default_pull() {
    let mut cfg = MailConfig::default();
    cfg.imap.host = Some("imap.example.com".to_string());
    cfg.imap.username = Some("me@example.com".to_string());
    cfg.imap.password_secret = Some("secret".to_string());
    cfg.mailboxes.clear();
    assert!(cfg.require_imap().is_ok());
    assert!(cfg.selected_pull_ids(&[]).is_err());
}

#[test]
fn password_secret_sources_are_explicit_by_field() {
    assert!(validate_password_secret_source(
        "imap.password_secret",
        Some("secret"),
        "imap.password_secret_env",
        None
    )
    .is_ok());
    assert!(validate_password_secret_source(
        "imap.password_secret",
        None,
        "imap.password_secret_env",
        Some("AFMAIL_IMAP_PASSWORD_SECRET")
    )
    .is_ok());
    assert!(validate_password_secret_source(
        "imap.password_secret",
        Some("secret"),
        "imap.password_secret_env",
        Some("AFMAIL_IMAP_PASSWORD_SECRET")
    )
    .is_err());
    assert!(validate_password_secret_source(
        "imap.password_secret",
        Some("literal:secret"),
        "imap.password_secret_env",
        None
    )
    .is_err());
    assert!(validate_password_secret_source(
        "imap.password_secret",
        None,
        "imap.password_secret_env",
        Some("IMAP_PASSWORD")
    )
    .is_err());
}
