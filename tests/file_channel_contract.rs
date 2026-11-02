#![allow(clippy::unwrap_used)]

#[cfg_attr(not(feature = "files"), allow(dead_code))]
#[path = "support/cli.rs"]
mod cli;
#[cfg(feature = "files")]
mod enabled {
    use super::cli::{parse_one, read_json, run, temp_root, write_json};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn ok(root: &Path, args: &[&str]) -> Value {
        let (status, stdout) = run(root, args);
        assert_eq!(status, 0, "{args:?}: {stdout}");
        parse_one(&stdout)
    }
    fn fixture(name: &str) -> PathBuf {
        let root = temp_root(name);
        fs::create_dir_all(root.join("input")).unwrap();
        fs::create_dir_all(root.join("output")).unwrap();
        ok(&root, &["init"]);
        let sources = json!({"inquiries":{"inbox_path":"input","outbox_path":"output","max_message_bytes":4096,"max_input_files":20}}).to_string();
        ok(&root, &["config", "set", "files.sources", &sources]);
        write_json(
            &root.join("input/one.json"),
            &json!({
                "schema_name":"channel_message","schema_version":1,
                "external_message_id":"web/one","conversation_id":"inquiry/42",
                "sender_id":"visitor/42","recipient_ids":["support"],
                "subject":"Opening hours","body_text":"Are you open on Sunday?"
            }),
        );
        root
    }
    fn queued_reply(root: &Path) -> (String, String, PathBuf, Value) {
        let pulled = ok(root, &["pull", "--files"]);
        let message_id = pulled["new_message_ids"][0].as_str().unwrap();
        let case = ok(
            root,
            &[
                "case",
                "create",
                "--name",
                "Opening hours",
                "--group",
                "support",
                "--message",
                message_id,
                "--reason",
                "answer inquiry",
            ],
        );
        let uid = case["case_uid"].as_str().unwrap().to_string();
        let case_path = root.join(case["case_path"].as_str().unwrap());
        let draft = ok(
            root,
            &[
                "case",
                "draft",
                "reply",
                &uid,
                message_id,
                "--body",
                "Yes, from 10 to 4.",
            ],
        );
        let name = draft["draft_name"].as_str().unwrap().to_string();
        ok(root, &["case", "draft", "send", &uid, &name]);
        let plan = ok(root, &["push"]);
        (uid, name, case_path, plan)
    }

    #[test]
    fn file_pulls_do_not_require_default_imap_mailboxes() {
        for explicit in [false, true] {
            let root = fixture("files-without-imap-defaults");
            let path = root.join(".afmail/config.json");
            let mut config = read_json(&path);
            config["actions"]["pull"]["default_mailbox_ids"] = json!([]);
            write_json(&path, &config);
            if explicit {
                // Selecting files must also bypass an independently configured IMAP source.
                ok(&root, &["config", "set", "imap.host", "imap.example.com"]);
            }
            let args = if explicit {
                vec!["pull", "--files"]
            } else {
                vec!["pull"]
            };
            let mut preview_args = args.clone();
            preview_args.push("--dry-run");
            assert_eq!(ok(&root, &preview_args)["candidate_count"], 1);
            let pulled = ok(&root, &args);
            assert_eq!(pulled["new_message_count"], 1);
            assert_eq!(pulled["new_message_ids"].as_array().unwrap().len(), 1);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn unrelated_broken_evidence_does_not_block_reads_and_incomplete_checks_cannot_file_mail() {
        let root = fixture("native-relation-isolation");
        let healthy = ok(&root, &["pull", "--files"])["new_message_ids"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let mut unrelated = read_json(&root.join("input/one.json"));
        unrelated["external_message_id"] = json!("web/unrelated");
        unrelated["conversation_id"] = json!("inquiry/unrelated");
        write_json(&root.join("input/unrelated.json"), &unrelated);
        let broken = ok(&root, &["pull", "--files"])["new_message_ids"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let evidence_path = root.join(format!(".afmail/messages/{broken}.message"));
        let evidence = fs::read(&evidence_path).unwrap();
        fs::remove_file(&evidence_path).unwrap();
        // A current cache is sufficient for indexing; missing unrelated raw
        // evidence must not be opened while showing this healthy message.
        let shown = ok(&root, &["message", "show", &healthy]);
        assert!(shown["context"]["related_message_issues"].is_null());
        fs::write(&evidence_path, evidence).unwrap();
        // An invalid unrelated cache is surfaced rather than hiding the
        // failure or making unrelated mail unreadable, and it blocks filing.
        let cache_path = root.join(format!("messages/{broken}.json"));
        let cache = read_json(&cache_path);
        let mut invalid = cache.clone();
        invalid["workspace"]["status"] = json!("bogus");
        write_json(&cache_path, &invalid);
        let cache_before = fs::read(&cache_path).unwrap();
        let shown = ok(&root, &["message", "show", &healthy]);
        assert_eq!(
            shown["context"]["related_message_issues"][0]["message_id"],
            broken
        );
        assert!(
            shown["context"]["related_message_issues"][0]["path"]
                .as_str()
                .unwrap()
                .ends_with(&format!("{broken}.json"))
        );
        let (code, output) = run(
            &root,
            &["message", "trash", &healthy, "--reason", "discard"],
        );
        assert_eq!(code, 1, "{output}");
        assert_eq!(
            parse_one(&output)["code"],
            "message_relation_index_incomplete"
        );
        assert_eq!(fs::read(&cache_path).unwrap(), cache_before);
        // A malformed unrelated cache is likewise surfaced.
        fs::write(&cache_path, b"{").unwrap();
        let shown = ok(&root, &["message", "show", &healthy]);
        assert_eq!(
            shown["context"]["related_message_issues"][0]["message_id"],
            broken
        );
        write_json(&cache_path, &cache);
        let shown = ok(&root, &["message", "show", &healthy]);
        assert!(shown["context"]["related_message_issues"].is_null());
        ok(
            &root,
            &["message", "trash", &healthy, "--reason", "discard"],
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_reply_relations_are_scoped_and_render_in_every_language() {
        let root = fixture("native-reply-relations");
        let parent = ok(&root, &["pull", "--files"])["new_message_ids"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let mut reply = read_json(&root.join("input/one.json"));
        reply["external_message_id"] = json!("web/two");
        reply["reply_to_external_message_id"] = json!("web/one");
        write_json(&root.join("input/two.json"), &reply);
        let child = ok(&root, &["pull", "--files"])["new_message_ids"][0]
            .as_str()
            .unwrap()
            .to_owned();

        // Neither another conversation nor another configured source may bind
        // the same external reply id to this parent.
        reply["conversation_id"] = json!("inquiry/other");
        reply["external_message_id"] = json!("web/three");
        write_json(&root.join("input/other.json"), &reply);
        fs::create_dir_all(root.join("other-input")).unwrap();
        reply["conversation_id"] = json!("inquiry/42");
        reply["external_message_id"] = json!("web/two");
        write_json(&root.join("other-input/reply.json"), &reply);
        ok(
            &root,
            &[
                "config",
                "set",
                "files.sources",
                r#"{"inquiries":{"inbox_path":"input","outbox_path":"output"},"other":{"inbox_path":"other-input","outbox_path":"other-output"}}"#,
            ],
        );
        let additional = ok(&root, &["pull", "--files"]);
        assert_eq!(additional["new_message_count"], 2, "{additional}");
        for id in additional["new_message_ids"].as_array().unwrap() {
            let shown = ok(&root, &["message", "show", id.as_str().unwrap()]);
            assert!(
                shown["context"]["related_messages"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
            );
        }

        let cache = root.join(format!("messages/{child}.json"));
        let cache_bytes = fs::read(&cache).unwrap();
        for (id, related) in [(&parent, &child), (&child, &parent)] {
            let shown = ok(&root, &["message", "show", id]);
            let links = shown["context"]["related_messages"].as_array().unwrap();
            assert_eq!(links.len(), 1, "{shown}");
            assert_eq!(links[0]["message"]["message_id"], *related);
        }
        assert_eq!(
            fs::read(&cache).unwrap(),
            cache_bytes,
            "show must stay read-only"
        );
        let details = ok(&root, &["triage", "list", "--details"]);
        let parent_row = details["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["message_id"] == parent)
            .unwrap();
        assert_eq!(parent_row["related_messages"].as_array().unwrap().len(), 1);
        for language in agent_first_mail::config::TemplateLanguage::ALL {
            ok(
                &root,
                &[
                    "config",
                    "set",
                    "workspace.language_bcp47",
                    language.as_str(),
                ],
            );
            ok(&root, &["render", "refresh"]);
            let shown = ok(&root, &["message", "show", &child]);
            assert_eq!(
                shown["context"]["message"]["message"]["origin"]["reply_to_external_message_id"],
                "web/one"
            );
            let view = root.join(shown["locator"]["view_path"].as_str().unwrap());
            assert!(fs::read_to_string(view).unwrap().contains(&parent));
        }
        assert_eq!(
            read_json(&cache)["origin"]["reply_to_external_message_id"],
            "web/one"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_input_reply_and_output_reuse_case_and_confirmed_push() {
        let root = fixture("native-channel-workflow");
        let input = fs::read(root.join("input/one.json")).unwrap();
        assert_eq!(
            ok(&root, &["pull", "--files", "--dry-run"])["candidate_count"],
            1
        );
        assert_eq!(fs::read_dir(root.join("messages")).unwrap().count(), 0);
        let (uid, name, case_path, plan) = queued_reply(&root);
        assert_eq!(plan["items"][0]["actions"], json!(["file_send"]));
        assert_eq!(
            plan["items"][0]["outbound"]["reply_route"]["source_id"],
            "inquiries"
        );
        assert!(plan["items"][0]["outbound"]["mime_hash"].is_null());
        assert!(plan["items"][0]["outbound"]["content_hash"].is_string());
        assert_eq!(fs::read_dir(root.join("output")).unwrap().count(), 0);
        let id = plan["plan_id"].as_str().unwrap();
        ok(&root, &["push", "--confirm", id]);
        let outputs = fs::read_dir(root.join("output"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(outputs.len(), 1);
        let output = read_json(&outputs[0].path());
        assert_eq!(output["schema_name"], "channel_message");
        assert_eq!(output["recipient_ids"], json!(["visitor/42"]));
        assert_eq!(output["conversation_id"], "inquiry/42");
        assert_eq!(output["reply_to_external_message_id"], "web/one");
        assert!(
            output["body_text"]
                .as_str()
                .unwrap()
                .starts_with("Yes, from 10 to 4.")
        );
        assert!(!case_path.join("drafts").join(name).exists());
        assert_eq!(ok(&root, &["pull", "--files"])["new_message_count"], 0);
        assert_eq!(fs::read(root.join("input/one.json")).unwrap(), input);
        assert_eq!(
            fs::read_dir(root.join(".afmail/messages"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "eml"))
                .count(),
            0
        );
        // Generated caches can be discarded: native JSON remains canonical.
        for entry in fs::read_dir(root.join("messages")).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        ok(&root, &["render", "refresh"]);
        let document = ok(&root, &["case", "show", &uid]);
        assert!(document.to_string().contains("inquiry/42"));
        let doctor = ok(&root, &["doctor"]);
        assert!(
            !doctor.to_string().contains("message_eml_missing"),
            "{doctor}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn conflicting_input_and_payload_paths_never_overwrite_or_redirect() {
        let root = fixture("native-channel-conflicts");
        let pulled = ok(&root, &["pull", "--files"]);
        let id = pulled["new_message_ids"][0].as_str().unwrap();
        let mut changed = read_json(&root.join("input/one.json"));
        changed["body_text"] = json!("different body");
        write_json(&root.join("input/two.json"), &changed);
        changed["external_message_id"] = json!("web/two");
        changed["outbox_path"] = json!("../redirect");
        write_json(&root.join("input/path.json"), &changed);
        let result = ok(&root, &["pull", "--files"]);
        assert_eq!(result["new_message_count"], 0);
        assert_eq!(result["files"]["warnings"].as_array().unwrap().len(), 2);
        let document = ok(&root, &["message", "show", id]);
        assert!(document.to_string().contains("Are you open on Sunday?"));
        assert!(!root.join("redirect").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn output_configuration_changes_invalidate_the_reviewed_plan() {
        let root = fixture("native-channel-plan-target");
        let (_, _, _, plan) = queued_reply(&root);
        ok(
            &root,
            &[
                "config",
                "set",
                "files.sources.inquiries.outbox_path",
                "changed-output",
            ],
        );
        let (status, stdout) = run(
            &root,
            &["push", "--confirm", plan["plan_id"].as_str().unwrap()],
        );
        assert_ne!(status, 0);
        assert!(stdout.contains("push_plan_changed"), "{stdout}");
        assert_eq!(fs::read_dir(root.join("output")).unwrap().count(), 0);
        assert!(!root.join("changed-output").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pull_budgets_apply_to_native_records_and_repeated_inputs() {
        let root = fixture("native-channel-budgets");
        let mut second = read_json(&root.join("input/one.json"));
        second["external_message_id"] = json!("web/two");
        write_json(&root.join("input/two.json"), &second);
        let bounded = ok(&root, &["pull", "--files", "--max-total-bytes", "1"]);
        assert_eq!(bounded["new_message_count"], 0);
        assert_eq!(bounded["budget_limited"], true);
        let first = ok(&root, &["pull", "--files", "--max-new-messages", "1"]);
        assert_eq!(first["new_message_count"], 1);
        // The producer retires inputs from the result, so the input left behind
        // by the budget is named with its reason.
        let skipped = &first["files"]["skipped_inputs"];
        assert_eq!(first["files"]["skipped_input_count"], 1, "{first}");
        assert_eq!(skipped[0]["source_id"], "inquiries");
        assert_eq!(skipped[0]["reason"], "message_count_budget_exceeded");
        assert!(
            skipped[0]["input_path"]
                .as_str()
                .unwrap()
                .ends_with("two.json"),
            "{first}"
        );
        assert_eq!(
            fs::metadata(root.join("input/two.json")).unwrap().len(),
            skipped[0]["size_bytes"].as_u64().unwrap()
        );
        let second = ok(&root, &["pull", "--files", "--max-new-messages", "1"]);
        assert_eq!(second["new_message_count"], 1);
        assert_eq!(second["files"]["merged_count"], 1);
        assert_eq!(second["files"]["skipped_inputs"], json!([]));
        let id = first["new_message_ids"][0].as_str().unwrap();
        let document = ok(&root, &["message", "show", id]);
        assert_eq!(
            document["context"]["message"]["content_provenance"]["source"],
            "file_message"
        );
        assert_eq!(
            document["context"]["message"]["analysis"]["security"]["authentication_warning"],
            false
        );
        assert!(document["context"]["message"]["message"]["eml_path"].is_null());
        let view = root.join(document["locator"]["view_path"].as_str().unwrap());
        assert!(fs::read_to_string(view).unwrap().contains("inquiry/42"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uncertain_publication_recovers_only_from_retained_identical_output() {
        for retain in [true, false] {
            let root = fixture("native-channel-recovery");
            let (uid, name, case_path, plan) = queued_reply(&root);
            let push_id = plan["items"][0]["push_id"].as_str().unwrap();
            let path = root.join(".afmail/push").join(format!("{push_id}.json"));
            let mut item = read_json(&path);
            let message_id = agent_first_mail::outbound::message_id_for_push(push_id);
            let prepared = agent_first_mail::outbound::prepare_outbound(
                &root,
                &case_path,
                &uid,
                &name,
                &agent_first_mail::config::MailConfig::load(&root).unwrap(),
                Some(&message_id),
                item["created_rfc3339"].as_str(),
            )
            .unwrap();
            let envelope: Value = serde_json::from_slice(&prepared.raw).unwrap();
            if retain {
                fs::write(
                    root.join("output").join(format!("{message_id}.json")),
                    envelope["content_json"].as_str().unwrap(),
                )
                .unwrap();
            }
            item["step_states"] = json!([{"index":0,"label":"file_send","status":"remote_outcome_unknown","started_rfc3339":"2026-10-22T00:00:00Z"}]);
            write_json(&path, &item);
            let preview = ok(&root, &["push"]);
            let (status, stdout) = run(
                &root,
                &["push", "--confirm", preview["plan_id"].as_str().unwrap()],
            );
            if retain {
                assert_eq!(status, 0, "{stdout}");
            } else {
                assert_ne!(status, 0);
                assert!(stdout.contains("outcome_unknown"));
                assert_eq!(fs::read_dir(root.join("output")).unwrap().count(), 0);
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn one_unusable_source_is_a_warning_and_other_sources_still_import() {
        let root = fixture("native-channel-source-isolation");
        let sources = json!({
            "absent":{"inbox_path":"not-created","outbox_path":"absent-output"},
            "inquiries":{"inbox_path":"input","outbox_path":"output","max_message_bytes":4096,"max_input_files":20},
            "zz_crowded":{"inbox_path":"crowded","outbox_path":"crowded-output","max_input_files":1}
        })
        .to_string();
        ok(&root, &["config", "set", "files.sources", &sources]);
        fs::create_dir_all(root.join("crowded")).unwrap();
        for name in ["a.json", "b.json"] {
            fs::write(root.join("crowded").join(name), "{}").unwrap();
        }
        let result = ok(&root, &["pull", "--files"]);
        assert_eq!(result["new_message_count"], 1, "{result}");
        let warnings = result["files"]["warnings"].as_array().unwrap();
        let codes: Vec<_> = warnings
            .iter()
            .map(|warning| (warning["source_id"].clone(), warning["code"].clone()))
            .collect();
        assert_eq!(
            codes,
            vec![
                (json!("absent"), json!("channel_inbox_unavailable")),
                (json!("zz_crowded"), json!("channel_inbox_limit")),
            ],
            "{result}"
        );
        // A default pull with only files configured succeeds the same way.
        assert_eq!(ok(&root, &["pull"])["files"]["merged_count"], 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn channel_directories_must_not_overlap_workspace_state_or_inboxes() {
        let root = fixture("native-channel-path-overlap");
        for (inbox, outbox) in [
            ("input", "messages"),
            ("input", "./.afmail/push/"),
            ("cases/support", "output"),
            ("input", "notifications/x/.."),
            ("input", "."),
            ("input", "input/"),
            ("input", "./x/../input"),
        ] {
            let sources =
                json!({"inquiries":{"inbox_path":inbox,"outbox_path":outbox}}).to_string();
            let (status, stdout) = run(&root, &["config", "set", "files.sources", &sources]);
            assert_ne!(status, 0, "{inbox} -> {outbox}: {stdout}");
            assert!(stdout.contains("config_invalid"), "{stdout}");
        }
        let sources = json!({
            "a":{"inbox_path":"input","outbox_path":"output"},
            "b":{"inbox_path":"output/","outbox_path":"b-output"}
        })
        .to_string();
        let (status, stdout) = run(&root, &["config", "set", "files.sources", &sources]);
        assert_ne!(status, 0);
        assert!(stdout.contains("config_invalid"), "{stdout}");
        // Identity is per source, so two sources sharing one inbox would import
        // every record twice.
        let sources = json!({
            "a":{"inbox_path":"input","outbox_path":"output"},
            "b":{"inbox_path":"./input/","outbox_path":"b-output"}
        })
        .to_string();
        let (status, stdout) = run(&root, &["config", "set", "files.sources", &sources]);
        assert_ne!(status, 0);
        assert_eq!(parse_one(&stdout)["code"], "config_invalid", "{stdout}");
        assert!(stdout.contains("inbox must differ"), "{stdout}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aliased_spellings_resolve_against_the_workspace_before_use() {
        let root = fixture("native-channel-path-alias");
        let canonical = fs::canonicalize(&root).unwrap();
        let config_path = root.join(".afmail/config.json");
        let mut config = read_json(&config_path);
        // Absolute spellings bypass the root-free config check; pull resolves them.
        config["files"]["sources"]["inquiries"]["outbox_path"] =
            json!(canonical.join(".afmail/cache").to_string_lossy());
        config["files"]["sources"]["other"] = json!({"inbox_path":"elsewhere","outbox_path":canonical.join("input").to_string_lossy()});
        write_json(&config_path, &config);
        let result = ok(&root, &["pull", "--files"]);
        assert_eq!(result["new_message_count"], 0, "{result}");
        let warnings = result["files"]["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 2, "{result}");
        assert!(
            warnings
                .iter()
                .all(|warning| warning["code"] == "config_invalid"),
            "{result}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn input_symlinks_and_actual_size_are_rejected() {
        let root = fixture("native-channel-boundaries");
        std::os::unix::fs::symlink(root.join("input/one.json"), root.join("input/link.json"))
            .unwrap();
        fs::write(root.join("input/large.json"), vec![b' '; 4097]).unwrap();
        let result = ok(&root, &["pull", "--files"]);
        assert_eq!(result["new_message_count"], 1);
        assert_eq!(result["files"]["warnings"].as_array().unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(not(feature = "files"))]
#[test]
fn explicit_file_pull_reports_disabled_feature() {
    let root = cli::temp_root("files-disabled");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(cli::run(&root, &["init"]).0, 0);
    let source = r#"{"sample":{"inbox_path":"input","outbox_path":"output"}}"#;
    assert_eq!(
        cli::run(&root, &["config", "set", "files.sources", source]).0,
        0
    );
    let (status, stdout) = cli::run(&root, &["pull", "--files"]);
    assert_ne!(status, 0);
    assert!(stdout.contains("feature_disabled"), "{stdout}");
    std::fs::remove_dir_all(root).unwrap();
}
