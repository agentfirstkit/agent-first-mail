use serde_json::Value;
use std::fs;

#[path = "support/cli.rs"]
mod cli;

use cli::{parse_one, run, temp_root};

#[test]
fn explicit_workspace_selects_target_workspace_from_agent_root() {
    let agent_root = temp_root("explicit-agent-root");
    assert!(fs::create_dir_all(&agent_root).is_ok());

    let (status, stdout) = run(&agent_root, &["init", "mail-a"]);
    assert_eq!(status, 0, "{stdout}");
    let mail_a_uid = parse_one(&stdout)["workspace_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, stdout) = run(&agent_root, &["init", "mail-b"]);
    assert_eq!(status, 0, "{stdout}");
    let mail_b = agent_root.join("mail-b");
    let mail_b_uid = parse_one(&stdout)["workspace_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(mail_a_uid, mail_b_uid);

    let (status, stdout) = run(
        &agent_root,
        &["--workspace", mail_b.to_str().unwrap_or_default(), "status"],
    );
    assert_eq!(status, 0, "{stdout}");
    let selected = parse_one(&stdout);
    assert_eq!(selected["workspace_uid"], mail_b_uid);
    assert!(
        selected["workspace_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("/mail-b"))
    );

    let _ = fs::remove_dir_all(agent_root);
}

#[test]
fn explicit_workspace_cannot_be_combined_with_init() {
    let agent_root = temp_root("explicit-init-rejected");
    assert!(fs::create_dir_all(&agent_root).is_ok());

    let (status, stdout) = run(&agent_root, &["init", "mail"]);
    assert_eq!(status, 0, "{stdout}");
    let workspace = agent_root.join("mail");

    let (status, stdout) = run(
        &agent_root,
        &[
            "--workspace",
            workspace.to_str().unwrap_or_default(),
            "init",
            "other",
        ],
    );
    assert_eq!(status, 2, "{stdout}");
    assert!(
        parse_one(&stdout)["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot be used with init")
    );

    let _ = fs::remove_dir_all(agent_root);
}

#[test]
fn init_subdirectory_registers_agent_root_and_rejects_reserved_paths() {
    let root = temp_root("init-subdir");
    assert!(fs::create_dir_all(&root).is_ok());

    let (status, stdout) = run(&root, &["init", "email-a"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["workspace_ref"], "email-a/");
    assert!(value.get("agent_skill_path").is_none());
    assert!(root.join("email-a/.afmail/config.json").is_file());
    assert!(root.join("email-a/templates").is_dir());
    assert!(root.join("email-a/.gitignore").is_file());
    assert!(!root.join(".codex").exists());
    assert!(!root.join("AGENTS.md").exists());
    assert!(!root.join(".gitignore").exists());
    let workspace_gitignore =
        fs::read_to_string(root.join("email-a/.gitignore")).unwrap_or_default();
    assert!(workspace_gitignore.contains("messages/*.json"));
    assert!(!workspace_gitignore.contains(".codex/skills/agent-first-mail/"));

    let (status, stdout) = run(&root, &["init", "email-b"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(!root.join("AGENTS.md").exists());

    assert!(fs::create_dir_all(root.join("not-mail/triage")).is_ok());
    let (status, stdout) = run(&root, &["init", "not-mail"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "invalid_request");
    let error_text = value
        .get("message")
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(error_text.contains("afmail-reserved paths"));
    assert!(!root.join("not-mail/.afmail").exists());

    let _ = fs::remove_dir_all(root);
}
