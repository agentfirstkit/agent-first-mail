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
fn notes_show_and_notification_refresh_preserve_plain_markdown_bytes() {
    let root = temp_root("notes-read-only");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &local_config());

    write_message(&root, "message_case_notes", 52, None);
    write_triage(&root, "message_case_notes", "case notes");
    let (case_uid, case_path) =
        create_case_with_message(&root, "notes-case", "message_case_notes", "case notes");
    let active_notes = "---\nlegacy: kept\n---\n\n# User Notes\n\n---\nnot yaml, just markdown\n";
    assert!(fs::write(case_path.join("notes.md"), active_notes).is_ok());

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        active_notes
    );
    assert_eq!(
        fs::read_to_string(case_path.join("notes.md")).unwrap_or_default(),
        active_notes
    );

    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 0, "{stdout}");
    let archived_path = root.join(format!("archived-cases/support/{case_uid}-notes-case"));
    assert_eq!(
        fs::read_to_string(archived_path.join("notes.md")).unwrap_or_default(),
        active_notes
    );

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        active_notes
    );
    assert_eq!(
        fs::read_to_string(archived_path.join("notes.md")).unwrap_or_default(),
        active_notes
    );

    write_message(&root, "message_notification_notes", 53, None);
    write_triage(&root, "message_notification_notes", "archive notes");
    let (notification_uid, archive_path) = create_notification(
        &root,
        "notes-archive",
        "message_notification_notes",
        "archive summary",
        "archive notes",
    );
    let archive_notes = "---\ncategory: kept\n---\n\n# Archive Notes\n";
    assert!(fs::write(archive_path.join("notes.md"), archive_notes).is_ok());

    let (status, stdout) = run(&root, &["notification", "show", &notification_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        fs::read_to_string(archive_path.join("notes.md")).unwrap_or_default(),
        archive_notes
    );

    let (status, stdout) = run(&root, &["notification", "notes", "show", &notification_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        archive_notes
    );
    assert_eq!(
        fs::read_to_string(archive_path.join("notes.md")).unwrap_or_default(),
        archive_notes
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_notes_show_and_append_error_until_explicit_replace() {
    let root = temp_root("notes-missing");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_missing_notes", 54, None);
    let (case_uid, case_path) =
        create_case_with_message(&root, "missing-notes", "message_missing_notes", "seed");
    let notes_path = case_path.join("notes.md");
    assert!(fs::remove_file(&notes_path).is_ok());

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "notes_missing");
    assert!(
        value["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("notes replace")
    );
    assert!(!notes_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "notes",
            "append",
            &case_uid,
            "--text",
            "must not create",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "notes_missing");
    assert!(!notes_path.exists());

    let (status, stdout) = run(
        &root,
        &["case", "notes", "replace", &case_uid, "--text", "restored"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        fs::read_to_string(&notes_path).unwrap_or_default(),
        "restored\n"
    );

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        "restored\n"
    );
    let _ = fs::remove_dir_all(root);
}
