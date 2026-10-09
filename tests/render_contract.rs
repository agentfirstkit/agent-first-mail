use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, temp_root, write_json, write_message, write_triage};
use serde_json::{Value, json};

fn local_config() -> Value {
    json!({
        "schema_name": "config",
        "schema_version": 1,
        "workspace": {"timezone_utc_offset": "UTC"},
        "actions": {
            "case": {"add": {"steps": []}},
            "message": {
                "notification": {
                    "by_source_mailbox_id": {
                        "inbox": {"steps": []}
                    }
                }
            }
        }
    })
}

fn create_case(root: &Path, name: &str, message_id: &str, reason: &str) -> (String, PathBuf) {
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
) -> (String, PathBuf) {
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
    let path = value["path"].as_str().unwrap_or_default();
    assert!(!path.is_empty(), "{value}");
    (notification_uid, root.join(path))
}

#[test]
fn render_rejects_inconsistent_disposition_status_and_duplicate_membership() {
    let root = temp_root("render-invalid-disposition-state");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert!(fs::create_dir_all(root.join("spam/data")).is_ok());
    assert!(fs::create_dir_all(root.join("trash/data")).is_ok());

    let item = json!({
        "message_id": "message_disposition_conflict",
        "added_rfc3339": "2026-07-22T00:00:00Z"
    });
    let mut spam = json!({
        "schema_name": "spam",
        "schema_version": 2,
        "collection_uid": "spam",
        "collection_name": "spam",
        "status": "trashed",
        "created_rfc3339": "2026-07-22T00:00:00Z",
        "updated_rfc3339": "2026-07-22T00:00:00Z",
        "items": [item.clone()]
    });
    write_json(&root.join("spam/data/spam.json"), &spam);

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_ne!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "disposition_messages_invalid");

    spam["status"] = json!("spam");
    write_json(&root.join("spam/data/spam.json"), &spam);
    write_json(
        &root.join("trash/data/trash.json"),
        &json!({
            "schema_name": "trash",
            "schema_version": 2,
            "collection_uid": "trash",
            "collection_name": "trash",
            "status": "trashed",
            "created_rfc3339": "2026-07-22T00:00:00Z",
            "updated_rfc3339": "2026-07-22T00:00:00Z",
            "items": [item]
        }),
    );

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_ne!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "disposition_state_invalid");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_refresh_rebuilds_generated_views_and_removes_obsolete_case_indexes() {
    let root = temp_root("render-refresh");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());

    write_message(&root, "message_active", 81, None);
    write_triage(&root, "message_active", "active");
    let (active_uid, active_path) = create_case(&root, "active-case", "message_active", "active");

    write_message(&root, "message_done", 82, None);
    write_triage(&root, "message_done", "done");
    let (done_uid, _done_path) = create_case(&root, "done-case", "message_done", "done");
    assert_eq!(
        run(&root, &["case", "archive", &done_uid, "--reason", "done"],).0,
        0
    );
    let archived_done_path = root.join(format!("archived-cases/support/{done_uid}-done-case"));

    write_message(&root, "message_notice", 83, None);
    write_triage(&root, "message_notice", "notice");
    let (_billing_uid, billing_path) = create_notification(
        &root,
        "billing",
        "message_notice",
        "invoice notice",
        "notice",
    );

    assert!(fs::write(active_path.join("views/messages/index.md"), "legacy active").is_ok());
    assert!(
        fs::write(
            archived_done_path.join("views/messages/index.md"),
            "legacy archived"
        )
        .is_ok()
    );
    assert!(fs::remove_file(active_path.join("case.md")).is_ok());
    assert!(fs::remove_file(active_path.join("views/messages/message_active.md")).is_ok());
    assert!(fs::remove_file(archived_done_path.join("case.md")).is_ok());
    assert!(fs::remove_file(archived_done_path.join("views/messages/message_done.md")).is_ok());
    assert!(fs::remove_file(billing_path.join("notification.md")).is_ok());
    assert!(fs::remove_file(billing_path.join("views/messages/message_notice.md")).is_ok());
    let before_push = parse_one(&run(&root, &["push", "list"]).1)["count"].clone();

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["generated"]["case/case.md.j2"], 2);
    assert_eq!(value["generated"]["case/message.md.j2"], 2);
    assert_eq!(value["generated"]["notification/notification.md.j2"], 1);
    assert_eq!(value["generated"]["notification/message.md.j2"], 1);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["builtin"], 2);
    assert_eq!(
        parse_one(&run(&root, &["push", "list"]).1)["count"],
        before_push
    );

    let active_index = fs::read_to_string(active_path.join("case.md")).unwrap_or_default();
    assert!(!active_index.starts_with("---\n"));
    assert!(!active_index.contains("kind: case_index"));
    assert!(active_index.starts_with("# active-case\n"));
    assert!(active_index.contains(&format!("Case: {active_uid} · Messages: 1")));
    assert!(active_index.contains("## Conversation"));
    assert!(active_index.contains("### Received from alice@example.com"));
    assert!(active_index.contains("Message: [`message_active`](views/messages/message_active.md)"));
    assert!(active_index.contains("From: alice@example.com"));
    assert!(active_index.contains("Attachments:"));
    assert!(!active_index.contains("- Status: case"));
    assert!(!active_index.contains("| Message |"));
    assert!(
        active_path
            .join("views/messages/message_active.md")
            .exists()
    );
    assert!(archived_done_path.join("case.md").exists());
    assert!(
        archived_done_path
            .join("views/messages/message_done.md")
            .exists()
    );
    assert!(billing_path.join("notification.md").exists());
    assert!(
        billing_path
            .join("views/messages/message_notice.md")
            .exists()
    );
    assert!(!active_path.join("views/messages/index.md").exists());
    assert!(!archived_done_path.join("views/messages/index.md").exists());

    let (warm_status, warm_stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(warm_status, 0, "{warm_stdout}");
    let warm = parse_one(&warm_stdout);
    assert_eq!(warm["generated"]["case/case.md.j2"], 0);
    assert_eq!(warm["generated"]["case/message.md.j2"], 0);
    assert_eq!(
        warm["template_sources"]["message/section.md.j2"]["builtin"]
            .as_u64()
            .unwrap_or(0),
        0
    );

    let canonical_case_view = fs::read_to_string(active_path.join("case.md")).unwrap_or_default();
    assert!(fs::write(active_path.join("case.md"), "tampered generated view\n").is_ok());
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let repaired = parse_one(&stdout);
    assert_eq!(repaired["generated"]["case/case.md.j2"], 1);
    assert_eq!(
        fs::read_to_string(active_path.join("case.md")).unwrap_or_default(),
        canonical_case_view
    );

    let mut config: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".afmail/config.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    config["workspace"]["timezone_utc_offset"] = json!("+08:00");
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let timezone_refresh = parse_one(&stdout);
    assert!(
        timezone_refresh["generated"]["case/case.md.j2"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    let timezone_case_view = fs::read_to_string(active_path.join("case.md")).unwrap_or_default();
    assert_ne!(timezone_case_view, canonical_case_view);
    // 10:00Z shown in the workspace's +08:00 as a readable local time.
    assert!(
        timezone_case_view.contains("2026-05-21 18:00"),
        "{timezone_case_view}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = root.join("outside-template.j2");
        assert!(fs::write(&outside, "outside").is_ok());
        let shared = root.join("templates/shared");
        assert!(fs::create_dir_all(&shared).is_ok());
        assert!(symlink(&outside, shared.join("linked-template.j2")).is_ok());
        let (status, stdout) = run(&root, &["render", "refresh"]);
        assert_eq!(status, 1, "{stdout}");
        assert_eq!(parse_one(&stdout)["code"], "template_path_unsafe");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_cache_hashes_the_complete_typed_template_document() {
    let root = temp_root("render-cache-complete-document");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());

    write_message(&root, "message_case_cache", 91, None);
    write_triage(&root, "message_case_cache", "case cache");
    let (_case_uid, case_path) =
        create_case(&root, "cache-case", "message_case_cache", "case cache");

    write_message(&root, "message_notification_cache", 92, None);
    write_triage(&root, "message_notification_cache", "notification cache");
    let (_notification_uid, notification_path) = create_notification(
        &root,
        "cache-notification",
        "message_notification_cache",
        "notification cache",
        "notification cache",
    );

    assert!(fs::create_dir_all(root.join("templates/en-US/case")).is_ok());
    assert!(fs::create_dir_all(root.join("templates/en-US/notification")).is_ok());
    assert!(
        fs::write(
            root.join("templates/en-US/case/message.md.j2"),
            "case tags={{ document.collection.tags | join(',') }}\n",
        )
        .is_ok()
    );
    assert!(
        fs::write(
            root.join("templates/en-US/notification/notification.md.j2"),
            "notification tags={{ document.collection.tags | join(',') }}\n",
        )
        .is_ok()
    );

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let warm = parse_one(&stdout);
    assert_eq!(warm["generated"]["case/message.md.j2"], 0);
    assert_eq!(warm["generated"]["notification/notification.md.j2"], 0);

    let case_json_path = case_path.join("data/case.json");
    let mut case_data: Value =
        serde_json::from_str(&fs::read_to_string(&case_json_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    case_data["tags"] = json!(["case-tag-from-document"]);
    write_json(&case_json_path, &case_data);

    let notification_json_path = notification_path.join("data/notification.json");
    let mut notification_data: Value =
        serde_json::from_str(&fs::read_to_string(&notification_json_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    notification_data["tags"] = json!(["notification-tag-from-document"]);
    write_json(&notification_json_path, &notification_data);

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let refreshed = parse_one(&stdout);
    assert_eq!(refreshed["generated"]["case/message.md.j2"], 1);
    assert_eq!(refreshed["generated"]["notification/notification.md.j2"], 1);
    assert_eq!(
        fs::read_to_string(case_path.join("views/messages/message_case_cache.md"))
            .unwrap_or_default(),
        "case tags=case-tag-from-document"
    );
    assert_eq!(
        fs::read_to_string(notification_path.join("notification.md")).unwrap_or_default(),
        "notification tags=notification-tag-from-document"
    );

    let _ = fs::remove_dir_all(root);
}
