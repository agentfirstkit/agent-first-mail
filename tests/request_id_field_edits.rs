use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("afmail-it-{name}-{}-{stamp}", std::process::id()))
}

fn bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_afmail") {
        Ok(value) => PathBuf::from(value),
        Err(_) => PathBuf::from(env!("CARGO_BIN_EXE_afmail")),
    }
}

fn run(cwd: &Path, args: &[&str]) -> (i32, String) {
    let output = Command::new(bin()).current_dir(cwd).args(args).output();
    let Ok(output) = output else {
        return (99, String::new());
    };
    (
        output.status.code().unwrap_or(99),
        String::from_utf8(output.stdout).unwrap_or_default(),
    )
}

fn run_with_env(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> (i32, String) {
    let mut command = Command::new(bin());
    command.current_dir(cwd).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output();
    let Ok(output) = output else {
        return (99, String::new());
    };
    (
        output.status.code().unwrap_or(99),
        String::from_utf8(output.stdout).unwrap_or_default(),
    )
}

fn parse_one(stdout: &str) -> Value {
    let value = stdout
        .lines()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .unwrap_or(Value::Null);
    assert!(agent_first_data::validate_protocol_event(&value, false).is_ok());
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    value.get(kind).cloned().unwrap_or(Value::Null)
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        assert!(fs::create_dir_all(parent).is_ok());
    }
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    assert!(fs::write(path, format!("{text}\n")).is_ok());
}

fn write_message(root: &Path, message_id: &str, uid: u64) {
    let message = json!({
        "schema_name": "message",
        "schema_version": 1,
        "message_id": message_id,
        "rfc822_message_id": format!("<{message_id}@example.com>"),
        "remote": {"locations": [{
            "mailbox_name": "INBOX",
            "mailbox_id": "inbox",
            "uid_validity": 1,
            "uid": uid,
            "flags": [],
            "observed_rfc3339": "2026-05-21T10:00:00Z"
        }]},
        "direction": "inbound",
        "subject": "Contract renewal",
        "from": "alice@example.com",
        "to": ["me@example.com"],
        "cc": [],
        "received_rfc3339": "2026-05-21T10:00:00Z",
        "body_text": "Body",
        "eml_path": format!(".afmail/messages/{message_id}.eml"),
        "attachments": [],
        "workspace": {"status": "triage"}
    });
    write_json(&root.join(format!("messages/{message_id}.json")), &message);
    write_json(
        &root.join(format!(".afmail/messages/{message_id}.remote.json")),
        &json!({
            "schema_name": "message_remote",
            "schema_version": 1,
            "message_id": message_id,
            "locations": message["remote"]["locations"].clone()
        }),
    );
    assert!(fs::write(
        root.join(format!(".afmail/messages/{message_id}.eml")),
        format!(
            "Message-ID: <{message_id}@example.com>\r\nFrom: alice@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Contract renewal\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody\r\n"
        )
    )
    .is_ok());
}

fn write_draft_config(root: &Path) {
    write_json(
        &root.join(".afmail/config.json"),
        &json!({
            "schema_name": "config",
            "schema_version": 1,
            "identities": [
                {"identity": "me", "name": "Me", "email": "me@example.com", "default": true}
            ],
            "audit": {"reason_mode": "required"}
        }),
    );
}

fn create_case(root: &Path, message_id: Option<&str>) -> String {
    let mut args = vec![
        "case",
        "create",
        "--name",
        "Retry Case",
        "--group",
        "support",
        "--reason",
        "seed case",
    ];
    if let Some(message_id) = message_id {
        args.push("--message");
        args.push(message_id);
        args.push("--summary");
        args.push("initial case summary");
    }
    let (status, stdout) = run(root, &args);
    assert_eq!(status, 0, "{stdout}");
    parse_one(&stdout)["case_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn create_contact(root: &Path) -> String {
    let (status, stdout) = run(
        root,
        &[
            "contact",
            "create",
            "--name",
            "Retry Contact",
            "--reason",
            "seed contact",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    parse_one(&stdout)["contact_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn create_notification(root: &Path, name: &str, message_id: Option<&str>) -> String {
    let mut args = vec![
        "notification",
        "create",
        "--name",
        name,
        "--reason",
        "seed notification",
    ];
    if let Some(message_id) = message_id {
        args.push("--message");
        args.push(message_id);
        args.push("--summary");
        args.push("initial notification summary");
    }
    let (status, stdout) = run(root, &args);
    assert_eq!(status, 0, "{stdout}");
    parse_one(&stdout)["notification_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn run_twice(root: &Path, request_id: &str, args: Vec<String>, _code: &str, id_key: &str) {
    let mut command = vec!["--request-id".to_string(), request_id.to_string()];
    command.extend(args);
    let command_refs = command.iter().map(String::as_str).collect::<Vec<_>>();
    let (status, stdout) = run(root, &command_refs);
    assert_eq!(status, 0, "{stdout}");
    let first = parse_one(&stdout);
    let (status, stdout) = run(root, &command_refs);
    assert_eq!(status, 0, "{stdout}");
    let replayed = parse_one(&stdout);
    assert_eq!(replayed[id_key], first[id_key]);
}

fn create_draft(root: &Path, case_uid: &str) -> String {
    let (status, stdout) = run(
        root,
        &[
            "case",
            "draft",
            "new",
            case_uid,
            "--to",
            "customer@example.com",
            "--subject",
            "Retry Draft",
            "--body",
            "Initial body",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn request_id_replays_create_and_notes_without_duplicate_effects() {
    let root = temp_root("request-id-create-notes");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let args = [
        "--request-id",
        "req_case_1",
        "case",
        "create",
        "--name",
        "Refund",
        "--group",
        "support",
        "--reason",
        "seed case",
    ];
    let (status, stdout) = run(&root, &args);
    assert_eq!(status, 0, "{stdout}");
    let created = parse_one(&stdout);
    let case_uid = created["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty());

    let (status, stdout) = run(&root, &args);
    assert_eq!(status, 0, "{stdout}");
    let replayed = parse_one(&stdout);
    assert_eq!(replayed["case_uid"], created["case_uid"]);
    let support_cases = fs::read_dir(root.join("cases/support"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or_default();
    assert_eq!(support_cases, 1);

    let (status, stdout) = run(
        &root,
        &[
            "--request-id",
            "req_case_1",
            "case",
            "create",
            "--name",
            "Different",
            "--group",
            "support",
            "--reason",
            "seed case",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "idempotency_conflict");

    let note_args = [
        "--request-id",
        "req_note_1",
        "case",
        "notes",
        "append",
        &case_uid,
        "--text",
        "private note text",
    ];
    let (status, stdout) = run(&root, &note_args);
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(&root, &note_args);
    assert_eq!(status, 0, "{stdout}");
    let notes_path = fs::read_dir(root.join("cases/support"))
        .ok()
        .and_then(|mut entries| entries.find_map(Result::ok))
        .map(|entry| entry.path().join("notes.md"));
    let notes = notes_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    assert_eq!(notes.matches("private note text").count(), 1);
    let request_record =
        fs::read_to_string(root.join(".afmail/requests/req_note_1.json")).unwrap_or_default();
    assert!(!request_record.contains("private note text"));
    assert!(request_record.contains("\"status\": \"completed\""));

    let replace_args = [
        "--request-id",
        "req_note_replace_1",
        "case",
        "notes",
        "replace",
        &case_uid,
        "--text",
        "replacement private note",
    ];
    let (status, stdout) = run(&root, &replace_args);
    assert_eq!(status, 0, "{stdout}");
    let replaced = parse_one(&stdout);
    let (status, stdout) = run(&root, &replace_args);
    assert_eq!(status, 0, "{stdout}");
    let replayed = parse_one(&stdout);
    assert_eq!(replayed["targets"], replaced["targets"]);
    assert_eq!(replayed["notes_path"], replaced["notes_path"]);
    assert_eq!(replayed["text_len_bytes"], replaced["text_len_bytes"]);
    assert_eq!(replayed["text_hash"], replaced["text_hash"]);
    let notes = notes_path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    assert_eq!(notes.matches("replacement private note").count(), 1);
    assert_eq!(notes.matches("private note text").count(), 0);
    let request_record = fs::read_to_string(root.join(".afmail/requests/req_note_replace_1.json"))
        .unwrap_or_default();
    assert!(!request_record.contains("replacement private note"));
    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert_eq!(audit.matches("\"kind\":\"case_notes_replaced\"").count(), 1);

    let (status, stdout) = run(&root, &["--request-id", "req_status_1", "status"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "request_id_not_supported");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_blocks_retry_after_effect_before_record_completion() {
    let root = temp_root("request-id-failure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let args = [
        "--request-id",
        "req_case_pending",
        "case",
        "create",
        "--name",
        "Pending",
        "--group",
        "support",
        "--reason",
        "seed case",
    ];
    let (status, stdout) = run_with_env(
        &root,
        &args,
        &[(
            "AFMAIL_TEST_FAIL_IDEMPOTENCY_AFTER_OPERATION",
            "req_case_pending",
        )],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["code"],
        "idempotency_after_operation_test_failure"
    );
    let support_cases = fs::read_dir(root.join("cases/support"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or_default();
    assert_eq!(support_cases, 1);

    let (status, stdout) = run(&root, &args);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "idempotency_incomplete");
    let support_cases = fs::read_dir(root.join("cases/support"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or_default();
    assert_eq!(support_cases, 1);

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 1, "{stdout}");
    let issues = parse_one(&stdout)["issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "idempotency_incomplete")
    );
    let record =
        fs::read_to_string(root.join(".afmail/requests/req_case_pending.json")).unwrap_or_default();
    assert!(record.contains("\"status\": \"pending\""));
    assert!(!record.contains("Pending"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_replays_field_edits_without_duplicate_audit() {
    let root = temp_root("request-id-field-edits");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_case_summary", 101);
    write_message(&root, "message_notification_summary", 102);
    let case_uid = create_case(&root, Some("message_case_summary"));
    let contact_uid = create_contact(&root);
    let notification_uid = create_notification(
        &root,
        "Retry Notification",
        Some("message_notification_summary"),
    );

    run_twice(
        &root,
        "req_case_tag",
        vec![
            "case".to_string(),
            "tag".to_string(),
            case_uid.clone(),
            "ops".to_string(),
            "--reason".to_string(),
            "tag case".to_string(),
        ],
        "case_tagged",
        "case_uid",
    );
    run_twice(
        &root,
        "req_case_rename",
        vec![
            "case".to_string(),
            "rename".to_string(),
            case_uid.clone(),
            "--name".to_string(),
            "Retry Case Renamed".to_string(),
            "--reason".to_string(),
            "rename case".to_string(),
        ],
        "case_renamed",
        "case_uid",
    );
    run_twice(
        &root,
        "req_case_move",
        vec![
            "case".to_string(),
            "move".to_string(),
            case_uid.clone(),
            "--group".to_string(),
            "billing".to_string(),
            "--reason".to_string(),
            "move case".to_string(),
        ],
        "case_moved",
        "case_uid",
    );
    run_twice(
        &root,
        "req_case_set_summary",
        vec![
            "case".to_string(),
            "set-summary".to_string(),
            case_uid.clone(),
            "message_case_summary".to_string(),
            "--summary".to_string(),
            "updated case summary".to_string(),
            "--reason".to_string(),
            "update case summary".to_string(),
        ],
        "case_summary_set",
        "case_uid",
    );
    run_twice(
        &root,
        "req_case_untag",
        vec![
            "case".to_string(),
            "untag".to_string(),
            case_uid.clone(),
            "ops".to_string(),
            "--reason".to_string(),
            "untag case".to_string(),
        ],
        "case_untagged",
        "case_uid",
    );
    run_twice(
        &root,
        "req_contact_email_add",
        vec![
            "contact".to_string(),
            "email".to_string(),
            "add".to_string(),
            contact_uid.clone(),
            "retry@example.com".to_string(),
            "--reason".to_string(),
            "add email".to_string(),
        ],
        "contact_email_added",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_phone_add",
        vec![
            "contact".to_string(),
            "phone".to_string(),
            "add".to_string(),
            contact_uid.clone(),
            "+1-555-0199".to_string(),
            "--reason".to_string(),
            "add phone".to_string(),
        ],
        "contact_phone_added",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_tag",
        vec![
            "contact".to_string(),
            "tag".to_string(),
            contact_uid.clone(),
            "ops".to_string(),
            "--reason".to_string(),
            "tag contact".to_string(),
        ],
        "contact_tagged",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_rename",
        vec![
            "contact".to_string(),
            "rename".to_string(),
            contact_uid.clone(),
            "--name".to_string(),
            "Retry Contact Renamed".to_string(),
            "--reason".to_string(),
            "rename contact".to_string(),
        ],
        "contact_renamed",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_move",
        vec![
            "contact".to_string(),
            "move".to_string(),
            contact_uid.clone(),
            "--group".to_string(),
            "partners".to_string(),
            "--reason".to_string(),
            "move contact".to_string(),
        ],
        "contact_moved",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_email_remove",
        vec![
            "contact".to_string(),
            "email".to_string(),
            "remove".to_string(),
            contact_uid.clone(),
            "retry@example.com".to_string(),
            "--reason".to_string(),
            "remove email".to_string(),
        ],
        "contact_email_removed",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_phone_remove",
        vec![
            "contact".to_string(),
            "phone".to_string(),
            "remove".to_string(),
            contact_uid.clone(),
            "+1-555-0199".to_string(),
            "--reason".to_string(),
            "remove phone".to_string(),
        ],
        "contact_phone_removed",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_contact_untag",
        vec![
            "contact".to_string(),
            "untag".to_string(),
            contact_uid.clone(),
            "ops".to_string(),
            "--reason".to_string(),
            "untag contact".to_string(),
        ],
        "contact_untagged",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_notification_rename",
        vec![
            "notification".to_string(),
            "rename".to_string(),
            notification_uid.clone(),
            "--name".to_string(),
            "Retry Notification Renamed".to_string(),
            "--reason".to_string(),
            "rename notification".to_string(),
        ],
        "notification_renamed",
        "notification_uid",
    );
    run_twice(
        &root,
        "req_notification_set_summary",
        vec![
            "notification".to_string(),
            "set-summary".to_string(),
            notification_uid,
            "message_notification_summary".to_string(),
            "--summary".to_string(),
            "updated notification summary".to_string(),
            "--reason".to_string(),
            "update notification summary".to_string(),
        ],
        "notification_summary_set",
        "notification_uid",
    );

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    for code in [
        "case_tagged",
        "case_renamed",
        "case_moved",
        "case_summary_set",
        "case_untagged",
        "contact_email_added",
        "contact_phone_added",
        "contact_tagged",
        "contact_renamed",
        "contact_moved",
        "contact_email_removed",
        "contact_phone_removed",
        "contact_untagged",
        "notification_renamed",
        "notification_summary_set",
    ] {
        assert_eq!(
            audit.matches(&format!("\"kind\":\"{code}\"")).count(),
            1,
            "{audit}"
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_replays_deletes_without_duplicate_audit() {
    let root = temp_root("request-id-deletes");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let case_uid = create_case(&root, None);
    let contact_uid = create_contact(&root);
    let notification_uid = create_notification(&root, "Retry Notification", None);

    run_twice(
        &root,
        "req_case_delete",
        vec![
            "case".to_string(),
            "delete".to_string(),
            case_uid,
            "--reason".to_string(),
            "delete empty case".to_string(),
        ],
        "case_deleted",
        "case_uid",
    );
    run_twice(
        &root,
        "req_contact_delete",
        vec![
            "contact".to_string(),
            "delete".to_string(),
            contact_uid,
            "--reason".to_string(),
            "delete contact".to_string(),
        ],
        "contact_deleted",
        "contact_uid",
    );
    run_twice(
        &root,
        "req_notification_delete",
        vec![
            "notification".to_string(),
            "delete".to_string(),
            notification_uid,
            "--reason".to_string(),
            "delete empty notification".to_string(),
        ],
        "notification_deleted",
        "notification_uid",
    );

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    for code in ["case_deleted", "contact_deleted", "notification_deleted"] {
        assert_eq!(
            audit.matches(&format!("\"kind\":\"{code}\"")).count(),
            1,
            "{audit}"
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_replays_membership_edits_without_duplicate_audit() {
    let root = temp_root("request-id-membership-edits");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_case_remove", 201);
    write_message(&root, "message_notification_remove", 202);
    write_message(&root, "message_notification_move", 203);
    write_message(&root, "message_case_add", 204);
    write_message(&root, "message_notification_add", 205);
    write_message(&root, "message_case_add_batch_a", 206);
    write_message(&root, "message_case_add_batch_b", 207);
    let case_uid = create_case(&root, Some("message_case_remove"));
    let case_add_uid = create_case(&root, None);
    let notification_remove_uid = create_notification(
        &root,
        "Retry Notification Remove",
        Some("message_notification_remove"),
    );
    let notification_move_from_uid = create_notification(
        &root,
        "Retry Notification Move From",
        Some("message_notification_move"),
    );
    let notification_move_to_uid = create_notification(&root, "Retry Notification Move To", None);
    let notification_add_uid = create_notification(&root, "Retry Notification Add", None);

    run_twice(
        &root,
        "req_case_add",
        vec![
            "case".to_string(),
            "add".to_string(),
            case_add_uid,
            "message_case_add".to_string(),
            "--summary".to_string(),
            "added case summary".to_string(),
            "--reason".to_string(),
            "add to case".to_string(),
        ],
        "case_message_added",
        "case_uid",
    );
    run_twice(
        &root,
        "req_notification_add",
        vec![
            "notification".to_string(),
            "add".to_string(),
            notification_add_uid,
            "message_notification_add".to_string(),
            "--summary".to_string(),
            "added notification summary".to_string(),
            "--reason".to_string(),
            "add to notification".to_string(),
        ],
        "notification_message_added",
        "notification_uid",
    );

    run_twice(
        &root,
        "req_case_remove",
        vec![
            "case".to_string(),
            "remove".to_string(),
            case_uid,
            "message_case_remove".to_string(),
            "--reason".to_string(),
            "remove from case".to_string(),
        ],
        "case_message_removed",
        "case_uid",
    );
    run_twice(
        &root,
        "req_notification_remove",
        vec![
            "notification".to_string(),
            "remove".to_string(),
            notification_remove_uid,
            "message_notification_remove".to_string(),
            "--reason".to_string(),
            "remove from notification".to_string(),
        ],
        "notification_message_removed",
        "notification_uid",
    );
    run_twice(
        &root,
        "req_notification_move",
        vec![
            "notification".to_string(),
            "move".to_string(),
            notification_move_from_uid,
            "message_notification_move".to_string(),
            notification_move_to_uid,
            "--reason".to_string(),
            "move notification message".to_string(),
        ],
        "notification_message_moved",
        "notification_uid",
    );

    let (status, stdout) = run(
        &root,
        &[
            "--request-id",
            "req_case_add_batch",
            "case",
            "add",
            "c00000000000",
            "message_case_add_batch_a",
            "message_case_add_batch_b",
            "--summary",
            "batch add summary",
            "--reason",
            "batch add",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "request_id_not_supported");

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    for code in [
        "case_message_added",
        "notification_message_added",
        "case_message_removed",
        "notification_message_removed",
        "notification_message_moved",
    ] {
        assert_eq!(
            audit.matches(&format!("\"kind\":\"{code}\"")).count(),
            1,
            "{audit}"
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_replays_draft_edits_without_duplicate_audit() {
    let root = temp_root("request-id-draft-edits");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_draft_config(&root);
    let case_uid = create_case(&root, None);
    let draft_name = create_draft(&root, &case_uid);

    run_twice(
        &root,
        "req_draft_change",
        vec![
            "case".to_string(),
            "draft".to_string(),
            "change".to_string(),
            case_uid.clone(),
            draft_name.clone(),
            "--subject".to_string(),
            "Retry Draft Updated".to_string(),
        ],
        "draft_changed",
        "draft_name",
    );

    let attachment_path = root.join("attachment.txt");
    assert!(fs::write(&attachment_path, "attachment body").is_ok());
    let attachment_arg = attachment_path.to_string_lossy().to_string();
    let command = [
        "--request-id".to_string(),
        "req_draft_attach".to_string(),
        "case".to_string(),
        "draft".to_string(),
        "attach".to_string(),
        case_uid.clone(),
        draft_name.clone(),
        attachment_arg,
    ];
    let command_refs = command.iter().map(String::as_str).collect::<Vec<_>>();
    let (status, stdout) = run(&root, &command_refs);
    assert_eq!(status, 0, "{stdout}");
    let mut first_attach = parse_one(&stdout);
    assert_eq!(first_attach["copied"], true);
    let (status, stdout) = run(&root, &command_refs);
    assert_eq!(status, 0, "{stdout}");
    let mut replayed_attach = parse_one(&stdout);
    if let Some(object) = first_attach.as_object_mut() {
        object.remove("trace");
    }
    if let Some(object) = replayed_attach.as_object_mut() {
        object.remove("trace");
    }
    assert_eq!(replayed_attach, first_attach);

    run_twice(
        &root,
        "req_draft_remove",
        vec![
            "case".to_string(),
            "draft".to_string(),
            "remove".to_string(),
            case_uid,
            draft_name,
            "--reason".to_string(),
            "remove draft".to_string(),
        ],
        "draft_removed",
        "draft_name",
    );

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    for code in ["draft_changed", "draft_removed"] {
        assert_eq!(
            audit.matches(&format!("\"kind\":\"{code}\"")).count(),
            1,
            "{audit}"
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_replays_case_lifecycle_edits_without_duplicate_audit() {
    let root = temp_root("request-id-case-lifecycle");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_draft_config(&root);
    let archive_case_uid = create_case(&root, None);
    let trash_case_uid = create_case(&root, None);

    run_twice(
        &root,
        "req_case_archive",
        vec![
            "case".to_string(),
            "archive".to_string(),
            archive_case_uid.clone(),
            "--reason".to_string(),
            "archive empty case".to_string(),
        ],
        "case_archived",
        "case_uid",
    );
    run_twice(
        &root,
        "req_case_unarchive",
        vec![
            "case".to_string(),
            "unarchive".to_string(),
            archive_case_uid,
            "--group".to_string(),
            "support".to_string(),
            "--reason".to_string(),
            "restore empty case".to_string(),
        ],
        "case_unarchived",
        "case_uid",
    );
    run_twice(
        &root,
        "req_case_trash",
        vec![
            "case".to_string(),
            "trash".to_string(),
            trash_case_uid,
            "--reason".to_string(),
            "trash empty case".to_string(),
        ],
        "case_trashed",
        "case_uid",
    );

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    for code in ["case_archived", "case_unarchived", "case_trashed"] {
        assert_eq!(
            audit.matches(&format!("\"kind\":\"{code}\"")).count(),
            1,
            "{audit}"
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn request_id_replays_message_dispositions_without_duplicate_audit() {
    let root = temp_root("request-id-message-dispositions");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam_retry", 301);
    write_message(&root, "message_trash_retry", 302);
    write_message(&root, "message_unspam_retry", 303);
    write_message(&root, "message_untrash_retry", 304);
    write_message(&root, "message_spam_batch_a", 305);
    write_message(&root, "message_spam_batch_b", 306);

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "spam",
            "message_unspam_retry",
            "--reason",
            "setup spam",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "trash",
            "message_untrash_retry",
            "--reason",
            "setup trash",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    run_twice(
        &root,
        "req_message_spam",
        vec![
            "message".to_string(),
            "spam".to_string(),
            "message_spam_retry".to_string(),
            "--reason".to_string(),
            "mark spam".to_string(),
        ],
        "message_spam_marked",
        "message_id",
    );
    run_twice(
        &root,
        "req_message_trash",
        vec![
            "message".to_string(),
            "trash".to_string(),
            "message_trash_retry".to_string(),
            "--reason".to_string(),
            "mark trash".to_string(),
        ],
        "message_trashed",
        "message_id",
    );
    run_twice(
        &root,
        "req_message_unspam",
        vec![
            "message".to_string(),
            "unspam".to_string(),
            "message_unspam_retry".to_string(),
            "--reason".to_string(),
            "restore spam".to_string(),
        ],
        "message_unspammed",
        "message_id",
    );
    run_twice(
        &root,
        "req_message_untrash",
        vec![
            "message".to_string(),
            "untrash".to_string(),
            "message_untrash_retry".to_string(),
            "--reason".to_string(),
            "restore trash".to_string(),
        ],
        "message_untrashed",
        "message_id",
    );

    let (status, stdout) = run(
        &root,
        &[
            "--request-id",
            "req_message_spam_batch",
            "message",
            "spam",
            "message_spam_batch_a",
            "message_spam_batch_b",
            "--reason",
            "batch spam",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "request_id_not_supported");

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    for (code, expected_count) in [
        ("message_spam_marked", 2usize),
        ("message_trashed", 2usize),
        ("message_unspammed", 1usize),
        ("message_untrashed", 1usize),
    ] {
        assert_eq!(
            audit.matches(&format!("\"kind\":\"{code}\"")).count(),
            expected_count,
            "{audit}"
        );
    }
    let _ = fs::remove_dir_all(root);
}
