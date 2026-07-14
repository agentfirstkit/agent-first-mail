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

    let (status, stdout) = run(&root, &["--log", "startup,request,progress", "status"]);
    assert_eq!(status, 0, "{stdout}");
    let events = protocol_events(&stdout);
    assert!(agent_first_data::validate_protocol_stream(&events, false).is_ok());
    assert_eq!(events.last().unwrap_or(&Value::Null)["kind"], "result");
    assert!(
        events
            .iter()
            .all(|event| event["trace"]["duration_ms"].as_u64().is_some())
    );

    let (status, stdout) = run(&root, &["unknown-command"]);
    assert_eq!(status, 2, "{stdout}");
    let events = protocol_events(&stdout);
    assert!(agent_first_data::validate_protocol_stream(&events, false).is_ok());
    assert_eq!(events[0]["kind"], "error");
    assert_eq!(events[0]["error"]["code"], "cli_error");
    assert!(events[0]["error"]["message"].as_str().is_some());
    assert!(events[0]["trace"]["duration_ms"].as_u64().is_some());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn version_uses_afdata_output_formats() {
    let root = temp_root("version");
    assert!(fs::create_dir_all(&root).is_ok());

    let (status, stdout) = run(&root, &["--version"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        stdout.trim(),
        format!("afmail {}", env!("CARGO_PKG_VERSION"))
    );

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
fn removed_flag_and_seen_commands_are_cli_errors() {
    let root = temp_root("message-seen");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    for action in ["flag", "unflag", "seen", "unseen"] {
        let (status, stdout) = run(&root, &["message", action, "message_seen"]);
        assert_eq!(status, 2, "{stdout}");
        let value = parse_one(&stdout);
        assert_eq!(value["code"], "cli_error");
    }

    let (status, stdout) = run(&root, &["push", "flag"]);
    assert_eq!(status, 2, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");

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
    assert_ne!(status, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stderr.contains("--group") || stdout.contains("--group"),
        "stdout: {stdout}\nstderr: {stderr}"
    );

    let _ = fs::remove_dir_all(root);
}
