use serde_json::json;
use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, temp_root, write_json};

#[test]
fn smtp_authentication_is_paired_but_configuration_can_be_completed_incrementally() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(run(root.path(), &["init"]).0, 0);
    for (first, value, missing) in [
        ("smtp.username", "support", "smtp.password_secret"),
        (
            "smtp.password_secret",
            "env:AFMAIL_TEST_ABSENT_SMTP_SECRET",
            "smtp.username",
        ),
    ] {
        write_json(
            &root.path().join(".afmail/config.json"),
            &json!({
                "smtp":{"host":"127.0.0.1"},
                "actions":{"draft":{"send":{"steps":[{"smtp_send":{}}]}}},
                "identities":[{"identity":"support","name":"Support","email":"support@example.com","default":true}]
            }),
        );
        assert_eq!(run(root.path(), &["config", "set", first, value]).0, 0);
        let (code, output) = run(root.path(), &["status"]);
        assert_eq!(code, 0, "{output}");
        let readiness = parse_one(&output)["readiness"].clone();
        assert_eq!(readiness["ready_for_send"], false);
        assert_eq!(
            readiness["senders"]["smtp"]["missing_config"],
            json!([missing])
        );
        let config = agent_first_mail::config::MailConfig::load(root.path()).unwrap();
        let error = config.require_smtp().unwrap_err();
        assert_eq!(error.error_code, "config_missing");
        assert_eq!(error.details.unwrap()["missing_config"], json!([missing]));
        if cfg!(feature = "smtp") {
            let (code, output) = run(root.path(), &["remote", "test"]);
            assert_eq!(code, 1, "{output}");
            assert_eq!(parse_one(&output)["missing_config"], json!([missing]));
        }
        let replacement = if missing == "smtp.username" {
            "support"
        } else {
            "test-password"
        };
        assert_eq!(
            run(root.path(), &["config", "set", missing, replacement]).0,
            0
        );
        assert_eq!(
            parse_one(&run(root.path(), &["status"]).1)["readiness"]["ready_for_send"],
            cfg!(feature = "smtp")
        );
    }
}

#[test]
fn consumer_diagnostics_report_corruption_even_without_the_cloudflare_feature() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(run(root.path(), &["init"]).0, 0);
    let directory = root.path().join(".afmail/cloudflare-consumer");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("journal.json"), b"broken journal").unwrap();
    let (code, output) = run(root.path(), &["status"]);
    assert_eq!(code, 0, "{output}");
    assert_eq!(
        parse_one(&output)["cloudflare_consumer"]["error"]["code"],
        "consumer_journal_invalid"
    );
    let (code, output) = run(root.path(), &["doctor"]);
    assert_eq!(code, 1, "{output}");
    let issues = parse_one(&output)["issues"].as_array().unwrap().clone();
    assert!(
        issues.iter().any(
            |issue| issue["code"] == "consumer_journal_invalid" && issue["repairable"] == false
        )
    );
    assert_eq!(
        fs::read(directory.join("journal.json")).unwrap(),
        b"broken journal"
    );
}

#[test]
fn invalid_completion_markers_never_settle_or_rewrite_pending_tasks() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(run(root.path(), &["init"]).0, 0);
    let directory = root.path().join(".afmail/cloudflare-consumer");
    fs::create_dir_all(directory.join("batches")).unwrap();
    let journal = directory.join("journal.json");
    let marker = directory.join("batches/mail_pending.completed");
    write_json(
        &journal,
        &json!({
            "schema_name":"cloudflare_consumer", "schema_version":2,
            "journal":{"receipts":{},"tasks":{"key":{"message_id":"message_pending","status":"pending"}},
            "active":{"task_id":"mail_pending","content_keys":["key"],"retry_after_epoch_s":0}}
        }),
    );
    let original = fs::read(&journal).unwrap();
    for bytes in [
        b"{".to_vec(), b"{}".to_vec(), vec![b' ';4097],
        serde_json::to_vec(&json!({"schema_name":"mail_received_task_completed","schema_version":1,"task_id":"mail_other"})).unwrap(),
        serde_json::to_vec(&json!({"schema_name":"mail_received_task_completed","schema_version":2,"task_id":"mail_pending"})).unwrap(),
    ] {
        fs::write(&marker, &bytes).unwrap();
        let (code, output) = run(root.path(), &["status"]);
        assert_eq!(code, 0, "{output}");
        assert_eq!(parse_one(&output)["cloudflare_consumer"]["error"]["code"], "consumer_completion_invalid");
        let (code, output) = run(root.path(), &["doctor"]);
        assert_eq!(code, 1, "{output}");
        assert!(parse_one(&output)["issues"].as_array().unwrap().iter().any(|issue|
            issue["code"] == "consumer_completion_invalid" && issue["path"] == ".afmail/cloudflare-consumer/batches/mail_pending.completed"));
        assert_eq!(fs::read(&journal).unwrap(), original);
        assert_eq!(fs::read(&marker).unwrap(), bytes);
    }
    write_json(
        &marker,
        &json!({"schema_name":"mail_received_task_completed","schema_version":1,"task_id":"mail_pending"}),
    );
    for (task_id, keys, task_status) in [
        ("mail_pending", json!(["key", "key"]), "pending"),
        ("mail_pending", json!(["missing"]), "pending"),
        ("../outside", json!(["key"]), "pending"),
        ("mail_pending", json!(["key"]), "completed"),
    ] {
        let mut invalid = serde_json::from_slice::<serde_json::Value>(&original).unwrap();
        invalid["journal"]["active"]["task_id"] = json!(task_id);
        invalid["journal"]["active"]["content_keys"] = keys;
        invalid["journal"]["tasks"]["key"]["status"] = json!(task_status);
        write_json(&journal, &invalid);
        let before = fs::read(&journal).unwrap();
        let (code, output) = run(root.path(), &["status"]);
        assert_eq!(code, 0, "{output}");
        assert_eq!(
            parse_one(&output)["cloudflare_consumer"]["error"]["code"],
            "consumer_journal_invalid"
        );
        assert_eq!(fs::read(&journal).unwrap(), before);
    }
}

#[test]
fn consumer_snapshot_recognizes_completion_without_rewriting_an_older_journal() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(run(root.path(), &["init"]).0, 0);
    let directory = root.path().join(".afmail/cloudflare-consumer");
    fs::create_dir_all(directory.join("batches")).unwrap();
    let path = directory.join("journal.json");
    write_json(
        &path,
        &json!({
            "schema_name":"cloudflare_consumer", "schema_version":2,
            "journal": {"receipts":{}, "tasks":{"content":{"message_id":"message_old","status":"pending"}},
            "active":{"task_id":"mail_old", "content_keys":["content"], "retry_after_epoch_s":0}}
        }),
    );
    let before = fs::read(&path).unwrap();
    let status = parse_one(&run(root.path(), &["status"]).1);
    assert_eq!(
        status["cloudflare_consumer"]["active_batch"]["attempt_count"],
        0
    );
    assert!(status["cloudflare_consumer"]["active_batch"]["last_error_code"].is_null());
    write_json(
        &directory.join("batches/mail_old.completed"),
        &json!({
            "schema_name":"mail_received_task_completed", "schema_version":1, "task_id":"mail_old"
        }),
    );
    let (code, output) = run(root.path(), &["status"]);
    assert_eq!(code, 0, "{output}");
    let state = parse_one(&output)["cloudflare_consumer"].clone();
    assert_eq!(state["pending_message_count"], 0);
    assert_eq!(state["completed_message_count"], 1);
    assert!(state["active_batch"].is_null());
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(directory.join("batches/mail_old.completed").is_file());
}

#[test]
fn readiness_follows_sources_features_and_the_selected_send_steps_without_resolving_secrets() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(run(root.path(), &["init"]).0, 0);
    let path = root.path().join(".afmail/config.json");
    let identity = json!([{"identity":"support","name":"Support","email":"support@example.com","default":true}]);
    let status = || {
        let (code, output) = run(root.path(), &["status"]);
        assert_eq!(code, 0, "{output}");
        parse_one(&output)["readiness"].clone()
    };
    let files = json!({"inquiries":{"inbox_path":"input","outbox_path":"output"}});
    write_json(
        &path,
        &json!({"files":{"sources":files},"identities":identity}),
    );
    let value = status();
    assert_eq!(value["ready_for_pull"], cfg!(feature = "files"));
    assert_eq!(value["ready_for_send"], cfg!(feature = "files"));
    assert_eq!(value["missing_config"], json!([]));
    assert_eq!(value["imap_configured"], false);
    assert!(!root.path().join("input").exists());
    assert!(!root.path().join("output").exists());

    write_json(
        &path,
        &json!({"files":{"sources":files},"imap":{"host":"imap.example.com"},"identities":identity}),
    );
    let value = status();
    assert_eq!(value["ready_for_pull"], false);
    assert_eq!(value["sources"]["files"]["ready"], cfg!(feature = "files"));
    assert!(
        value["missing_config"]
            .as_array()
            .unwrap()
            .contains(&json!("imap.username"))
    );

    let cloudflare = json!({"account_id":"a".repeat(32),"r2":{"bucket":"spool","access_key_id":"key","secret_access_key_secret":"env:AFMAIL_TEST_ABSENT_R2_SECRET"},"sending":{"api_token_secret":"env:AFMAIL_TEST_ABSENT_SEND_SECRET"}});
    write_json(
        &path,
        &json!({"cloudflare":cloudflare,"actions":{"draft":{"send":{"steps":[{"cloudflare_send":{}}]}}},"identities":identity}),
    );
    let value = status();
    assert_eq!(value["ready_for_pull"], cfg!(feature = "cloudflare"));
    assert_eq!(value["ready_for_send"], cfg!(feature = "cloudflare"));
    assert_eq!(value["missing_config"], json!([]));
    // The default email send workflow also appends to IMAP mailboxes.
    write_json(
        &path,
        &json!({"cloudflare":cloudflare,"identities":identity}),
    );
    assert_eq!(status()["ready_for_send"], false);

    // A local SMTP relay may deliberately use no authentication or IMAP.
    write_json(
        &path,
        &json!({"smtp":{"host":"localhost"},"actions":{"draft":{"send":{"steps":[{"smtp_send":{}}]}}},"identities":identity}),
    );
    let value = status();
    assert_eq!(value["ready_for_send"], cfg!(feature = "smtp"));
    assert_eq!(value["senders"]["smtp"]["missing_config"], json!([]));
}

#[test]
fn imap_pull_defaults_do_not_change_connection_or_send_readiness() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(run(root.path(), &["init"]).0, 0);
    write_json(
        &root.path().join(".afmail/config.json"),
        &json!({
            "imap":{"host":"imap.example.com","username":"support","password_secret":"env:AFMAIL_TEST_ABSENT_IMAP_SECRET"},
            "smtp":{"host":"localhost"},
            "actions":{"pull":{"default_mailbox_ids":[]}},
            "identities":[{"identity":"support","name":"Support","email":"support@example.com","default":true}]
        }),
    );
    let (code, output) = run(root.path(), &["status"]);
    assert_eq!(code, 0, "{output}");
    let value = parse_one(&output)["readiness"].clone();
    assert_eq!(value["ready_for_pull"], false);
    assert_eq!(value["sources"]["imap"]["ready"], false);
    assert_eq!(
        value["sources"]["imap"]["missing_config"],
        json!(["actions.pull.default_mailbox_ids"])
    );
    // The default send workflow still has usable IMAP append steps.
    assert_eq!(value["imap_configured"], true);
    assert_eq!(value["ready_for_send"], cfg!(feature = "smtp"));
    let (code, output) = run(root.path(), &["pull", "--dry-run"]);
    assert_eq!(code, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "config_invalid");
}

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

#[test]
fn send_steps_require_exactly_one_smtp_send() {
    let root = temp_root("send-steps");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(run(&root, &["init"]).0, 0);
    for steps in [
        json!([]),
        json!([{"append_to_mailbox_id":"sent"}]),
        json!([{"smtp_send":{}},{"smtp_send":{}}]),
        json!([{"smtp_send":{}},{"cloudflare_send":{}}]),
    ] {
        let (status, stdout) = run(
            &root,
            &[
                "config",
                "set",
                "actions.draft.send.steps",
                &steps.to_string(),
            ],
        );
        assert_eq!(status, 1, "{stdout}");
        let error = parse_one(&stdout);
        assert_eq!(error["code"], "config_invalid");
        assert_eq!(
            error["message"],
            "actions.draft.send.steps must contain exactly one send step (smtp_send or cloudflare_send)"
        );
    }
    assert_eq!(
        run(
            &root,
            &[
                "config",
                "set",
                "actions.draft.send.steps",
                "[{\"smtp_send\":{}}]"
            ]
        )
        .0,
        0
    );
    assert_eq!(
        run(&root, &["config", "set", "actions.draft.save.steps", "[]"]).0,
        0
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cloudflare_send_config_has_one_transport_and_redacts_its_token() {
    let root = temp_root("cloudflare-sending-config");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(run(&root, &["init"]).0, 0);
    for (key, value) in [
        ("cloudflare.account_id", "0123456789abcdef0123456789abcdef"),
        ("cloudflare.sending.api_token_secret", "test-sending-token"),
        ("actions.draft.save.steps", "[]"),
        ("actions.draft.send.steps", "[{\"cloudflare_send\":{}}]"),
    ] {
        let (status, stdout) = run(&root, &["config", "set", key, value]);
        assert_eq!(status, 0, "{stdout}");
        assert!(!stdout.contains("test-sending-token"));
    }
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(!stdout.contains("test-sending-token"));
    for steps in [
        r#"[{"smtp_send":{},"cloudflare_send":{}}]"#,
        r#"[{"cloudflare_send":{"ignored":"value"}}]"#,
        r#"[{"cloudflare_send":{},"on":"reply_to_message"}]"#,
    ] {
        let (status, stdout) = run(&root, &["config", "set", "actions.draft.send.steps", steps]);
        assert_eq!(status, 1, "{stdout}");
        assert_eq!(parse_one(&stdout)["code"], "config_invalid");
    }
    let (status, stdout) = run(&root, &["remote", "test"]);
    if cfg!(feature = "cloudflare") {
        assert_eq!(status, 0, "{stdout}");
        let result = parse_one(&stdout);
        assert_eq!(result["smtp"]["status"], "skipped");
        assert_eq!(result["cloudflare_sending"]["status"], "not_tested");
        assert_eq!(result["cloudflare_sending"]["permissions_verified"], false);
        assert_eq!(result["cloudflare_sending"]["message_sent"], false);
    } else {
        assert_eq!(status, 1, "{stdout}");
        assert_eq!(parse_one(&stdout)["code"], "feature_disabled");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_mail_source_is_reported_explicitly() {
    let root = temp_root("no-mail-source");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(run(&root, &["init"]).0, 0);
    for args in [vec!["pull"], vec!["remote", "test"]] {
        let (status, output) = run(&root, &args);
        assert_eq!(status, 1, "{output}");
        assert_eq!(parse_one(&output)["code"], "config_missing");
    }
    let (status, output) = run(&root, &["doctor"]);
    assert_eq!(status, 3, "{output}");
    let doctor = parse_one(&output);
    assert_eq!(doctor["checks"]["mail_source_configured"], true);
    assert_eq!(doctor["issues"][0]["code"], "no_mail_source");
    let _ = fs::remove_dir_all(root);
}

#[cfg(feature = "cloudflare")]
#[test]
fn worker_scaffold_is_standalone_and_refuses_existing_content() {
    let root = temp_root("worker-scaffold");
    fs::create_dir_all(&root).unwrap();
    let (status, output) = run(&root, &["cloudflare", "scaffold", "worker"]);
    assert_eq!(status, 0, "{output}");
    // Exactly the tracked project: never an npm install, build output, or
    // local secret that happens to sit in the template directory.
    fn written(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                written(&path, root, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
                out.push(relative.replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    written(&root.join("worker"), &root.join("worker"), &mut files);
    files.sort();
    assert_eq!(
        files,
        [
            ".gitignore",
            "README.md",
            "cloudflare.config.ts",
            "package-lock.json",
            "package.json",
            "src/index.ts",
            "src/policy.ts",
            "src/quota.ts",
            "test/index.spec.ts",
            "tsconfig.json",
            "vitest.config.ts",
            "wrangler.jsonc",
        ]
    );
    let source = fs::read(root.join("worker/src/index.ts")).unwrap();
    let (status, output) = run(&root, &["cloudflare", "scaffold", "worker"]);
    assert_eq!(status, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "path_not_empty");
    assert_eq!(fs::read(root.join("worker/src/index.ts")).unwrap(), source);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cloudflare_config_validates_prefixes_and_redacts_credentials() {
    let root = temp_root("cloudflare-config");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(run(&root, &["init"]).0, 0);
    let (status, output) = run(&root, &["config", "set", "cloudflare.r2.bucket", "spool"]);
    assert_eq!(status, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "config_invalid");
    assert_eq!(
        run(
            &root,
            &[
                "config",
                "set",
                "cloudflare.account_id",
                "0123456789abcdef0123456789abcdef"
            ]
        )
        .0,
        0
    );
    assert_eq!(
        run(&root, &["config", "set", "cloudflare.r2.bucket", "spool"]).0,
        0
    );
    assert_eq!(
        run(
            &root,
            &[
                "config",
                "set",
                "cloudflare.r2.secret_access_key_secret",
                "private-test-key"
            ]
        )
        .0,
        0
    );
    for output in ["json", "yaml", "plain"] {
        let (status, text) = run(&root, &["config", "show", "--output", output]);
        assert_eq!(status, 0, "{text}");
        assert!(!text.contains("private-test-key"), "{text}");
    }
    let (status, output) = run(
        &root,
        &[
            "config",
            "set",
            "cloudflare.r2.imported_prefix",
            "inbox/imported/",
        ],
    );
    assert_eq!(status, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "config_invalid");
    let _ = fs::remove_dir_all(root);
}
