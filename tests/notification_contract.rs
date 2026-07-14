use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/cli.rs"]
mod cli;

use cli::{
    parse_one, read_json, run, temp_root, update_message_json, write_json, write_message,
    write_triage,
};
use serde_json::{Value, json};

fn local_config() -> Value {
    json!({
        "schema_name": "config",
        "schema_version": 1,
        "workspace": {
            "timezone_utc_offset": "+08:00"
        }
    })
}

fn create_notification(
    root: &Path,
    name: &str,
    message_id: Option<&str>,
    summary: Option<&str>,
    reason: Option<&str>,
) -> (String, PathBuf, Value) {
    let mut args = vec![
        "notification".to_string(),
        "create".to_string(),
        "--name".to_string(),
        name.to_string(),
    ];
    if let Some(message_id) = message_id {
        args.push("--message".to_string());
        args.push(message_id.to_string());
    }
    if let Some(summary) = summary {
        args.push("--summary".to_string());
        args.push(summary.to_string());
    }
    if let Some(reason) = reason {
        args.push("--reason".to_string());
        args.push(reason.to_string());
    }
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let (status, stdout) = run(root, &refs);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let notification_uid = value["notification_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!notification_uid.is_empty(), "{value}");
    let path = value["path"].as_str().unwrap_or_default();
    assert!(!path.is_empty(), "{value}");
    (notification_uid, root.join(path), value)
}

#[test]
fn notification_delete_requires_empty_collection_and_protects_notes() {
    let root = temp_root("notification-delete");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "notice");

    let (notification_uid, notification_path, _value) = create_notification(
        &root,
        "delete-me",
        Some("message_notice"),
        Some("notice"),
        Some("reference notice"),
    );
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "delete",
            &notification_uid,
            "--reason",
            "not needed",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "notification_not_empty");
    assert!(notification_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "remove",
            &notification_uid,
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "delete",
            &notification_uid,
            "--reason",
            "empty collection",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["notification_uid"], notification_uid);
    assert_eq!(value["notes_deleted"], true);
    assert_eq!(value["notes_modified"], false);
    assert!(!notification_path.exists());
    assert_eq!(
        parse_one(&run(&root, &["notification", "list"]).1)["count"],
        0
    );

    let (notes_uid, notes_path, _value) =
        create_notification(&root, "notes-delete", None, None, Some("notes category"));
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "notes",
            "append",
            &notes_uid,
            "--text",
            "keep this context",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "delete",
            &notes_uid,
            "--reason",
            "delete notes collection",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "notification_notes_present");
    assert!(notes_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "delete",
            &notes_uid,
            "--allow-notes",
            "--reason",
            "delete notes collection",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["notes_modified"], true);
    assert!(!notes_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn notification_index_renders_thread_blocks_with_summary_fallbacks() {
    let root = temp_root("archive-thread-index");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());

    write_message(&root, "message_notification_reply", 96, None);
    update_message_json(&root, "message_notification_reply", |message| {
        message["direction"] = json!("outbound");
        message["subject"] = json!("Original subject");
        message["from"] = json!("Me <me@example.com>");
        message["to"] = json!(["alice@example.com"]);
        message["received_rfc3339"] = Value::Null;
        message["sent_rfc3339"] = json!("2026-05-21T01:00:00Z");
        message["in_reply_to"] = json!("<external@example.com>");
    });
    write_triage(&root, "message_notification_reply", "reply archive");
    let (notification_uid, archive_path, _) = create_notification(
        &root,
        "thread-archive",
        Some("message_notification_reply"),
        Some("Invoice summary"),
        Some("archive reply"),
    );

    write_message(&root, "message_notification_forward", 97, None);
    update_message_json(&root, "message_notification_forward", |message| {
        message["subject"] = json!("转发: Fallback subject");
        message["received_rfc3339"] = json!("2026-05-21T15:00:00+08:00");
    });
    write_triage(&root, "message_notification_forward", "forward archive");
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "add",
            &notification_uid,
            "message_notification_forward",
            "--summary",
            "",
            "--reason",
            "archive forward",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let index = fs::read_to_string(archive_path.join("notification.md")).unwrap_or_default();
    assert!(index.contains("## 1. ← Received forward: 转发: Fallback subject"));
    assert!(index.contains("## 2. → Sent reply: Invoice summary"));
    assert!(!index.contains("→ Sent reply: Original subject"));
    assert!(index.contains("- From: alice@example.com"));
    assert!(index.contains("- To: alice@example.com"));
    assert!(index.contains(
        "- Message: [message_notification_forward](views/messages/message_notification_forward.md)"
    ));
    assert!(index.contains(
        "- Message: [message_notification_reply](views/messages/message_notification_reply.md)"
    ));
    assert!(index.contains("- Time: 2026-05-21T15:00:00+08:00"));
    assert!(index.contains("- Time: 2026-05-21T09:00:00+08:00"));
    assert!(!index.contains(" — "));
    assert!(!index.contains("2026-05-21 15:00"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn notification_move_remove_and_log_filters_work() {
    let root = temp_root("notification-remove");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "invoice notice");

    let (billing_uid, billing_path, _value) = create_notification(
        &root,
        "billing",
        Some("message_notice"),
        Some("invoice notice"),
        Some("invoice notice"),
    );
    let (receipts_uid, receipts_path, _value) =
        create_notification(&root, "receipts", None, None, Some("receipts category"));
    assert!(
        billing_path
            .join("views/messages/message_notice.md")
            .exists()
    );

    let (status, stdout) = run(&root, &["notification", "show", &billing_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["message_count"], 1);

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "move",
            &billing_uid,
            "message_notice",
            &receipts_uid,
            "--reason",
            "receipts owns it",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["notification_uid"], receipts_uid);
    assert!(
        !billing_path
            .join("views/messages/message_notice.md")
            .exists()
    );
    assert!(
        receipts_path
            .join("views/messages/message_notice.md")
            .exists()
    );

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "remove",
            &billing_uid,
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "notification_entry_not_found");

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "set-summary",
            &receipts_uid,
            "message_notice",
            "--summary",
            "receipt summary",
            "--reason",
            "better summary",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["summary"], "receipt summary");

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "remove",
            &receipts_uid,
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    assert!(root.join("triage/message_notice.md").exists());
    assert!(
        !receipts_path
            .join("views/messages/message_notice.md")
            .exists()
    );

    let message = read_json(&root.join("messages/message_notice.json"));
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"]["notification_uid"].is_null());
    assert!(message["workspace"].get("buckets").is_none());

    let (status, stdout) = run(&root, &["log", "message", "message_notice"]);
    assert_eq!(status, 0, "{stdout}");
    let log = parse_one(&stdout);
    assert!(log["count"].as_u64().unwrap_or(0) >= 3);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn notification_add_accepts_multiple_message_ids() {
    let root = temp_root("notification-batch-add");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    write_message(&root, "message_note_a", 61, None);
    write_triage(&root, "message_note_a", "notification batch note");
    write_message(&root, "message_note_b", 62, None);
    write_triage(&root, "message_note_b", "notification batch note");
    let (notification_uid, notification_path, _) = create_notification(
        &root,
        "batch-notify",
        None,
        None,
        Some("batch add notification"),
    );
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "add",
            &notification_uid,
            "message_note_a",
            "message_note_b",
            "--summary",
            "batch summary",
            "--reason",
            "same notification group",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value.as_array().map(Vec::len), Some(2));
    assert!(value[0].get("code").is_none());
    assert_eq!(value[1]["message_id"], "message_note_b");
    let notification_data = read_json(&notification_path.join("data/notification.json"));
    assert_eq!(notification_data["items"].as_array().map(Vec::len), Some(2));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn notification_show_propagates_message_cache_and_evidence_errors_without_mutating_membership() {
    let root = temp_root("notification-show-evidence-error");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_notification_error", 64, None);
    write_triage(&root, "message_notification_error", "notification error");
    let (notification_uid, notification_path, _) = create_notification(
        &root,
        "evidence-error",
        Some("message_notification_error"),
        Some("preserve canonical membership"),
        Some("notification error"),
    );
    let data_path = notification_path.join("data/notification.json");
    let before = fs::read(&data_path).unwrap_or_default();
    let cache_path = root.join("messages/message_notification_error.json");
    let cache_before = fs::read(&cache_path).unwrap_or_default();
    assert!(fs::write(&cache_path, b"{invalid message cache").is_ok());

    let (status, stdout) = run(&root, &["notification", "show", &notification_uid]);
    assert_ne!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "store_error");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|error| error.contains("messages/message_notification_error.json"))
    );
    assert_eq!(fs::read(&data_path).unwrap_or_default(), before);

    assert!(fs::write(&cache_path, cache_before).is_ok());
    let evidence_path = root.join(".afmail/messages/message_notification_error.remote.json");
    assert!(fs::write(&evidence_path, b"{invalid remote evidence").is_ok());

    let (status, stdout) = run(&root, &["notification", "show", &notification_uid]);
    assert_ne!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "store_error");
    assert!(value["message"].as_str().is_some_and(|error| {
        error.contains(".afmail/messages/message_notification_error.remote.json")
    }));
    assert_eq!(fs::read(&data_path).unwrap_or_default(), before);
    let _ = fs::remove_dir_all(root);
}
