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
