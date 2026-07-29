use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, temp_root, update_message_json, write_message, write_triage};
use serde_json::{Value, json};

fn create_case_with_message(
    root: &Path,
    name: &str,
    message_id: &str,
    reason: &str,
) -> (String, PathBuf) {
    let (status, stdout) = run(
        root,
        &[
            "case",
            "create",
            "--name",
            name,
            "--group",
            "support",
            "--message",
            message_id,
            "--reason",
            reason,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let case_uid = value["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty(), "{value}");
    let case_path = value["case_path"].as_str().unwrap_or_default();
    assert!(!case_path.is_empty(), "{value}");
    (case_uid, root.join(case_path))
}

fn create_notification(
    root: &Path,
    name: &str,
    message_id: &str,
    summary: &str,
    reason: &str,
) -> (String, PathBuf, Value) {
    let (status, stdout) = run(
        root,
        &[
            "notification",
            "create",
            "--name",
            name,
            "--message",
            message_id,
            "--summary",
            summary,
            "--reason",
            reason,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let notification_uid = value["notification_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!notification_uid.is_empty(), "{value}");
    let notification_path = value["path"].as_str().unwrap_or_default();
    assert!(!notification_path.is_empty(), "{value}");
    (notification_uid, root.join(notification_path), value)
}

#[test]
fn message_list_filters_by_sender_subject_since_and_scope() {
    let root = temp_root("message-list");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    write_message(&root, "message_list_triage", 71, None);
    write_triage(&root, "message_list_triage", "triage search note");
    update_message_json(&root, "message_list_triage", |value| {
        value["from"] = json!("Alice Example <alice@example.com>");
        value["sender"] = json!("Alice Example <alice@example.com>");
        value["subject"] = json!("Invoice overdue");
        value["received_rfc3339"] = json!("2026-05-21T09:00:00Z");
    });

    write_message(&root, "message_list_case", 72, None);
    let (case_uid, _case_path) = create_case_with_message(
        &root,
        "message-list-case",
        "message_list_case",
        "case search note",
    );
    update_message_json(&root, "message_list_case", |value| {
        value["from"] = json!("Bob Example <bob@example.com>");
        value["sender"] = json!("Bob Example <bob@example.com>");
        value["subject"] = json!("Invoice follow-up");
        value["received_rfc3339"] = json!("2026-05-22T09:00:00Z");
    });
    assert!(
        root.join(format!(
            "cases/support/{case_uid}-message-list-case/case.md"
        ))
        .is_file()
    );

    write_message(&root, "message_list_notification", 73, None);
    let (notification_uid, _notification_path, _) = create_notification(
        &root,
        "message-list-notification",
        "message_list_notification",
        "receipt note",
        "notification search note",
    );
    update_message_json(&root, "message_list_notification", |value| {
        value["from"] = json!("Carol Example <carol@example.com>");
        value["sender"] = json!("Carol Example <carol@example.com>");
        value["subject"] = json!("Receipt shipped");
        value["received_rfc3339"] = json!("2026-05-23T09:00:00Z");
    });
    assert!(
        root.join(format!(
            "notifications/{notification_uid}-message-list-notification/notification.md"
        ))
        .is_file()
    );

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "list",
            "--sender",
            "bob@example.com",
            "--subject-contains",
            "invoice",
            "--since",
            "2026-05-22",
            "--in",
            "case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 1);
    assert_eq!(value["items"][0]["message_id"], "message_list_case");
    assert_eq!(value["items"][0]["workspace_status"], "case");
    assert_eq!(value["items"][0]["case_refs"][0]["case_uid"], case_uid);
    assert!(value["items"][0]["case_refs"][0].get("lifecycle").is_none());

    let (status, stdout) = run(
        &root,
        &["message", "list", "--since", "2026-05-21", "--in", "all"],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 3);
    let items = value["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items[0]["message_id"], "message_list_notification");
    assert_eq!(items[1]["message_id"], "message_list_case");
    assert_eq!(items[2]["message_id"], "message_list_triage");
    assert_eq!(items[0]["notification_uid"], notification_uid);
    assert_eq!(items[1]["case_refs"][0]["case_uid"], case_uid);
    assert!(items[1]["case_refs"][0].get("lifecycle").is_none());

    let _ = fs::remove_dir_all(root);
}
