use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, read_json, run, temp_root};

#[test]
fn pull_failure_writes_workspace_progress_snapshot() {
    let root = temp_root("pull-progress-failure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "config_missing");
    let progress = read_json(&root.join(".afmail/cache/progress.json"));
    assert_eq!(progress["command"], "pull");
    assert_eq!(progress["status"], "failed");
    assert!(progress["error"]["code"].as_str().is_some(), "{progress}");
    assert!(
        progress["error"]["message"].as_str().is_some(),
        "{progress}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_rejects_unknown_ids_before_network_config() {
    let root = temp_root("pull-unknown-id");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(&root, &["pull", "INBOX"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "unknown_mailbox_id");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|message| message.contains("available ids:"))
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(feature = "cloudflare")]
#[path = "support/fake_s3.rs"]
mod fake_s3;

#[cfg(feature = "cloudflare")]
mod cloudflare {
    use super::*;
    use serde_json::json;
    const RAW: &[u8] = b"Message-ID: <same@example.net>\r\nFrom: sender@example.net\r\nTo: visible@example.net\r\nSubject: Mail\r\n\r\nHello\r\n";
    #[test]
    fn quota_managed_cleanup_requires_control_and_retries_without_duplicate_import() {
        for archive in [None, Some("imported/")] {
            let server = fake_s3::FakeS3::new();
            server.state.lock().unwrap().quota_managed = true;
            server.put("inbox/001.eml", RAW, "hidden@example.com");
            let root = workspace(&server, "r2-quota-control", archive);
            let (status, output) = run(&root, &["pull", "--cloudflare"]);
            assert_eq!(status, 0, "{output}");
            assert_eq!(
                parse_one(&output)["cloudflare"]["warnings"][0]["code"],
                "config_missing"
            );
            assert!(
                server
                    .state
                    .lock()
                    .unwrap()
                    .objects
                    .contains_key("inbox/001.eml")
            );
            let path = root.join(".afmail/config.json");
            let mut config = cli::read_json(&path);
            config["cloudflare"]["spool"] =
                json!({"control_url":server.endpoint,"token_secret":"q".repeat(32)});
            cli::write_json(&path, &config);
            server.state.lock().unwrap().control_failures = 1;
            let (status, output) = run(&root, &["pull", "--cloudflare"]);
            assert_eq!(status, 0, "{output}");
            assert_eq!(parse_one(&output)["cloudflare"]["merged_count"], 1);
            assert_eq!(
                parse_one(&output)["cloudflare"]["warnings"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            let (status, output) = run(&root, &["pull", "--cloudflare"]);
            assert_eq!(status, 0, "{output}");
            assert_eq!(parse_one(&output)["cloudflare"]["merged_count"], 1);
            let state = server.state.lock().unwrap();
            assert!(!state.objects.contains_key("inbox/001.eml"));
            assert_eq!(
                state.objects.contains_key("imported/001.eml"),
                archive.is_some()
            );
            assert!(
                !state
                    .requests
                    .iter()
                    .any(|(method, _)| method == "DELETE" || method == "PUT")
            );
        }
    }
    #[allow(clippy::unwrap_used, reason = "test workspace fixture")]
    fn workspace(
        server: &fake_s3::FakeS3,
        name: &str,
        imported_prefix: Option<&str>,
    ) -> std::path::PathBuf {
        let root = temp_root(name);
        fs::create_dir_all(&root).unwrap();
        assert_eq!(run(&root, &["init"]).0, 0);
        cli::write_json(
            &root.join(".afmail/config.json"),
            &json!({"cloudflare":{"r2":{"endpoint_url":server.endpoint,"bucket":"spool","access_key_id":"test-access","secret_access_key_secret":"test-secret","imported_prefix":imported_prefix}},"identities":[{"identity":"me","name":"Me","email":"hidden@example.com","default":true}]}),
        );
        root
    }
    #[test]
    fn cloudflare_pulls_do_not_require_default_imap_mailboxes() {
        for explicit in [false, true] {
            let server = fake_s3::FakeS3::new();
            server.put("inbox/001.eml", RAW, "hidden@example.com");
            let root = workspace(&server, "r2-without-imap-defaults", None);
            let path = root.join(".afmail/config.json");
            let mut config = cli::read_json(&path);
            config["actions"] = json!({"pull":{"default_mailbox_ids":[]}});
            if explicit {
                config["imap"] = json!({"host":"imap.example.com"});
            }
            cli::write_json(&path, &config);
            let args = if explicit {
                vec!["pull", "--cloudflare"]
            } else {
                vec!["pull"]
            };
            let mut preview_args = args.clone();
            preview_args.push("--dry-run");
            let (status, output) = run(&root, &preview_args);
            assert_eq!(status, 0, "{output}");
            assert_eq!(parse_one(&output)["candidate_count"], 1);
            let (status, output) = run(&root, &args);
            assert_eq!(status, 0, "{output}");
            assert_eq!(parse_one(&output)["new_message_count"], 1);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn cloudflare_hints_shared_message_id_without_merging_distinct_bytes() {
        let server = fake_s3::FakeS3::new();
        let second = [b"Received: by another-relay\r\n".as_slice(), RAW].concat();
        server.put("inbox/001.eml", RAW, "hidden@example.com");
        server.put("inbox/002.eml", &second, "alias@example.com");
        let root = workspace(&server, "r2-duplicate-candidate", None);
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        let result = parse_one(&output);
        assert_eq!(result["new_message_count"], 2);
        assert_eq!(result["cloudflare"]["merged_count"], 0);
        let ids = result["new_message_ids"].as_array().unwrap();
        let warning = &result["cloudflare"]["warnings"][0];
        assert_eq!(warning["code"], "duplicate_candidate");
        assert_eq!(warning["message_id"], ids[1]);
        assert_eq!(warning["candidate_message_ids"], json!([ids[0]]));
        assert_eq!(warning["evidence"], "same_rfc822_message_id_distinct_raw");
        for (id, raw) in [
            (ids[0].as_str().unwrap(), RAW),
            (ids[1].as_str().unwrap(), second.as_slice()),
        ] {
            assert_eq!(
                fs::read(root.join(format!(".afmail/messages/{id}.eml"))).unwrap(),
                raw
            );
        }
        // Rebuilding the index on a later invocation retains both candidates;
        // an exact byte replay still merges without a weak duplicate warning.
        server.put("inbox/003.eml", &second, "third@example.com");
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        let replay = parse_one(&output);
        assert_eq!(replay["new_message_count"], 0);
        assert_eq!(replay["cloudflare"]["merged_count"], 1);
        assert_eq!(replay["cloudflare"]["warnings"], json!([]));
        let third = [b"Received: by third-relay\r\n".as_slice(), RAW].concat();
        server.put("inbox/004.eml", &third, "fourth@example.com");
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        let candidates =
            parse_one(&output)["cloudflare"]["warnings"][0]["candidate_message_ids"].clone();
        assert_eq!(candidates.as_array().unwrap().len(), 2);
        assert!(
            ids.iter()
                .all(|id| candidates.as_array().unwrap().contains(id))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cloudflare_paginates_deduplicates_and_retries_ack() {
        let server = fake_s3::FakeS3::new();
        server.put("inbox/001.eml", RAW, "hidden@example.com");
        server.put("inbox/002.eml", RAW, "alias@example.com");
        server.state.lock().unwrap().delete_failures = 1;
        let root = workspace(&server, "r2-retry", None);
        let (status, output) = run(&root, &["remote", "test"]);
        assert_eq!(status, 0, "{output}");
        let tested = parse_one(&output);
        assert_eq!(tested["imap"]["status"], "skipped");
        assert_eq!(tested["cloudflare"]["status"], "succeeded");
        let (status, output) = run(&root, &["pull"]);
        assert_eq!(status, 0, "{output}");
        let result = parse_one(&output);
        assert_eq!(result["new_message_count"], 1);
        assert_eq!(result["cloudflare"]["merged_count"], 1);
        assert_eq!(
            result["cloudflare"]["warnings"].as_array().unwrap().len(),
            1
        );
        let id = result["new_message_ids"][0].as_str().unwrap();
        assert_eq!(
            fs::read(root.join(format!(".afmail/messages/{id}.eml"))).unwrap(),
            RAW
        );
        let metadata = cli::read_json(&root.join(format!(".afmail/messages/{id}.json")));
        assert_eq!(
            metadata["delivery"]["envelope_to"],
            json!(["hidden@example.com", "alias@example.com"])
        );
        assert_eq!(metadata["delivery"]["sources"].as_array().unwrap().len(), 2);
        assert!(metadata.get("remote").is_none());
        let message = cli::read_json(&root.join(format!("messages/{id}.json")));
        assert_eq!(message["identity_email"], "hidden@example.com");
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        assert_eq!(parse_one(&output)["cloudflare"]["merged_count"], 1);
        assert!(server.state.lock().unwrap().objects.is_empty());
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        assert_eq!(parse_one(&output)["cloudflare"]["merged_count"], 0);
        let (status, output) = run(&root, &["doctor"]);
        assert_eq!(status, 0, "{output}");
        assert_eq!(
            run(&root, &["message", "spam", id, "--reason", "test"]).0,
            0
        );
        assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn interrupted_pull_replays_only_acknowledgement() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let server = fake_s3::FakeS3::new();
        server.put("inbox/interrupted.eml", RAW, "hidden@example.com");
        server.state.lock().unwrap().pause_delete = true;
        let root = workspace(&server, "r2-interrupted", None);
        let mut child = Command::new(env!("CARGO_BIN_EXE_afmail"))
            .args(["pull", "--cloudflare"])
            .current_dir(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut delete_started = false;
        while Instant::now() < deadline {
            delete_started = server
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|(method, _)| method == "DELETE");
            if delete_started {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        {
            let mut state = server.state.lock().unwrap();
            state.delete_failures = 1;
            state.pause_delete = false;
        }
        assert!(delete_started, "pull did not reach acknowledgement");
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        let result = parse_one(&output);
        assert_eq!(result["new_message_count"], 0);
        assert_eq!(result["cloudflare"]["merged_count"], 1);
        assert!(server.state.lock().unwrap().objects.is_empty());
        assert_eq!(
            fs::read_dir(root.join(".afmail/messages"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "eml"))
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cloudflare_dry_run_budgets_oversize_and_archive_copy() {
        let server = fake_s3::FakeS3::new();
        server.put("inbox/a b+.eml", RAW, "hidden@example.com");
        server.put("inbox/large.eml", RAW, "hidden@example.com");
        server
            .state
            .lock()
            .unwrap()
            .objects
            .get_mut("inbox/large.eml")
            .unwrap()
            .size_bytes = 101 * 1024 * 1024;
        let root = workspace(&server, "r2-copy", Some("imported/"));
        let (status, output) = run(&root, &["pull", "--cloudflare", "--dry-run"]);
        assert_eq!(status, 0, "{output}");
        let result = parse_one(&output);
        assert_eq!(result["candidate_count"], 1);
        assert_eq!(
            result["cloudflare"]["skipped_objects"][0]["reason"],
            "message_too_large"
        );
        assert!(
            server
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|(_, key)| key.is_empty())
        );
        let (status, output) = run(&root, &["pull", "--cloudflare", "--max-new-messages", "0"]);
        assert_eq!(status, 0, "{output}");
        assert_eq!(parse_one(&output)["new_message_count"], 0);
        server.state.lock().unwrap().copy_failure = true;
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        assert_eq!(parse_one(&output)["cloudflare"]["moved_object_count"], 0);
        assert!(
            server
                .state
                .lock()
                .unwrap()
                .objects
                .contains_key("inbox/a b+.eml")
        );
        server.state.lock().unwrap().copy_failure = false;
        let (status, output) = run(&root, &["pull", "--cloudflare"]);
        assert_eq!(status, 0, "{output}");
        assert_eq!(parse_one(&output)["cloudflare"]["moved_object_count"], 1);
        let state = server.state.lock().unwrap();
        assert!(state.objects.contains_key("imported/a b+.eml"));
        assert!(!state.objects.contains_key("inbox/a b+.eml"));
        assert!(state.objects.contains_key("inbox/large.eml"));
        drop(state);
        // Time windows apply only to IMAP; the parser refuses the mix.
        for args in [
            &["pull", "--cloudflare", "--since-days", "3"][..],
            &["pull", "--files", "--all-history"][..],
            &["pull", "--cloudflare", "--files", "--since-days", "3"][..],
        ] {
            let (status, output) = run(&root, args);
            assert_eq!(status, 2, "{args:?}: {output}");
            assert_eq!(
                parse_one(&output)["code"],
                "cli_unregistered_combination",
                "{args:?}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(not(feature = "cloudflare"))]
#[test]
fn cloudflare_commands_report_disabled_feature() {
    let root = temp_root("cloudflare-disabled");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(run(&root, &["init"]).0, 0);
    for args in [
        vec!["pull", "--cloudflare"],
        vec!["cloudflare", "scaffold", "worker"],
        vec!["cloudflare", "consume"],
        vec!["cloudflare", "watch"],
    ] {
        let (status, output) = run(&root, &args);
        assert_eq!(status, 1, "{output}");
        assert_eq!(parse_one(&output)["code"], "feature_disabled");
    }
    let _ = fs::remove_dir_all(root);
}
