use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, read_json, run, temp_root, update_message_json, write_json, write_message};
use serde_json::json;

fn draft_markdown(case_uid: &str, to: &str, subject: &str, body: &str) -> String {
    let raw = format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - {to}\ncc: []\nsubject: \"{subject}\"\nattachments:\n---\n\n{body}\n"
    );
    draft_with_compose(raw)
}

fn draft_with_compose(raw: String) -> String {
    let Some((frontmatter, body)) = raw.split_once("\n---\n") else {
        return raw;
    };
    format!(
        "{frontmatter}\n---\n# Draft\n\n<!-- afmail:compose:start -->\n{}\n<!-- afmail:compose:end -->\n",
        body.trim_matches(['\r', '\n'])
    )
}

fn create_case(root: &std::path::Path, name: &str, reason: &str) -> (String, std::path::PathBuf) {
    let (status, stdout) = run(
        root,
        &[
            "case", "create", "--name", name, "--group", "support", "--reason", reason,
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

fn create_case_from_message(
    root: &std::path::Path,
    name: &str,
    message_id: &str,
    reason: &str,
) -> (String, std::path::PathBuf) {
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

fn create_case_draft_and_queue_send(
    root: &std::path::Path,
    name: &str,
    reason: &str,
    subject: &str,
    body: &str,
) -> (
    String,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
    std::path::PathBuf,
) {
    let (case_uid, case_path) = create_case(root, name, reason);
    let draft_path = case_path.join("drafts/new.md");
    assert!(
        fs::write(
            &draft_path,
            draft_markdown(&case_uid, "bob@example.com", subject, body),
        )
        .is_ok()
    );
    assert_eq!(
        run(root, &["case", "draft", "send", &case_uid, "new.md"]).0,
        0
    );
    let push = parse_one(&run(root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    (case_uid, case_path, draft_path, push_id, push_path)
}

#[test]
fn draft_show_missing_draft_returns_not_found() {
    let root = temp_root("draft-show-missing");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (case_uid, _) = create_case(&root, "show-missing-thread", "show missing draft case");

    let (status, stdout) = run(&root, &["case", "draft", "show", &case_uid, "missing.md"]);
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["code"], "draft_not_found");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_list_reports_typed_local_state_without_claiming_a_send() {
    let root = temp_root("draft-list");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "list-status", "draft list case");

    assert!(
        fs::write(
            case_path.join("drafts/editable.md"),
            draft_markdown(
                &case_uid,
                "editable@example.com",
                "Editable",
                "Editable body",
            ),
        )
        .is_ok()
    );

    assert!(
        fs::write(
            case_path.join("drafts/queued-upload.md"),
            draft_markdown(
                &case_uid,
                "queue@example.com",
                "Queued upload",
                "Queued upload body",
            ),
        )
        .is_ok()
    );
    let (status, stdout) = run(
        &root,
        &["case", "draft", "upload", &case_uid, "queued-upload.md"],
    );
    assert_eq!(status, 0, "{stdout}");
    let uploaded = parse_one(&stdout);
    assert_eq!(uploaded["action"], "save_draft");

    assert!(
        fs::write(
            case_path.join("drafts/queued-send.md"),
            draft_markdown(
                &case_uid,
                "send@example.com",
                "Queued send",
                "Queued send body",
            ),
        )
        .is_ok()
    );
    let (status, stdout) = run(
        &root,
        &["case", "draft", "send", &case_uid, "queued-send.md"],
    );
    assert_eq!(status, 0, "{stdout}");
    let sent = parse_one(&stdout);
    assert_eq!(sent["action"], "send");

    let (status, stdout) = run(&root, &["case", "draft", "list", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["draft_count"], 3);
    let drafts = value["drafts"].as_array().cloned().unwrap_or_default();
    let editable = drafts
        .iter()
        .find(|draft| draft["draft_name"] == "editable.md")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert_eq!(editable["status"], "editable");
    assert!(editable["queued_action"].is_null());
    assert_eq!(editable["remote_changed"], false);
    let upload = drafts
        .iter()
        .find(|draft| draft["draft_name"] == "queued-upload.md")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert_eq!(upload["status"], "queued_upload");
    assert_eq!(upload["queued_action"], "save_draft");
    assert_eq!(upload["remote_changed"], false);
    assert!(
        upload["push_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    let send = drafts
        .iter()
        .find(|draft| draft["draft_name"] == "queued-send.md")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert_eq!(send["status"], "queued_send");
    assert_eq!(send["queued_action"], "send");
    assert_eq!(send["remote_changed"], false);
    assert!(
        send["push_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    let (status, yaml) = run(
        &root,
        &["case", "draft", "list", &case_uid, "--output", "yaml"],
    );
    assert_eq!(status, 0, "{yaml}");
    assert!(yaml.contains("status: \"queued_send\""), "{yaml}");
    assert!(yaml.contains("queued_action: \"save_draft\""), "{yaml}");
    assert!(!yaml.contains("status: \"sent\""), "{yaml}");

    let (status, plain) = run(
        &root,
        &["case", "draft", "list", &case_uid, "--output", "plain"],
    );
    assert_eq!(status, 0, "{plain}");
    assert!(plain.contains("queued_send"), "{plain}");
    assert!(plain.contains("save_draft"), "{plain}");
    assert!(!plain.contains("status=sent"), "{plain}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_remove_deletes_draft_state_and_outbound_queue() {
    let root = temp_root("draft-remove");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "remove-thread", "remove draft case");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "bob@example.com",
            "--subject",
            "Remove me",
            "--body",
            "This draft will be removed before sending.",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, &draft_name]).0,
        0
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
    assert_eq!(push["items"][0]["action"], "send");
    assert!(push["items"][0].get("eml_path").is_none());
    assert!(push["items"][0].get("message_id").is_none());
    let sent_count = fs::read_dir(root.join("messages"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("message_sent_"))
                })
                .count()
        })
        .unwrap_or(0);
    assert_eq!(sent_count, 0);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "remove",
            &case_uid,
            &draft_name,
            "--reason",
            "mistaken draft",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let removed = parse_one(&stdout);
    assert_eq!(removed["queued_removed"], true);
    assert_eq!(removed["push_ids"], serde_json::json!([push_id]));
    assert_eq!(removed["actions"], serde_json::json!(["send"]));
    assert_eq!(removed["mail_sent"], false);
    assert!(!case_path.join(format!("drafts/{draft_name}")).exists());
    assert!(
        !root
            .join(".afmail/push")
            .join(format!("{push_id}.json"))
            .exists()
    );
    let state = read_json(&case_path.join("data/drafts.json"));
    assert!(state["drafts"].get(draft_name.as_str()).is_none());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_change_updates_requested_fields_and_validates() {
    let root = temp_root("draft-change");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "change-thread", "change draft case");
    let body_path = root.join("body.txt");
    assert!(fs::write(&body_path, "Replacement body\n").is_ok());
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "alice@example.com",
            "--cc",
            "old-cc@example.com",
            "--subject",
            "Old subject",
            "--body",
            "Original body",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "change",
            &case_uid,
            &draft_name,
            "--subject",
            "New subject",
            "--to",
            "bob@example.com",
            "--clear-cc",
            "--body-file",
            "body.txt",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let changed = parse_one(&stdout);
    assert_eq!(
        changed["changed_fields"],
        serde_json::json!(["subject", "to", "cc", "body"])
    );
    assert!(
        changed["draft_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    let draft =
        fs::read_to_string(case_path.join(format!("drafts/{draft_name}"))).unwrap_or_default();
    assert!(draft.contains("subject: New subject"));
    assert!(draft.contains("bob@example.com"));
    assert!(!draft.contains("old-cc@example.com"));
    assert!(draft.contains("Replacement body"));
    let state = read_json(&case_path.join("data/drafts.json"));
    assert_eq!(
        state["drafts"][draft_name.as_str()]["last_validated_hash"],
        changed["draft_hash"]
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "change",
            &case_uid,
            &draft_name,
            "--body",
            "one",
            "--body-file",
            "body.txt",
        ],
    );
    assert_eq!(status, 2, "{stdout}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_save_send_queue_is_idempotent_and_switches_action() {
    let root = temp_root("draft-queue-switch");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "queue-thread", "queue draft case");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "bob@example.com",
            "--subject",
            "Queue me",
            "--body",
            "First body",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, stdout) = run(&root, &["case", "draft", "upload", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");
    let saved = parse_one(&stdout);
    assert_eq!(saved["action"], "save_draft");
    let push_id = saved["push_id"].as_str().unwrap_or_default().to_string();
    let (status, stdout) = run(&root, &["case", "draft", "upload", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["push_id"], push_id);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["action"], "save_draft");
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let mut pending_upload = read_json(&push_path);
    pending_upload["attempt_count"] = json!(2);
    pending_upload["step_states"] = json!([{
        "index": 0,
        "label": "append_to_mailbox_id_drafts",
        "status": "pending"
    }]);
    pending_upload["last_error"] = json!("old upload plan");
    write_json(&push_path, &pending_upload);

    let draft_path = case_path.join(format!("drafts/{draft_name}"));
    let original = fs::read_to_string(&draft_path).unwrap_or_default();
    assert!(fs::write(&draft_path, original.replace("First body", "Latest body")).is_ok());
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");
    let sent = parse_one(&stdout);
    assert_eq!(sent["push_id"], push_id);
    assert_eq!(sent["action"], "send");
    let replaced = read_json(&push_path);
    assert_eq!(replaced["attempt_count"], 0);
    assert_eq!(replaced["step_states"], json!([]));
    assert!(replaced.get("last_error").is_none());
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["action"], "send");
    assert!(
        fs::read_to_string(&draft_path)
            .unwrap_or_default()
            .contains("Latest body")
    );
    let sent_count = fs::read_dir(root.join("messages"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("message_sent_"))
                })
                .count()
        })
        .unwrap_or(0);
    assert_eq!(sent_count, 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_remove_rejects_started_outbound_push() {
    let root = temp_root("draft-remove-started");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, _case_path, draft_path, push_id, push_path) = create_case_draft_and_queue_send(
        &root,
        "sent-thread",
        "sent draft case",
        "Sent maybe",
        "Hi Bob",
    );
    let mut push_item = read_json(&push_path);
    push_item["step_states"] = serde_json::json!([
        {
            "index": 0,
            "label": "append_to_mailbox_id_drafts",
            "status": "succeeded",
            "started_rfc3339": "2026-06-09T00:00:00Z",
            "completed_rfc3339": "2026-06-09T00:00:01Z",
            "result_summary": "append succeeded"
        },
        {
            "index": 1,
            "label": "smtp_send",
            "status": "succeeded",
            "started_rfc3339": "2026-06-09T00:00:01Z",
            "completed_rfc3339": "2026-06-09T00:00:02Z",
            "result_summary": "smtp_send succeeded"
        }
    ]);
    write_json(&push_path, &push_item);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "remove",
            &case_uid,
            "new.md",
            "--reason",
            "mistaken draft",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_already_started");
    assert_eq!(value["push_id"], push_id);
    assert!(
        value["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("review its current plan_id"))
    );
    assert!(draft_path.exists());
    assert!(push_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_remove_rejects_in_progress_outbound_push() {
    let root = temp_root("draft-remove-in-progress");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, _case_path, draft_path, push_id, push_path) = create_case_draft_and_queue_send(
        &root,
        "in-progress-thread",
        "in progress draft case",
        "In progress",
        "Hi Bob",
    );
    let mut push_item = read_json(&push_path);
    push_item["step_states"] = serde_json::json!([{
        "index": 0,
        "label": "append_to_mailbox_id_drafts",
        "status": "pending",
        "started_rfc3339": "2026-06-09T00:00:00Z"
    }]);
    write_json(&push_path, &push_item);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "remove",
            &case_uid,
            "new.md",
            "--reason",
            "mistaken draft",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_already_started");
    assert_eq!(value["push_id"], push_id);
    assert!(draft_path.exists());
    assert!(push_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_action_change_rejects_started_push_without_changing_canonical_intent() {
    let root = temp_root("draft-action-change-started");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, _case_path, draft_path, _push_id, push_path) = create_case_draft_and_queue_send(
        &root,
        "action-change-thread",
        "action change case",
        "Do not replace",
        "Remote work has started",
    );
    let mut push_item = read_json(&push_path);
    push_item["step_states"] = json!([{
        "index": 0,
        "label": "append_to_mailbox_id_drafts",
        "status": "succeeded",
        "started_rfc3339": "2026-07-22T00:00:00Z",
        "completed_rfc3339": "2026-07-22T00:00:01Z",
        "result_summary": "draft uploaded"
    }]);
    write_json(&push_path, &push_item);
    let draft_before = fs::read(&draft_path).unwrap_or_default();
    let push_before = fs::read(&push_path).unwrap_or_default();

    let (status, stdout) = run(&root, &["case", "draft", "upload", &case_uid, "new.md"]);
    assert_ne!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_already_started");
    assert_eq!(fs::read(&draft_path).unwrap_or_default(), draft_before);
    assert_eq!(fs::read(&push_path).unwrap_or_default(), push_before);
    let draft = fs::read_to_string(&draft_path).unwrap_or_default();
    assert!(draft.contains("sync_intent: send"), "{draft}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_attach_copies_external_file_and_updates_frontmatter() {
    let root = temp_root("draft-attach");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "attach-thread", "new outbound case");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "bob@example.com",
            "--subject",
            "With attachment",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(fs::create_dir_all(root.join("source")).is_ok());
    assert!(fs::write(root.join("source/公司资料?.txt"), "hello").is_ok());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "attach",
            &case_uid,
            &draft_name,
            "source/公司资料?.txt",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["copied"], true);
    assert_eq!(value["attachment"], "files/公司资料_.txt");
    assert!(case_path.join("files/公司资料_.txt").is_file());
    let draft =
        fs::read_to_string(case_path.join(format!("drafts/{draft_name}"))).unwrap_or_default();
    assert!(draft.contains("attachments:\n  - files/公司资料_.txt"));

    let (status, stdout) = run(
        &root,
        &["case", "draft", "validate", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "attach",
            &case_uid,
            &draft_name,
            "source/公司资料?.txt",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["already_present"], true);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_show_reports_content_queue_and_attachment_status() {
    let root = temp_root("draft-show");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "show-thread", "show draft case");
    assert!(fs::write(case_path.join("files/present.txt"), "hello").is_ok());
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nsubject: \"Review me\"\nto:\n  - bob@example.com\ncc:\n  - team@example.com\nattachments:\n  - files/present.txt\n  - files/missing.txt\n---\nReview body\n"
    ));
    assert!(fs::write(case_path.join("drafts/show.md"), draft).is_ok());

    let (status, stdout) = run(&root, &["case", "draft", "show", &case_uid, "show.md"]);
    assert_eq!(status, 0, "{stdout}");
    let shown = parse_one(&stdout);
    assert_eq!(shown["draft"]["frontmatter"]["subject"], "Review me");
    assert_eq!(shown["draft"]["frontmatter"]["to"][0], "bob@example.com");
    assert_eq!(shown["draft"]["frontmatter"]["cc"][0], "team@example.com");
    let shown_body = shown["draft"]["body_markdown"].as_str().unwrap_or_default();
    assert!(shown.get("authored_body").is_none());
    let authored_start = shown["analysis"]["authored_body_range"]["start_bytes"]
        .as_u64()
        .unwrap_or_default() as usize;
    let authored_end = shown["analysis"]["authored_body_range"]["end_bytes"]
        .as_u64()
        .unwrap_or_default() as usize;
    assert_eq!(&shown_body[authored_start..authored_end], "Review body");
    assert!(
        shown["draft"]["body_markdown"]
            .as_str()
            .is_some_and(|body| body.contains("<!-- afmail:compose:start -->"))
    );
    assert_eq!(shown["analysis"]["completeness"], "complete");
    assert_eq!(
        shown["draft"]["frontmatter"]["attachments"][0],
        "files/present.txt"
    );
    assert_eq!(shown["analysis"]["attachments"][0]["exists"], true);
    assert_eq!(shown["analysis"]["attachments"][0]["size_bytes"], 5);
    assert_eq!(
        shown["draft"]["frontmatter"]["attachments"][1],
        "files/missing.txt"
    );
    assert_eq!(shown["analysis"]["attachments"][1]["exists"], false);
    assert!(shown["analysis"]["attachments"][1]["size_bytes"].is_null());
    assert_eq!(shown["analysis"]["status"], "editable");
    assert!(shown["analysis"]["push_id"].is_null());
    assert!(shown.get("queued_action").is_none());
    assert!(shown.get("remote_changed").is_none());

    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, "show.md"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "draft_invalid");
    assert!(fs::write(case_path.join("files/missing.txt"), "later").is_ok());
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, "show.md"]);
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(&root, &["case", "draft", "show", &case_uid, "show.md"]);
    assert_eq!(status, 0, "{stdout}");
    let queued = parse_one(&stdout);
    assert_eq!(queued["analysis"]["status"], "queued_send");
    assert_eq!(queued["draft"]["frontmatter"]["sync_intent"], "send");
    assert!(queued.get("queued_action").is_none());
    assert!(queued.get("remote_changed").is_none());
    assert!(
        queued["analysis"]["push_id"]
            .as_str()
            .map(|push_id| push_id.starts_with("push_"))
            .unwrap_or(false)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_save_send_intent_toggles_in_place() {
    let root = temp_root("draft-toggle-intent");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "toggle-thread", "toggle outbound case");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Toggle\"\nattachments:\n---\n\nHello\n"
    ));
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );

    // Save queues exactly one SaveDraft item.
    let saved = parse_one(&run(&root, &["case", "draft", "upload", &case_uid, "new.md"]).1);
    assert_eq!(saved["action"], "save_draft");
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["kind"], "draft");
    assert_eq!(push["items"][0]["action"], "save_draft");
    let push_id = push["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!push_id.is_empty());

    // Toggle to Send in place: still ONE queued item, same push_id, the intent
    // follows the latest command, and no stale step state remains.
    let sent = parse_one(&run(&root, &["case", "draft", "send", &case_uid, "new.md"]).1);
    assert_eq!(sent["action"], "send");
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(
        push["count"], 1,
        "toggling intent must not add a second queued item: {push}"
    );
    assert_eq!(push["items"][0]["push_id"], push_id);
    assert_eq!(push["items"][0]["action"], "send");
    assert!(
        push["items"][0]["step_states"]
            .as_array()
            .is_some_and(|states| states.is_empty())
    );

    // Toggle back to Save: still one item, same push_id.
    let saved_again = parse_one(&run(&root, &["case", "draft", "upload", &case_uid, "new.md"]).1);
    assert_eq!(saved_again["action"], "save_draft");
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["push_id"], push_id);
    assert_eq!(push["items"][0]["action"], "save_draft");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn incomplete_new_draft_uploads_but_cannot_enter_send_queue() {
    let root = temp_root("draft-incomplete-new");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) =
        create_case(&root, "incomplete-new", "verify incomplete draft boundary");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "bob@example.com",
            "--subject",
            "Needs a body",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let draft_path = case_path.join(format!("drafts/{draft_name}"));
    let original = fs::read_to_string(&draft_path).unwrap_or_default();
    assert!(original.contains("<!-- afmail:compose:start -->"));
    assert!(original.contains("<!-- afmail:compose:end -->"));

    let (status, stdout) = run(
        &root,
        &["case", "draft", "validate", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["completeness"], "incomplete");

    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, &draft_name]);
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "draft_incomplete");
    assert_eq!(error["remote_changed"], false);
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    assert_eq!(
        fs::read_to_string(&draft_path).unwrap_or_default(),
        original
    );

    let (status, stdout) = run(&root, &["case", "draft", "upload", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");
    let upload = parse_one(&stdout);
    assert_eq!(upload["action"], "save_draft");
    assert_eq!(upload["completeness"], "incomplete");
    let push_id = upload["push_id"].as_str().unwrap_or_default().to_string();
    let listed = parse_one(&run(&root, &["case", "draft", "list", &case_uid]).1);
    assert_eq!(listed["drafts"][0]["status"], "queued_upload");
    assert_eq!(listed["drafts"][0]["completeness"], "incomplete");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "change",
            &case_uid,
            &draft_name,
            "--body",
            "Now this draft has authored text.",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["completeness"], "complete");
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");
    let send = parse_one(&stdout);
    assert_eq!(send["push_id"], push_id);
    assert_eq!(send["action"], "send");
    assert_eq!(send["completeness"], "complete");
    let listed = parse_one(&run(&root, &["case", "draft", "list", &case_uid]).1);
    assert_eq!(listed["drafts"][0]["status"], "queued_send");
    assert_eq!(listed["drafts"][0]["completeness"], "complete");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn queued_send_is_revalidated_if_authored_text_is_removed_before_push() {
    let root = temp_root("draft-incomplete-before-push");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, _) = create_case(
        &root,
        "incomplete-before-push",
        "verify push-time draft validation",
    );
    let created = parse_one(
        &run(
            &root,
            &[
                "case",
                "draft",
                "new",
                &case_uid,
                "--to",
                "bob@example.com",
                "--subject",
                "Revalidate me",
                "--body",
                "This initially has authored text.",
            ],
        )
        .1,
    );
    let draft_name = created["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "change",
            &case_uid,
            &draft_name,
            "--body",
            "   ",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["completeness"], "incomplete");

    let (status, stdout) = run(&root, &["push"]);
    assert_eq!(status, 1, "{stdout}");
    let preview_error = parse_one(&stdout);
    assert_eq!(preview_error["code"], "draft_incomplete");
    assert_eq!(preview_error["remote_changed"], false);
    let queue_files = fs::read_dir(root.join(".afmail/push"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                })
                .count()
        })
        .unwrap_or_default();
    assert_eq!(queue_files, 1);
    let drafts = parse_one(&run(&root, &["case", "draft", "list", &case_uid]).1);
    assert_eq!(drafts["drafts"][0]["status"], "queued_send");
    assert_eq!(drafts["drafts"][0]["completeness"], "incomplete");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_new_and_draft_new_from_scratch() {
    let root = temp_root("draft-new");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "create",
            "--name",
            "new-thread",
            "--group",
            "support",
            "--reason",
            "new outbound case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let case_uid = value["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty(), "{value}");
    let case_path = root.join(value["case_path"].as_str().unwrap_or_default());
    assert!(case_path.join("case.md").exists());
    assert!(case_path.join("data/case.json").exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "plumber@example.com <plumber@example.com>",
            "--subject",
            "Leak under the sink",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let draft_name = value["draft_name"].as_str().unwrap_or_default().to_string();
    assert!(!draft_name.is_empty());
    let draft_path = case_path.join(format!("drafts/{draft_name}"));
    assert!(draft_path.exists());
    let draft_text = fs::read_to_string(&draft_path).unwrap_or_default();
    assert!(draft_text.contains("plumber@example.com"));
    assert!(!draft_text.contains("plumber@example.com <plumber@example.com>"));

    // The scaffolded draft validates against the case with no inbound message.
    let (status, stdout) = run(
        &root,
        &["case", "draft", "validate", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reply_quote_and_identity_footer_do_not_count_as_authored_text() {
    let root = temp_root("draft-incomplete-reply");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert!(fs::create_dir_all(root.join("identities")).is_ok());
    assert!(fs::write(
        root.join("identities/me.md"),
        "---\nkind: identity\nidentity: me\nfooter: |\n  --\n  Me\n---\nLocal identity notes.\n",
    )
    .is_ok());
    write_message(&root, "message_incomplete_reply", 73, None);
    let (case_uid, case_path) = create_case_from_message(
        &root,
        "incomplete-reply",
        "message_incomplete_reply",
        "verify quote boundary",
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "reply",
            &case_uid,
            "message_incomplete_reply",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let draft =
        fs::read_to_string(case_path.join(format!("drafts/{draft_name}"))).unwrap_or_default();
    assert!(draft.contains("<!-- afmail:quote:start -->"));
    assert!(draft.contains("> Body"));
    assert!(draft.ends_with("--\nMe\n"));

    let shown = parse_one(&run(&root, &["case", "draft", "show", &case_uid, &draft_name]).1);
    let shown_body = shown["draft"]["body_markdown"].as_str().unwrap_or_default();
    let authored_start = shown["analysis"]["authored_body_range"]["start_bytes"]
        .as_u64()
        .unwrap_or_default() as usize;
    let authored_end = shown["analysis"]["authored_body_range"]["end_bytes"]
        .as_u64()
        .unwrap_or_default() as usize;
    assert_eq!(&shown_body[authored_start..authored_end], "");
    assert_eq!(shown["analysis"]["completeness"], "incomplete");
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, &draft_name]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "draft_incomplete");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "change",
            &case_uid,
            &draft_name,
            "--body",
            "Thanks, I will handle this.",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["completeness"], "complete");
    let changed =
        fs::read_to_string(case_path.join(format!("drafts/{draft_name}"))).unwrap_or_default();
    assert!(changed.contains("Thanks, I will handle this."));
    assert!(changed.contains("<!-- afmail:quote:start -->"));
    assert!(changed.contains("> Body"));
    assert!(changed.ends_with("--\nMe\n"));
    assert_eq!(changed.matches("<!-- afmail:compose:start -->").count(), 1);
    assert_eq!(changed.matches("<!-- afmail:compose:end -->").count(), 1);

    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, &draft_name]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["action"], "send");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_reply_scaffolds_prefilled_draft() {
    let root = temp_root("case-reply");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_reply", 60, None);
    let (case_uid, case_path) =
        create_case_from_message(&root, "case-reply", "message_reply", "seed");

    let (status, stdout) = run(
        &root,
        &["case", "draft", "reply", &case_uid, "message_reply"],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["draft_name"], "reply-message_reply.md");

    let draft =
        fs::read_to_string(case_path.join("drafts/reply-message_reply.md")).unwrap_or_default();
    assert!(draft.contains("reply_to_message_id: message_reply"));
    assert!(draft.contains("send_intent: reply"));
    assert!(draft.contains("alice@example.com"));
    assert!(draft.contains("Re: Contract renewal"));
    assert!(draft.contains("> Body"));

    // The scaffolded draft must validate against the case.
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "validate",
            &case_uid,
            "reply-message_reply.md",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    // A second reply must not clobber the first.
    let (status, _) = run(
        &root,
        &["case", "draft", "reply", &case_uid, "message_reply"],
    );
    assert_eq!(status, 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_reply_all_includes_original_recipients_minus_self() {
    let root = temp_root("case-reply-all");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_ra", 70, None);
    // Give the message multiple recipients, including this account itself.
    update_message_json(&root, "message_ra", |message| {
        message["from"] = json!("Alice <alice@example.com>");
        message["to"] = json!(["me@example.com", "Bob <bob@example.com>"]);
        message["cc"] = json!(["carol@example.com"]);
    });
    let (case_uid, case_path) = create_case_from_message(&root, "case-ra", "message_ra", "seed");

    let (status, stdout) = run(
        &root,
        &["case", "draft", "reply", &case_uid, "message_ra", "--all"],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft =
        fs::read_to_string(case_path.join("drafts/reply-message_ra.md")).unwrap_or_default();
    // To carries the sender plus the other To recipient; Cc carries the original
    // Cc. This account's own address is excluded from both.
    assert!(draft.contains("alice@example.com"));
    assert!(draft.contains("bob@example.com"));
    assert!(draft.contains("carol@example.com"));
    assert!(!draft.contains("me@example.com"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_reply_prefers_reply_to_header() {
    let root = temp_root("case-reply-to");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_rt", 72, None);
    update_message_json(&root, "message_rt", |message| {
        message["from"] = json!("News Bot <news@example.com>");
        message["reply_to"] = json!(["Support Desk <support@example.net>"]);
    });
    let (case_uid, case_path) = create_case_from_message(&root, "case-rt", "message_rt", "seed");

    let (status, stdout) = run(&root, &["case", "draft", "reply", &case_uid, "message_rt"]);
    assert_eq!(status, 0, "{stdout}");
    let draft =
        fs::read_to_string(case_path.join("drafts/reply-message_rt.md")).unwrap_or_default();
    let frontmatter = draft.split("\n---\n").next().unwrap_or_default();
    assert!(frontmatter.contains("Support Desk <support@example.net>"));
    assert!(!frontmatter.contains("News Bot <news@example.com>"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_save_send_validate_current_content_without_freshness_gate() {
    let root = temp_root("draft-current-content");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "fresh-thread", "fresh draft case");
    let draft_path = case_path.join("drafts/new.md");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Hello\"\nattachments:\n---\n\nHi Bob\n"
    ));
    assert!(fs::write(&draft_path, &draft).is_ok());

    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, "new.md"]);
    assert_eq!(status, 0, "{stdout}");
    let queued = parse_one(&stdout);
    assert_eq!(queued["action"], "send");
    assert!(queued.get("draft_hash").is_none());
    let push_id = queued["push_id"].as_str().unwrap_or_default().to_string();
    let state_path = case_path.join("data/drafts.json");
    let state = read_json(&state_path);
    let first_hash = state["drafts"]["new.md"]["last_validated_hash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(first_hash.starts_with("blake3:"));
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["items"][0]["push_id"], push_id);
    assert!(
        push["items"][0]["outbound"]["draft_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    assert!(
        push["items"][0]["outbound"]["mime_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    assert_eq!(push["items"][0]["outbound"]["identity"]["identity"], "me");
    assert_eq!(
        push["items"][0]["outbound"]["to"],
        json!(["bob@example.com"])
    );
    assert_eq!(push["items"][0]["outbound"]["subject"], "Hello");
    assert!(
        push["items"][0]["outbound"]["review_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("drafts/new.md"))
    );
    assert!(push["items"][0].get("eml_path").is_none());
    assert!(push["items"][0].get("message_id").is_none());
    let first_plan_id = push["plan_id"].clone();
    let first_plan_draft_hash = push["items"][0]["outbound"]["draft_hash"].clone();

    assert_eq!(
        run(
            &root,
            &[
                "case",
                "move",
                &case_uid,
                "--group",
                "waiting",
                "--reason",
                "better group",
            ],
        )
        .0,
        0
    );
    let moved_path = root.join(format!("cases/waiting/{case_uid}-fresh-thread"));
    let moved_state_path = moved_path.join("data/drafts.json");
    assert!(moved_state_path.is_file());
    let draft_path = moved_path.join("drafts/new.md");
    assert!(fs::write(&draft_path, draft.replace("Hi Bob", "Hi Bob\nP.S. changed")).is_ok());

    let (status, stdout) = run(&root, &["case", "draft", "upload", &case_uid, "new.md"]);
    assert_eq!(status, 0, "{stdout}");
    let requeued = parse_one(&stdout);
    assert_eq!(requeued["push_id"], push_id);
    assert_eq!(requeued["action"], "save_draft");
    let new_state = read_json(&moved_state_path);
    let new_hash = new_state["drafts"]["new.md"]["last_validated_hash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(new_hash, first_hash);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["action"], "save_draft");
    assert_ne!(push["plan_id"], first_plan_id);
    assert_ne!(
        push["items"][0]["outbound"]["draft_hash"],
        first_plan_draft_hash
    );
    let _ = fs::remove_dir_all(root);
}
