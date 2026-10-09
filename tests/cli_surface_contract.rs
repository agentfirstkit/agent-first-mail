use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, run_allow_stderr, temp_root};
use serde_json::Value;

fn protocol_events(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
        .collect()
}

#[cfg(any(not(feature = "api"), not(feature = "demo"), not(feature = "review")))]
fn assert_feature_disabled(root: &std::path::Path, args: &[&str], feature: &str) {
    let (status, stdout) = run(root, args);
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "feature_disabled", "{error}");
    assert_eq!(error["feature"], feature, "{error}");
    assert!(
        error["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains(&format!("--features {feature}"))),
        "{error}"
    );
}

#[test]
#[cfg(not(feature = "api"))]
fn api_command_reports_its_disabled_build_feature() {
    let root = temp_root("api-feature-disabled");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_feature_disabled(&root, &["api", "serve"], "api");
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(not(feature = "demo"))]
fn demo_command_reports_its_disabled_build_feature() {
    let root = temp_root("demo-feature-disabled");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_feature_disabled(&root, &["demo", "init"], "demo");
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(not(feature = "review"))]
fn review_command_reports_its_disabled_build_feature() {
    let root = temp_root("review-feature-disabled");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_feature_disabled(&root, &["triage", "review", "message_example"], "review");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn protocol_v1_streams_are_valid_and_have_one_terminal_event() {
    let root = temp_root("protocol-v1");
    assert!(fs::create_dir_all(&root).is_ok());

    let (status, stdout) = run(&root, &["init"]);
    assert_eq!(status, 0, "{stdout}");
    let events = protocol_events(&stdout);
    assert!(agent_first_data::validate_protocol_stream(&events, false).is_ok());
    assert_eq!(events[0]["kind"], "result");
    assert!(events[0]["result"].get("code").is_none());
    assert!(events[0]["trace"]["duration_ms"].as_u64().is_some());

    let (status, stdout) = run(
        &root,
        &[
            "status",
            "--output-to",
            "stdout",
            "--log",
            "startup",
            "--log",
            "request",
            "--log",
            "progress",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let events = protocol_events(&stdout);
    assert!(agent_first_data::validate_protocol_stream(&events, false).is_ok());
    assert_eq!(events.last().unwrap_or(&Value::Null)["kind"], "result");
    assert!(
        events
            .iter()
            .all(|event| event["trace"]["duration_ms"].as_u64().is_some())
    );

    // A rejected argv never reached a command, so it carries the rule it broke
    // in `error.code` and no timing for work that never started.
    let (status, stdout) = run(&root, &["unknown-command"]);
    assert_eq!(status, 2, "{stdout}");
    let events = protocol_events(&stdout);
    assert!(agent_first_data::validate_protocol_stream(&events, false).is_ok());
    assert_eq!(events[0]["kind"], "error");
    assert_eq!(events[0]["error"]["code"], "cli_unknown_command");
    assert!(events[0]["error"]["message"].as_str().is_some());
    assert!(events[0]["error"]["hint"].as_str().is_some());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn version_uses_afdata_output_formats() {
    let root = temp_root("version");
    assert!(fs::create_dir_all(&root).is_ok());

    // Bare `--version` is a protocol-v1 event by default now, not conventional
    // `<name> <version>` text — matches every other afmail command's default.
    let (status, stdout) = run(&root, &["--version"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["name"], "afmail");
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    let build = value["build"].as_str().map(str::to_owned);

    let (status, stdout) = run(&root, &["--help"]);
    assert_eq!(status, 0, "{stdout}");
    let help_result = parse_one(&stdout);
    assert_eq!(help_result["code"], "help");
    let help = &help_result["help"];
    assert_eq!(help["schema"], "cli-help-v2");
    assert_eq!(help["command_path"], "afmail");
    assert!(help.get("code").is_none());
    assert!(help.get("description").is_none());
    assert!(help.get("description_markdown").is_none());
    assert!(help.get("versions").is_none());
    // help-v1's `scope`/`inherited_arguments_from`/`arguments` are gone: a
    // command answers with every shape it has, and the root has none of its
    // own, only subcommands.
    assert!(help.get("scope").is_none());
    assert!(help.get("inherited_arguments_from").is_none());
    assert!(help.get("arguments").is_none());
    assert!(
        help["subcommands"]
            .as_array()
            .is_some_and(|commands| commands.iter().all(|command| command
                .as_str()
                .is_some_and(|command| command.ends_with(" --help")))),
        "{stdout}"
    );
    assert!(!stdout.contains(env!("CARGO_PKG_VERSION")));
    if let Some(build) = build {
        assert!(!stdout.contains(&build));
    }

    // No short flags exist anywhere in the registry.
    let (status, stdout) = run(&root, &["-V"]);
    assert_eq!(status, 2, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "cli_unknown_argument");

    let (status, stdout) = run(&root, &["--version", "--output", "json"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));

    let (status, stdout) = run(&root, &["--version", "--output", "yaml"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.starts_with("---\n"), "{stdout}");
    assert!(stdout.contains("kind: \"result\""), "{stdout}");
    assert!(
        stdout.contains(&format!("  version: \"{}\"", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line == format!("afmail {}", env!("CARGO_PKG_VERSION")))
    );

    let (status, stdout) = run(&root, &["--version", "--output", "plain"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.contains("kind=result"), "{stdout}");
    assert!(
        stdout.contains(&format!("result.version={}", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line == format!("afmail {}", env!("CARGO_PKG_VERSION")))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn output_setup_failure_keeps_the_shared_error_contract_without_running_the_command() {
    let root = temp_root("output-setup-failed");
    assert!(fs::create_dir_all(&root).is_ok());

    for args in [
        vec!["init", "--stdout-file", "."],
        vec!["init", "--stderr-file", "missing-parent/events.jsonl"],
    ] {
        let (status, stdout, stderr) = run_allow_stderr(&root, &args);
        assert_eq!(status, 1, "{args:?}: stdout={stdout} stderr={stderr}");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        let events = protocol_events(&stderr);
        assert_eq!(events.len(), 1, "{args:?}: {stderr}");
        assert!(agent_first_data::validate_protocol_stream(&events, true).is_ok());
        assert_eq!(events[0]["kind"], "error");
        assert_eq!(events[0]["error"]["code"], "output_setup_failed");
        assert_eq!(events[0]["error"]["retryable"], false);
        assert!(!root.join(".afmail").exists(), "init must not have run");
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn removed_flag_and_seen_commands_are_unknown_commands() {
    let root = temp_root("message-seen");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    for action in ["flag", "unflag", "seen", "unseen"] {
        let (status, stdout) = run(&root, &["message", action, "message_seen"]);
        assert_eq!(status, 2, "{stdout}");
        let value = parse_one(&stdout);
        assert_eq!(value["code"], "cli_unknown_command");
    }

    let (status, stdout) = run(&root, &["push", "flag"]);
    assert_eq!(status, 2, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_unknown_command");

    let _ = fs::remove_dir_all(root);
}

/// One `--help` call answers a command completely: every legal shape, with
/// every argument that shape accepts spelled into its usage line. There is no
/// second call to discover inherited globals, and no recursive mode.
#[test]
fn leaf_help_returns_complete_shapes() {
    let root = temp_root("help-shapes");
    assert!(fs::create_dir_all(&root).is_ok());

    let (status, stdout) = run(&root, &["message", "list", "--help"]);
    assert_eq!(status, 0, "{stdout}");
    let result = parse_one(&stdout);
    let help = &result["help"];
    assert_eq!(result["code"], "help");
    assert_eq!(help["schema"], "cli-help-v2");
    assert_eq!(help["command_path"], "afmail message list");
    assert!(help.get("inherited_arguments_from").is_none());
    let shapes = help["shapes"].as_array().cloned().unwrap_or_default();
    assert_eq!(shapes.len(), 1, "{stdout}");
    let usage = shapes[0]["usage"].as_str().unwrap_or_default();
    for argument in ["--sender", "--workspace", "--output", "--stdout-file"] {
        assert!(usage.contains(argument), "{usage}");
    }
    // A leaf has no children, so the key is simply absent.
    assert!(help.get("subcommands").is_none(), "{stdout}");

    // A command with more than one shape names each one, and says how it
    // differs from its siblings.
    let (status, stdout) = run(&root, &["push", "--help"]);
    assert_eq!(status, 0, "{stdout}");
    let shapes = parse_one(&stdout)["help"]["shapes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(shapes.len(), 2, "{stdout}");
    assert!(
        shapes.iter().all(|shape| shape["about"]
            .as_str()
            .is_some_and(|about| !about.is_empty())),
        "{stdout}"
    );

    let (status, plain) = run(&root, &["message", "list", "--help", "--output", "plain"]);
    assert_eq!(status, 0, "{plain}");
    assert!(plain.contains("afmail message list"), "{plain}");
    assert!(plain.contains("--workspace"), "{plain}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn help_pseudo_command_is_a_structured_cli_error() {
    let root = temp_root("help-pseudo");
    assert!(fs::create_dir_all(&root).is_ok());

    let (status, stdout) = run(&root, &["help"]);
    assert_eq!(status, 2, "{stdout}");
    let event = protocol_events(&stdout);
    assert_eq!(event.len(), 1, "{event:?}");
    assert_eq!(event[0]["kind"], "error");
    assert_eq!(event[0]["error"]["code"], "cli_unknown_command");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn handoff_subcommands_are_removed() {
    let root = temp_root("handoff-gone");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    assert_eq!(run(&root, &["push", "handoff"]).0, 2);
    assert_eq!(run(&root, &["case", "case-x", "handoff"]).0, 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_install_status_and_uninstall_work() {
    let root = temp_root("skill-cwd");
    let skills_dir = temp_root("skill-dir");
    assert!(fs::create_dir_all(&root).is_ok());
    let skills_dir_str = skills_dir.to_string_lossy().to_string();

    let (status, stdout) = run(
        &root,
        &[
            "skill",
            "status",
            "--agent",
            "codex",
            "--skills-dir",
            &skills_dir_str,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["skill"], "agent-first-mail");
    assert_eq!(value["installed_all"], false);

    let (status, stdout) = run(
        &root,
        &[
            "skill",
            "install",
            "--agent",
            "codex",
            "--skills-dir",
            &skills_dir_str,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let skill_path = skills_dir.join("agent-first-mail/SKILL.md");
    assert!(
        value["targets"][0]["skill_path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("agent-first-mail/SKILL.md")
    );
    assert!(skill_path.is_file());
    let text = fs::read_to_string(&skill_path).unwrap_or_default();
    assert!(text.contains("afmail-managed-skill: true"));
    assert!(text.contains("name: agent-first-mail"));

    let (status, stdout) = run(
        &root,
        &[
            "skill",
            "status",
            "--agent",
            "codex",
            "--skills-dir",
            &skills_dir_str,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["installed_all"], true);
    assert_eq!(value["valid_all"], true);
    assert_eq!(value["targets"][0]["managed"], true);

    let (status, stdout) = run(
        &root,
        &[
            "skill",
            "uninstall",
            "--agent",
            "codex",
            "--skills-dir",
            &skills_dir_str,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    assert!(!skill_path.exists());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(skills_dir);
}

#[test]
fn case_create_requires_explicit_group() {
    let root = temp_root("case-requires-group");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout, stderr) = run_allow_stderr(
        &root,
        &[
            "case",
            "create",
            "--name",
            "needs-category",
            "--message",
            "message_group_required",
            "--reason",
            "needs handling",
        ],
    );
    // `--group` is required by the only registered `case create` shape, so the
    // mix without it is simply not a shape.
    assert_eq!(status, 2, "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(
        parse_one(&stderr)["code"],
        "cli_unregistered_combination",
        "stdout: {stdout}\nstderr: {stderr}"
    );
    let (status, help) = run(&root, &["case", "create", "--help"]);
    assert_eq!(status, 0, "{help}");
    assert!(
        parse_one(&help)["help"]["shapes"][0]["usage"]
            .as_str()
            .is_some_and(|usage| usage.contains("--group <GROUP>")),
        "{help}"
    );

    let _ = fs::remove_dir_all(root);
}

/// A review or serve command reports an address the caller must act on, waits an
/// unbounded time, then reports its outcome. Both are caller-needed, so the
/// ordered stream must stay on one destination rather than splitting the first
/// output onto the diagnostic stream.
#[test]
#[cfg(all(feature = "demo", feature = "review"))]
fn event_stream_commands_keep_the_whole_stream_on_stdout() {
    let root = temp_root("event-stream-routing");
    assert!(fs::create_dir_all(&root).is_ok());

    // No workspace here, so every command fails the same way. What differs is
    // which stream carries the failure, and that is decided by the mode.
    let (_, stream_stdout, stream_stderr) = run_allow_stderr(&root, &["demo", "serve"]);
    assert!(
        stream_stdout.contains("workspace_not_found"),
        "an event-stream command must collapse onto stdout: stdout={stream_stdout} stderr={stream_stderr}"
    );
    assert!(
        stream_stderr.trim().is_empty(),
        "an event-stream command must not split across streams: {stream_stderr}"
    );

    let (_, finite_stdout, finite_stderr) = run_allow_stderr(&root, &["pull"]);
    assert!(
        finite_stderr.contains("workspace_not_found"),
        "a finite command keeps diagnostics on stderr: {finite_stderr}"
    );
    assert!(
        finite_stdout.trim().is_empty(),
        "a finite command must not write a failure to the result stream: {finite_stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(all(feature = "demo", feature = "api"))]
fn event_stream_commands_reject_split_routing() {
    let root = temp_root("event-stream-split-rejected");
    assert!(fs::create_dir_all(&root).is_ok());

    for args in [
        vec!["demo", "serve", "--output-to", "split"],
        vec!["api", "serve", "--output-to", "split"],
    ] {
        let (status, stdout, stderr) = run_allow_stderr(&root, &args);
        assert_eq!(status, 2, "{args:?}: stdout={stdout} stderr={stderr}");
        // The contract itself has no `split`, so the rejection names the
        // destinations that are registered.
        let error = parse_one(&stderr);
        assert_eq!(error["code"], "cli_invalid_argument_value", "{args:?}");
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains("expected one of stdout, stderr")),
            "{args:?}: {stderr}"
        );
    }

    let _ = fs::remove_dir_all(root);
}
