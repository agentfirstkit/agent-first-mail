use serde_json::json;
use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, temp_root, write_json};

#[test]
fn legacy_config_is_rejected() {
    let root = temp_root("legacy");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    write_json(
        &root.join(".afmail/config.json"),
        &json!({"schema_name": "config", "schema_version": 1, "imap_host": "imap.example.com"}),
    );
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "config_invalid");

    write_json(
        &root.join(".afmail/config.json"),
        &json!({
            "schema_name": "config",
            "schema_version": 1,
            "imap": {"host": null, "port": 993, "tls": true, "username": null, "password_secret": null},
            "pull": {"folders": ["INBOX"]},
            "case": {},
            "special_use": {},
            "smtp": {"host": null, "port": 587, "starttls": true, "tls_wrapper": false, "username": null, "password_secret": null, "from": null},
            "push": {"spam_mark_seen": true}
        }),
    );
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "config_invalid");

    write_json(
        &root.join(".afmail/config.json"),
        &json!({
            "schema_name": "config",
            "schema_version": 1,
            "imap": {"host": null, "port": 993, "tls": true, "username": null, "password_secret": null},
            "pull": {"folders": ["INBOX"]},
            "folders": {"drafts": "Drafts", "sent": "Sent"},
            "smtp": {"host": null, "port": 587, "starttls": true, "tls_wrapper": false, "username": null, "password_secret": null, "from": null},
            "push": {}
        }),
    );
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "config_invalid");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn dependency_lock_excludes_native_tls_and_openssl() {
    let lock = include_str!("../Cargo.lock");
    for name in ["native-tls", "openssl", "openssl-sys"] {
        assert!(
            !lock.contains(&format!("name = \"{name}\"")),
            "forbidden dependency in Cargo.lock: {name}"
        );
    }
}
