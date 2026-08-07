#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
    assert_eq!(cfg.audit.reason_mode, ReasonMode::Required);
    assert_eq!(cfg.identities.len(), 1);
    assert_eq!(
        cfg.default_identity().ok().map(|i| i.identity.as_str()),
        Some("me")
    );
    assert_eq!(cfg.workspace.language_bcp47, None);
    assert_eq!(cfg.template_language(), TemplateLanguage::EnUs);
    assert!(cfg.workspace.timezone_utc_offset.is_some());
    assert_eq!(
        cfg.actions
            .message
            .notification
            .by_source_mailbox_id
            .get("inbox")
            .map(|m| {
                m.steps
                    .iter()
                    .find_map(|step| step.move_to_mailbox_id.as_deref())
            }),
        Some(Some("archive"))
    );
    assert_eq!(
        cfg.actions
            .message
            .unspam
            .steps
            .first()
            .and_then(|step| step.move_to_mailbox_id.as_deref()),
        Some("inbox")
    );
    assert_eq!(
        cfg.actions
            .message
            .untrash
            .steps
            .first()
            .and_then(|step| step.move_to_mailbox_id.as_deref()),
        Some("inbox")
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
        value["actions"]["draft"]["save"]["steps"],
        json!([{"append_to_mailbox_id": "drafts"}])
    );
    assert_eq!(
        value["actions"]["draft"]["send"]["steps"],
        json!([
            {"append_to_mailbox_id": "drafts"},
            {"smtp_send": {}},
            {"append_to_mailbox_id": "sent"},
            {"add_flags": ["\\Seen", "\\Answered"], "on": "reply_to_message"}
        ])
    );
    assert_eq!(
        value["actions"]["message"]["unspam"]["steps"],
        json!([{"move_to_mailbox_id": "inbox"}])
    );
    assert_eq!(
        value["actions"]["message"]["untrash"]["steps"],
        json!([{"move_to_mailbox_id": "inbox"}])
    );
    assert_eq!(
        value["actions"]["pull"]["default_mailbox_ids"],
        json!(["inbox", "sent", "archive", "junk", "trash"])
    );
    assert_eq!(
        value["mailboxes"]["sent"]["resolve"],
        json!([
            {"special_use": "\\Sent"},
            {"mailbox_name": "Sent"},
            {"mailbox_name": "Sent Mail"},
            {"mailbox_name": "Sent Messages"}
        ])
    );
    assert!(value["mailboxes"].get("all").is_none());
    assert!(value["mailboxes"].get("flagged").is_none());
    assert!(value.get("imap_mailboxes").is_none());
    assert!(value.get("pull").is_none());
    assert!(value.get("push").is_none());
    assert!(value.get("ui").is_none());
    assert!(value.get("timezone").is_none());
    assert_eq!(
        value["identities"],
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}])
    );
    assert_eq!(value["workspace"]["language_bcp47"], Value::Null);
    assert!(value["workspace"].get("timezone_utc_offset").is_some());
}

#[test]
fn default_config_sparse_value_is_empty() {
    let cfg = MailConfig::default();
    assert_eq!(cfg.sparse_value().unwrap_or(Value::Null), json!({}));
}

#[test]
fn sparse_config_merges_with_current_defaults() {
    let sparse = json!({
        "imap": {
            "host": "imap.example.com"
        },
        "actions": {
            "draft": {
                "send": {
                    "steps": [
                        {"smtp_send": {}},
                        {"append_to_mailbox_id": "sent"}
                    ]
                }
            }
        }
    });
    let config: MailConfig =
        serde_json::from_value(merge_with_defaults(sparse).unwrap_or(Value::Null))
            .unwrap_or_default();
    assert_eq!(config.imap.host.as_deref(), Some("imap.example.com"));
    assert_eq!(config.imap.port, 993);
    assert_eq!(
        config
            .actions
            .draft
            .save
            .steps
            .first()
            .and_then(|step| step.append_to_mailbox_id.as_deref()),
        Some("drafts")
    );
    assert_eq!(config.actions.draft.send.steps.len(), 2);
    assert!(config.actions.draft.send.steps[0].smtp_send.is_some());
}

#[test]
fn deprecated_case_default_group_is_ignored() {
    let sparse = json!({
        "case": {
            "default_group": "open"
        }
    });
    let merged = merge_with_defaults(sparse).unwrap_or(Value::Null);
    assert_eq!(merged["case"], json!({}));
    let config: MailConfig = serde_json::from_value(merged).unwrap_or_default();
    assert_eq!(config.case, CaseSection::default());
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
    assert!(cfg.set_key("schema_name", &["config".to_string()]).is_ok());
    assert!(cfg.set_key("schema_version", &["1".to_string()]).is_ok());
    assert!(cfg.set_key("imap.tls", &["false".to_string()]).is_ok());
    assert_eq!(cfg.get_key("imap.tls"), Ok(json!(false)));
    assert!(
        cfg.set_key(
            "mailboxes.extra.resolve",
            &[r#"[{"mailbox_name":"Extra"}]"#.to_string()]
        )
        .is_ok()
    );
    assert!(
        cfg.set_key(
            "actions.pull.by_mailbox_id.extra.direction",
            &["outbound".to_string()]
        )
        .is_ok()
    );
    assert_eq!(
        cfg.get_key("actions.pull.by_mailbox_id.extra.direction"),
        Ok(json!("outbound"))
    );
    assert!(
        cfg.set_key(
            "actions.pull.by_mailbox_id.junk.import_as",
            &["spam".to_string()]
        )
        .is_ok()
    );
    assert_eq!(
        cfg.get_key("actions.pull.by_mailbox_id.junk.import_as"),
        Ok(json!("spam"))
    );
    assert!(
        cfg.set_key(
            "actions.message.notification.by_source_mailbox_id.inbox.steps.0.move_to_mailbox_id",
            &["archive".to_string()]
        )
        .is_ok()
    );
    assert_eq!(
        cfg.get_key(
            "actions.message.notification.by_source_mailbox_id.inbox.steps.0.move_to_mailbox_id"
        ),
        Ok(json!("archive"))
    );
    assert!(
        cfg.set_key(
            "actions.case.add.steps",
            &[r#"[{"append_to_mailbox_id":"drafts"}]"#.to_string()]
        )
        .is_ok()
    );
    assert_eq!(
        cfg.get_key("actions.case.add.steps"),
        Ok(json!([{"append_to_mailbox_id": "drafts"}]))
    );
    assert!(
        cfg.set_key(
            "actions.message.notification.by_source_mailbox_id.inbox.steps",
            &[r#"[{"move_to_mailbox_id":"archive"}]"#.to_string()]
        )
        .is_ok()
    );
    assert_eq!(
        cfg.get_key("actions.message.notification.by_source_mailbox_id.inbox.steps"),
        Ok(json!([{"move_to_mailbox_id": "archive"}]))
    );
    let mailboxes_json = serde_json::to_string(&cfg.mailboxes).unwrap_or_default();
    assert!(cfg.set_key("mailboxes", &[mailboxes_json]).is_ok());
    let actions_json = serde_json::to_string(&cfg.actions).unwrap_or_default();
    assert!(cfg.set_key("actions", &[actions_json]).is_ok());
    assert!(
        cfg.set_key("audit.reason_mode", &["optional".to_string()])
            .is_ok()
    );
    assert_eq!(cfg.get_key("audit.reason_mode"), Ok(json!("optional")));
    assert!(
        cfg.set_key("workspace.language_bcp47", &["zh-CN".to_string()])
            .is_ok()
    );
    assert_eq!(cfg.get_key("workspace.language_bcp47"), Ok(json!("zh-CN")));
    assert!(
        cfg.set_key("workspace.language_bcp47", &["fr-FR".to_string()])
            .is_ok()
    );
    assert_eq!(cfg.template_language(), TemplateLanguage::FrFr);
    assert!(
        cfg.set_key("workspace.language_bcp47", &["not a tag".to_string()])
            .is_err()
    );
    assert!(
        cfg.set_key("workspace.language_bcp47", &["null".to_string()])
            .is_ok()
    );
    assert_eq!(cfg.get_key("workspace.language_bcp47"), Ok(Value::Null));
    assert!(
        cfg.set_key("imap.password_secret", &["secret".to_string()])
            .is_ok()
    );
    assert_eq!(cfg.get_key("imap.password_secret"), Ok(json!("secret")));
    // The same key names where the secret is, so switching source is a set,
    // not a move to a sibling key.
    assert!(
        cfg.set_key(
            "imap.password_secret",
            &["env:AFMAIL_IMAP_PASSWORD_SECRET".to_string()]
        )
        .is_ok()
    );
    assert_eq!(
        cfg.get_key("imap.password_secret"),
        Ok(json!("env:AFMAIL_IMAP_PASSWORD_SECRET"))
    );
    assert!(cfg
        .set_key(
            "identities",
            &[r#"[{"identity":"support","name":"Support","email":"support@example.com","default":true}]"#
                .to_string()]
        )
        .is_ok());
    assert_eq!(
        cfg.get_key("identities"),
        Ok(
            json!([{"identity": "support", "name": "Support", "email": "support@example.com", "default": true}])
        )
    );
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
    assert!(
        cfg.set_key("workspace.timezone_utc_offset", &["+0530".to_string()])
            .is_ok()
    );
    assert_eq!(
        cfg.get_key("workspace.timezone_utc_offset"),
        Ok(json!("+05:30"))
    );
    assert!(
        cfg.set_key("workspace.timezone_utc_offset", &["+00:00".to_string()])
            .is_ok()
    );
    assert_eq!(
        cfg.get_key("workspace.timezone_utc_offset"),
        Ok(json!("UTC"))
    );
    assert!(
        cfg.set_key("workspace.timezone_utc_offset", &["null".to_string()])
            .is_ok()
    );
    assert_eq!(
        cfg.get_key("workspace.timezone_utc_offset"),
        Ok(Value::Null)
    );
    assert!(
        cfg.set_key(
            "workspace.timezone_utc_offset",
            &["Asia/Shanghai".to_string()]
        )
        .is_err()
    );
    cfg.workspace.timezone_utc_offset = Some("+0530".to_string());
    assert!(cfg.validate().is_err());
}

#[test]
fn invalid_mailbox_config_fails() {
    let mut cfg = MailConfig::default();
    assert!(
        cfg.set_key(
            "actions.pull.by_mailbox_id.junk.import_as",
            &["bogus".to_string()]
        )
        .is_err()
    );
    assert!(
        cfg.set_key(
            "actions.pull.by_mailbox_id.sent.direction",
            &["sideways".to_string()]
        )
        .is_err()
    );
    if let Some(entry) = cfg
        .actions
        .message
        .notification
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
fn invalid_mailbox_resolve_config_fails() {
    let mut cfg = MailConfig::default();
    assert!(
        cfg.set_key("mailboxes.archive.resolve", &[r#"[]"#.to_string()])
            .is_err()
    );
    assert!(
        cfg.set_key(
            "mailboxes.archive.resolve",
            &[r#"[{"mailbox_name":"Archive","special_use":"\\Archive"}]"#.to_string()]
        )
        .is_err()
    );
    assert!(
        cfg.set_key(
            "mailboxes.archive.resolve",
            &[r#"[{"mailbox_name":""}]"#.to_string()]
        )
        .is_err()
    );
    assert!(
        cfg.set_key(
            "mailboxes.archive.resolve",
            &[r#"[{"unknown":"Archive"}]"#.to_string()]
        )
        .is_err()
    );
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
    let valid = |value: &str| validate_password_secret_source("imap.password_secret", Some(value));

    // The secret itself, or where it is.
    assert!(valid("secret").is_ok());
    assert!(valid("env:AFMAIL_IMAP_PASSWORD_SECRET").is_ok());
    assert!(valid("file:/etc/afmail/creds.json#imap.password_secret").is_ok());
    // The escape hatch, for a password that starts with a scheme prefix. This
    // used to be a hard error telling the caller to use a second key.
    assert!(valid("literal:env:not-a-source").is_ok());

    // afmail's own rule survives the move: a variable holding a credential
    // says so in its name.
    assert!(valid("env:IMAP_PASSWORD").is_err());
    assert!(valid("env:lower_secret").is_err());

    // A malformed source is a config error, named as one.
    let error = valid("file:/etc/afmail/creds.json").expect_err("no dot path");
    assert_eq!(error.error_code, "config_invalid");
    assert!(error.message.contains("imap.password_secret"), "{error:?}");

    // A stream source has nothing to read from in a config file.
    for raw in ["stdin", "prompt", "fd:3"] {
        assert!(valid(raw).is_err(), "{raw}");
    }
}

#[test]
fn identity_config_validation() {
    let mut cfg = MailConfig::default();
    cfg.identities.clear();
    assert!(cfg.validate().is_err());

    let mut cfg = MailConfig::default();
    cfg.identities[0].default = false;
    assert!(cfg.validate().is_err());

    let mut cfg = MailConfig::default();
    cfg.identities.push(IdentityConfig {
        identity: "other".to_string(),
        name: "Other".to_string(),
        email: "other@example.com".to_string(),
        default: true,
    });
    assert!(cfg.validate().is_err());

    let mut cfg = MailConfig::default();
    cfg.identities.push(IdentityConfig {
        identity: "me".to_string(),
        name: "Other".to_string(),
        email: "other@example.com".to_string(),
        default: false,
    });
    assert!(cfg.validate().is_err());

    let mut cfg = MailConfig::default();
    cfg.identities.push(IdentityConfig {
        identity: "alias".to_string(),
        name: "Alias".to_string(),
        email: "me@example.com".to_string(),
        default: false,
    });
    assert!(cfg.validate().is_ok());

    let mut cfg = MailConfig::default();
    cfg.identities.push(IdentityConfig {
        identity: "身份一".to_string(),
        name: "Unicode Identity".to_string(),
        email: "unicode@example.com".to_string(),
        default: false,
    });
    assert!(cfg.validate().is_ok());

    let mut cfg = MailConfig::default();
    cfg.identities[0].identity = "bad/path".to_string();
    assert!(cfg.validate().is_err());

    let mut cfg = MailConfig::default();
    cfg.identities[0].identity = "support@flfgsc.cn".to_string();
    assert!(cfg.validate().is_err());

    let mut cfg = MailConfig::default();
    cfg.identities[0].email = "not an email".to_string();
    assert!(cfg.validate().is_err());
}

#[test]
fn config_add_and_remove_keyed_identity() {
    let mut cfg = MailConfig::default();

    // Append a second, non-default identity to the `identities` keyed list. The
    // slug becomes its `identity`; `default` falls back to false.
    cfg.add_key(
        "identities",
        "support",
        &[
            ("name".to_string(), "Support".to_string()),
            ("email".to_string(), "support@example.com".to_string()),
        ],
    )
    .expect("add identity");
    assert_eq!(cfg.identities.len(), 2);
    assert_eq!(
        cfg.get_key("identities.support.email"),
        Ok(json!("support@example.com"))
    );
    assert!(
        cfg.identities
            .iter()
            .any(|i| i.identity == "support" && !i.default)
    );

    cfg.add_key(
        "identities",
        "法律法规速查小客服",
        &[
            ("name".to_string(), "法律法规速查小客服".to_string()),
            ("email".to_string(), "support@flfgsc.cn".to_string()),
        ],
    )
    .expect("add unicode identity");
    assert_eq!(
        cfg.get_key("identities.法律法规速查小客服.email"),
        Ok(json!("support@flfgsc.cn"))
    );

    assert!(
        cfg.add_key(
            "identities",
            "support@flfgsc.cn",
            &[
                ("name".to_string(), "Dot Identity".to_string()),
                ("email".to_string(), "dot@example.com".to_string()),
            ],
        )
        .is_err()
    );

    // Removing by slug shrinks the list; removing a missing slug is a no-op so
    // setup scripts can be idempotent after a clean init.
    cfg.remove_key("identities", "support")
        .expect("remove identity");
    cfg.remove_key("identities", "法律法规速查小客服")
        .expect("remove unicode identity");
    assert_eq!(cfg.identities.len(), 1);
    cfg.remove_key("identities", "missing")
        .expect("remove missing identity is no-op");
    assert_eq!(cfg.identities.len(), 1);

    // Adding a new default identity makes it the only default, so setup scripts
    // can replace the initial identity in one config add.
    let mut cfg = MailConfig::default();
    cfg.add_key(
        "identities",
        "support",
        &[
            ("name".to_string(), "Support".to_string()),
            ("email".to_string(), "support@example.com".to_string()),
            ("default".to_string(), "true".to_string()),
        ],
    )
    .expect("add replacement default identity");
    assert_eq!(cfg.identities.iter().filter(|i| i.default).count(), 1);
    assert!(
        cfg.identities
            .iter()
            .any(|i| i.identity == "me" && !i.default)
    );
    assert!(
        cfg.identities
            .iter()
            .any(|i| i.identity == "support" && i.default)
    );
}
