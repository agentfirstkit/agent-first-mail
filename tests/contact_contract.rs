use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, temp_root};

fn parse_lines(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[test]
fn contact_delete_protects_notes_and_frees_email() {
    let root = temp_root("contact-delete");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Alice",
            "--email",
            "alice@example.com",
            "--reason",
            "new contact",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let contact_uid = parse_one(&stdout)["contact_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "notes",
            "append",
            &contact_uid,
            "--text",
            "keep this note",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "delete",
            &contact_uid,
            "--reason",
            "remove duplicate",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "contact_notes_present");

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "delete",
            &contact_uid,
            "--allow-notes",
            "--reason",
            "remove duplicate",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Alice Recreated",
            "--email",
            "alice@example.com",
            "--reason",
            "recreate contact",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&run(&root, &["contact", "list"]).1)["count"], 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn contact_notes_append_and_replace_write_redacted_audit() {
    let root = temp_root("contact-notes-audit");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Audited Contact",
            "--email",
            "audit-notes@example.com",
            "--reason",
            "new contact",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let contact_uid = parse_one(&stdout)["contact_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let append_text = "private contact note";
    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "notes",
            "append",
            &contact_uid,
            "--text",
            append_text,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["contact_uid"], contact_uid);
    assert_eq!(value["text_len_bytes"], append_text.len());
    assert!(
        value["text_hash"]
            .as_str()
            .unwrap_or_default()
            .starts_with("fnv1a64:")
    );
    assert!(
        value["notes_path"]
            .as_str()
            .unwrap_or_default()
            .starts_with("contacts/")
    );

    let replace_text = "replacement contact note";
    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "notes",
            "replace",
            &contact_uid,
            "--text",
            replace_text,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["contact_uid"], contact_uid);
    assert_eq!(value["text_len_bytes"], replace_text.len());
    assert!(
        value["text_hash"]
            .as_str()
            .unwrap_or_default()
            .starts_with("fnv1a64:")
    );

    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    let events = parse_lines(&audit);
    let appended = events
        .iter()
        .find(|event| event["kind"] == "contact_notes_appended");
    assert!(appended.is_some(), "{audit}");
    let Some(appended) = appended else {
        return;
    };
    assert_eq!(appended["targets"][0]["kind"], "contact");
    assert_eq!(appended["targets"][0]["id"], contact_uid);
    assert_eq!(appended["operation"], "append");
    assert_eq!(appended["text_len_bytes"], append_text.len());
    assert!(
        appended["text_hash"]
            .as_str()
            .unwrap_or_default()
            .starts_with("fnv1a64:")
    );
    assert!(
        !appended.to_string().contains(append_text),
        "audit must not store contact note body: {appended}"
    );

    let replaced = events
        .iter()
        .find(|event| event["kind"] == "contact_notes_replaced");
    assert!(replaced.is_some(), "{audit}");
    let Some(replaced) = replaced else {
        return;
    };
    assert_eq!(replaced["targets"][0]["kind"], "contact");
    assert_eq!(replaced["targets"][0]["id"], contact_uid);
    assert_eq!(replaced["operation"], "replace");
    assert_eq!(replaced["text_len_bytes"], replace_text.len());
    assert!(
        !replaced.to_string().contains(replace_text),
        "audit must not store contact note body: {replaced}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn contact_index_fail_closed_on_invalid_and_duplicate_email() {
    let root = temp_root("contact-index-fail-closed");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Alice",
            "--email",
            "shared@example.com",
            "--reason",
            "seed alice",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let alice_uid = parse_one(&stdout)["contact_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let alice_path = fs::read_dir(root.join("contacts").join("people"))
        .ok()
        .and_then(|entries| entries.filter_map(std::result::Result::ok).next())
        .map(|entry| entry.path())
        .unwrap_or_default();
    assert!(alice_path.is_file(), "{alice_path:?}");

    let duplicate_path = root
        .join("contacts")
        .join("people")
        .join("p20990101001-duplicate.md");
    let duplicate = fs::read_to_string(&alice_path)
        .unwrap_or_default()
        .replace(&alice_uid, "p20990101001")
        .replace("Alice", "Duplicate");
    assert!(fs::write(&duplicate_path, duplicate).is_ok());

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 1, "{stdout}");
    let doctor = parse_one(&stdout);
    assert_eq!(doctor["checks"]["contacts"], true);
    assert!(
        doctor["issues"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|issue| issue["code"] == "contact_email_duplicate")
    );

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Bob",
            "--email",
            "bob@example.com",
            "--reason",
            "seed bob",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "contact_index_invalid");
    assert_eq!(error["diagnostics"][0]["code"], "contact_email_duplicate");

    assert!(fs::remove_file(&duplicate_path).is_ok());
    let invalid_path = root
        .join("contacts")
        .join("people")
        .join("p20990101002-broken.md");
    assert!(fs::write(&invalid_path, "---\ncontact_uid: [not valid]\n---\n").is_ok());

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 1, "{stdout}");
    let doctor = parse_one(&stdout);
    assert!(
        doctor["issues"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|issue| issue["code"] == "contact_invalid")
    );

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "email",
            "add",
            &alice_uid,
            "alice2@example.com",
            "--reason",
            "alternate email",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "contact_index_invalid");
    let _ = fs::remove_dir_all(root);
}
