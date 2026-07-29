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

fn create_case(
    root: &Path,
    name: &str,
    group: &str,
    message_id: Option<&str>,
    reason: &str,
) -> (String, PathBuf) {
    let mut args = vec![
        "case".to_string(),
        "create".to_string(),
        "--name".to_string(),
        name.to_string(),
        "--group".to_string(),
        group.to_string(),
    ];
    if let Some(message_id) = message_id {
        args.push("--message".to_string());
        args.push(message_id.to_string());
    }
    args.push("--reason".to_string());
    args.push(reason.to_string());
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let (status, stdout) = run(root, &refs);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let case_uid = value["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty(), "{value}");
    let case_path = value["case_path"].as_str().unwrap_or_default();
    assert!(!case_path.is_empty(), "{value}");
    (case_uid, root.join(case_path))
}

#[test]
fn case_archive_validates_unrelated_drafts_before_local_writes() {
    let root = temp_root("archive-invalid-unrelated-draft");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());
    write_message(&root, "message_archive_preflight", 41, None);
    write_triage(
        &root,
        "message_archive_preflight",
        "archive preflight message",
    );

    let (case_uid, case_path) = create_case(
        &root,
        "archive-preflight",
        "support",
        Some("message_archive_preflight"),
        "ready to archive",
    );
    let (other_case_uid, other_case_path) =
        create_case(&root, "old-invalid-draft", "support", None, "old case");
    assert!(
        fs::write(
            other_case_path.join("drafts/invalid.md"),
            format!(
                "---\nkind: draft\ncase_uid: {other_case_uid}\nto: [broken@example.com\n---\n\nOld draft\n"
            ),
        )
        .is_ok()
    );

    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);

    assert_eq!(status, 1, "{stdout}");
    assert!(
        case_path.is_dir(),
        "archive failure must leave the case active"
    );
    let archived_path = root
        .join("archived-cases/support")
        .join(case_path.file_name().unwrap_or_default());
    assert!(
        !archived_path.exists(),
        "archive failure must not move the case"
    );
    let transaction_count = fs::read_dir(root.join(".afmail/transactions"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                })
                .count()
        })
        .unwrap_or_default();
    assert_eq!(
        transaction_count, 0,
        "archive preflight must not leave a sentinel"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_remove_and_delete_empty_active_case() {
    let root = temp_root("case-remove-delete");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_case_remove", 42, None);
    write_triage(&root, "message_case_remove", "case message");

    let (case_uid, case_path) = create_case(
        &root,
        "remove-delete",
        "support",
        Some("message_case_remove"),
        "belongs here for now",
    );

    let (status, stdout) = run(
        &root,
        &["case", "delete", &case_uid, "--reason", "delete empty case"],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_not_empty");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "remove",
            &case_uid,
            "message_case_remove",
            "--reason",
            "does not belong here",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["message_count"], 0);
    assert!(root.join("triage/message_case_remove.md").exists());
    let case_json = read_json(&case_path.join("data/case.json"));
    assert!(case_json.get("message_count").is_none());
    assert_eq!(
        case_json
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        0
    );
    assert!(case_json.get("status").is_none());

    let (status, stdout) = run(
        &root,
        &["case", "delete", &case_uid, "--reason", "empty case"],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    assert!(!case_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_group_dirs_are_cleaned_only_when_empty() {
    let root = temp_root("case-group-cleanup");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());
    write_message(&root, "message_alpha", 45, None);
    write_message(&root, "message_beta", 46, None);
    write_triage(&root, "message_alpha", "alpha");
    write_triage(&root, "message_beta", "beta");

    let (case_alpha_uid, _case_alpha_path) =
        create_case(&root, "case-alpha", "tests", Some("message_alpha"), "alpha");
    let (case_beta_uid, _case_beta_path) =
        create_case(&root, "case-beta", "tests", Some("message_beta"), "beta");

    assert_eq!(
        run(
            &root,
            &["case", "archive", &case_alpha_uid, "--reason", "done"]
        )
        .0,
        0
    );
    assert!(root.join("cases/tests").exists());
    assert!(
        root.join(format!("cases/tests/{case_beta_uid}-case-beta/case.md"))
            .exists()
    );

    assert_eq!(
        run(
            &root,
            &[
                "case",
                "move",
                &case_beta_uid,
                "--group",
                "waiting",
                "--reason",
                "better group",
            ],
        )
        .0,
        0
    );
    assert!(!root.join("cases/tests").exists());
    assert!(
        root.join(format!("cases/waiting/{case_beta_uid}-case-beta/case.md"))
            .exists()
    );

    assert!(fs::write(root.join("cases/waiting/keep.txt"), "user file").is_ok());
    assert_eq!(
        run(
            &root,
            &["case", "archive", &case_beta_uid, "--reason", "done"]
        )
        .0,
        0
    );
    assert!(root.join("cases/waiting/keep.txt").exists());
    assert!(
        root.join(format!(
            "archived-cases/waiting/{case_beta_uid}-case-beta/case.md"
        ))
        .exists()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_add_accepts_multiple_message_ids() {
    let root = temp_root("batch-add");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());

    write_message(&root, "message_case_a", 51, None);
    write_triage(&root, "message_case_a", "case batch note");
    write_message(&root, "message_case_b", 52, None);
    write_triage(&root, "message_case_b", "case batch note");
    let (case_uid, case_path) =
        create_case(&root, "batch-add-case", "support", None, "batch add case");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_case_a",
            "message_case_b",
            "--reason",
            "same case thread",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value.as_array().map(Vec::len), Some(2));
    assert!(value[0].get("code").is_none());
    assert_eq!(value[1]["message_id"], "message_case_b");
    let case_data = read_json(&case_path.join("data/case.json"));
    assert_eq!(case_data["schema_version"], 3);
    assert_eq!(case_data["items"].as_array().map(Vec::len), Some(2));
    for derived in [
        "message_count",
        "thread_count",
        "attachment_count",
        "last_message_rfc3339",
    ] {
        assert!(case_data.get(derived).is_none(), "{derived}: {case_data}");
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn archived_case_move_preserves_canonical_status() {
    let root = temp_root("archived-case-move");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());
    write_message(&root, "message_archived_move", 50, None);
    write_triage(&root, "message_archived_move", "archive move");
    let (case_uid, case_path) = create_case(
        &root,
        "case-archived-move",
        "support",
        Some("message_archived_move"),
        "archive move",
    );

    let (status, stdout) = run(
        &root,
        &["case", "archive", &case_uid, "--reason", "complete"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert!(!case_path.exists());
    let archived_path = root.join(format!(
        "archived-cases/support/{case_uid}-case-archived-move"
    ));
    assert!(archived_path.join("case.md").exists());
    let archived_before = read_json(&archived_path.join("data/case.json"));
    assert!(archived_before["archived_rfc3339"].as_str().is_some());
    assert_eq!(archived_before["status"], "archived");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "move",
            &case_uid,
            "--group",
            "members",
            "--reason",
            "调整事项分组",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["case_uid"], case_uid);
    assert_eq!(value["from_group"], "support");
    assert_eq!(value["to_group"], "members");
    assert_eq!(value["status"], "archived");
    assert_eq!(value["changed"], true);
    assert_eq!(
        value["case_path"],
        format!("archived-cases/members/{case_uid}-case-archived-move")
    );
    assert!(!archived_path.exists());
    assert!(!root.join("archived-cases/support").exists());
    let moved_path = root.join(format!(
        "archived-cases/members/{case_uid}-case-archived-move"
    ));
    assert!(moved_path.join("case.md").exists());
    let archived_after = read_json(&moved_path.join("data/case.json"));
    assert!(archived_after["archived_rfc3339"].as_str().is_some());
    assert_eq!(archived_after["status"], "archived");

    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["context"]["location"]["group"], "members");
    assert!(value["context"]["location"].get("archived").is_none());
    assert!(value["context"]["location"].get("lifecycle").is_none());
    assert_eq!(value["context"]["collection"]["status"], "archived");

    let (status, stdout) = run(&root, &["case", "list", "--archived"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 1);
    assert_eq!(value["items"][0]["group"], "members");
    assert_eq!(value["items"][0]["status"], "archived");
    assert!(value["items"][0].get("archived").is_none());
    assert!(value["items"][0].get("lifecycle").is_none());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "move",
            &case_uid,
            "--group",
            "members",
            "--reason",
            "分组无需变更",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["changed"], false);
    assert_eq!(
        value["case_path"],
        format!("archived-cases/members/{case_uid}-case-archived-move")
    );
    assert!(moved_path.join("case.md").exists());

    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"case_moved\""));
    assert!(log.contains("\"reason\":\"调整事项分组\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archived_case_notes_rename_unarchive_and_active_hint_work() {
    let root = temp_root("archive-case-restore");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());
    write_message(&root, "message_case", 42, None);
    write_triage(&root, "message_case", "case work");
    let (case_uid, case_path) = create_case(
        &root,
        "case-archive",
        "support",
        Some("message_case"),
        "case work",
    );

    assert!(fs::write(case_path.join("case.md"), "stale generated case").is_ok());
    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["context"]["collection"]["collection_uid"], case_uid);
    assert_eq!(
        value["context"]["collection"]["collection_name"],
        "case-archive"
    );
    assert_eq!(value["context"]["location"]["group"], "support");
    assert_eq!(
        root.join(
            value["locator"]["collection_path"]
                .as_str()
                .unwrap_or_default()
        ),
        case_path
    );
    assert_eq!(
        root.join(value["locator"]["view_path"].as_str().unwrap_or_default()),
        case_path.join("case.md")
    );
    assert!(value.get("index_path").is_none());
    assert_eq!(
        root.join(
            value["locator"]["messages_path"]
                .as_str()
                .unwrap_or_default()
        ),
        case_path.join("views/messages")
    );
    assert!(value.get("text").is_none());
    assert_eq!(
        fs::read_to_string(case_path.join("case.md")).unwrap_or_default(),
        "stale generated case"
    );

    let (status, stdout) = run(
        &root,
        &["case", "archive", &case_uid, "--reason", "completed"],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    assert!(!case_path.exists());
    let archived_path = root.join(format!("archived-cases/support/{case_uid}-case-archive"));
    assert!(archived_path.exists());

    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["context"]["collection"]["collection_uid"], case_uid);
    assert_eq!(
        value["context"]["collection"]["collection_name"],
        "case-archive"
    );
    assert!(value["context"]["location"].get("archived").is_none());
    assert!(value["context"]["location"].get("lifecycle").is_none());
    assert_eq!(value["context"]["collection"]["status"], "archived");
    assert!(
        fs::read_to_string(archived_path.join("case.md"))
            .unwrap_or_default()
            .contains("# case-archive")
    );

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_case",
            "--reason",
            "active edit",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["items"][0]["error"]["archived_group"], "support");
    assert_eq!(
        value["items"][0]["error"]["suggested_command"],
        format!("afmail case unarchive {case_uid} --group support --reason TEXT")
    );

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "notes",
            "append",
            &case_uid,
            "--text",
            "archive note",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let notes = fs::read_to_string(archived_path.join("notes.md")).unwrap_or_default();
    assert!(notes.contains("archive note"));

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "rename",
            &case_uid,
            "--name",
            "case-renamed",
            "--reason",
            "better id",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["case_uid"], case_uid);
    assert!(!archived_path.exists());
    let renamed_archive_path = root.join(format!("archived-cases/support/{case_uid}-case-renamed"));
    assert!(renamed_archive_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "unarchive",
            &case_uid,
            "--group",
            "support",
            "--reason",
            "needs attention",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    let restored_path = root.join(format!("cases/support/{case_uid}-case-renamed"));
    assert!(restored_path.join("case.md").exists());
    assert!(!renamed_archive_path.exists());

    let case_data = read_json(&restored_path.join("data/case.json"));
    assert_eq!(case_data["collection_uid"], case_uid);
    assert_eq!(case_data["collection_name"], "case-renamed");
    assert!(case_data.get("buckets").is_none());
    assert!(case_data.get("primary_bucket").is_none());
    assert!(case_data["archived_rfc3339"].is_null());
    let (status, stdout) = run(&root, &["log", "case", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert!(parse_one(&stdout)["count"].as_u64().unwrap_or(0) >= 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn active_case_generates_localized_message_index_and_views() {
    let root = temp_root("case-messages");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = local_config();
    config["workspace"]["language_bcp47"] = json!("zh-CN");
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_case1", 81, None);
    write_triage(&root, "message_case1", "seed");
    let (case_uid, case_path) = create_case(
        &root,
        "case-messages",
        "support",
        Some("message_case1"),
        "seed",
    );

    let index = fs::read_to_string(case_path.join("case.md")).unwrap_or_default();
    assert!(!index.starts_with("---\n"));
    assert!(!index.contains("kind: case_index"));
    assert!(index.starts_with("# case-messages\n"));
    assert!(index.contains(&format!("事项: {case_uid} · 消息: 1")));
    assert!(index.contains("## 正文"));
    assert!(index.contains("### 收到自 alice@example.com"));
    assert!(index.contains("消息: [`message_case1`](views/messages/message_case1.md)"));
    assert!(index.contains("发件人: alice@example.com"));
    assert!(index.contains("附件:"));
    assert!(!index.contains("- 状态: case"));
    assert!(!case_path.join("views/messages/index.md").exists());
    let view =
        fs::read_to_string(case_path.join("views/messages/message_case1.md")).unwrap_or_default();
    assert!(view.contains("kind: case_message"));
    assert!(view.contains(&format!("事项: case-messages（`{case_uid}`）")));
    assert!(view.contains("Body"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_index_renders_thread_blocks_with_local_times_and_actions() {
    let root = temp_root("case-thread-index");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = local_config();
    config["workspace"]["timezone_utc_offset"] = json!("+08:00");
    write_json(&root.join(".afmail/config.json"), &config);

    write_message(&root, "message_sent_reply", 91, None);
    update_message_json(&root, "message_sent_reply", |message| {
        message["direction"] = json!("outbound");
        message["subject"] = json!("Follow up");
        message["from"] = json!("Me <me@example.com>");
        message["to"] = json!(["alice@example.com"]);
        message["received_rfc3339"] = Value::Null;
        message["sent_rfc3339"] = json!("2026-05-21T02:00:00Z");
        message["in_reply_to"] = json!("<message_received@example.com>");
    });
    write_triage(&root, "message_sent_reply", "reply");
    let (case_uid, case_path) = create_case(
        &root,
        "thread-case",
        "support",
        Some("message_sent_reply"),
        "seed",
    );

    for (message_id, uid, subject, time) in [
        (
            "message_received",
            92,
            "Initial request",
            "2026-05-21T01:00:00Z",
        ),
        (
            "message_re_subject",
            93,
            "Re: Initial request",
            "2026-05-21T03:00:00Z",
        ),
        ("message_fwd", 94, "Fwd: FYI", "2026-05-21T12:00:00+08:00"),
        ("message_fw_sent", 95, "Fw: FYI", "2026-05-21T05:00:00Z"),
    ] {
        write_message(&root, message_id, uid, None);
        update_message_json(&root, message_id, |message| {
            message["subject"] = json!(subject);
            message["received_rfc3339"] = json!(time);
            if message_id == "message_fw_sent" {
                message["direction"] = json!("outbound");
                message["from"] = json!("Me <me@example.com>");
                message["to"] = json!(["alice@example.com"]);
                message["received_rfc3339"] = Value::Null;
                message["sent_rfc3339"] = json!(time);
            }
        });
        write_triage(&root, message_id, "thread");
        let (status, stdout) = run(
            &root,
            &[
                "case",
                "add",
                &case_uid,
                message_id,
                "--reason",
                "belongs to the thread",
            ],
        );
        assert_eq!(status, 0, "{stdout}");
    }

    let index = fs::read_to_string(case_path.join("case.md")).unwrap_or_default();
    let received = index.find("### Received from alice@example.com - 2026-05-21T09:00:00+08:00");
    let sent_reply = index.find("### Sent to alice@example.com - 2026-05-21T10:00:00+08:00");
    let re_subject = index.find("### Received from alice@example.com - 2026-05-21T11:00:00+08:00");
    let fwd = index.find("### Received from alice@example.com - 2026-05-21T12:00:00+08:00");
    let fw_sent = index.find("### Sent to alice@example.com - 2026-05-21T13:00:00+08:00");
    assert!(received.is_some(), "{index}");
    assert!(sent_reply.is_some(), "{index}");
    assert!(re_subject.is_some(), "{index}");
    assert!(fwd.is_some(), "{index}");
    assert!(fw_sent.is_some(), "{index}");
    assert!(received < sent_reply && sent_reply < re_subject && re_subject < fwd && fwd < fw_sent);
    assert!(index.contains("From: alice@example.com"));
    assert!(index.contains("To: alice@example.com"));
    assert!(index.contains("Message: [`message_received`](views/messages/message_received.md)"));
    assert!(!index.contains("2026-05-21T01:00:00Z"));
    assert!(!index.contains("- Status: case"));
    assert!(index.contains("## Conversation"));
    let _ = fs::remove_dir_all(root);
}
