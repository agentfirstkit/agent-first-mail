#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic)]

use agent_first_mail::store::Workspace;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

#[path = "support/cli.rs"]
#[allow(dead_code)]
mod cli;

const WORKER_MODE: &str = "AFMAIL_TEST_FILE_DESCRIPTOR_MODE";
const FILE_LIMIT: libc::rlim_t = 64;
const MESSAGE_COUNT: usize = 5_000;

fn run_worker(mode: &str) {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "file_descriptor_worker",
            "--ignored",
            "--nocapture",
        ])
        .env(WORKER_MODE, mode);
    // Only the isolated child receives this limit; parallel tests and Cargo
    // keep their original process limit. The hook performs only libc calls.
    unsafe {
        command.pre_exec(|| {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            limit.rlim_cur = FILE_LIMIT.min(limit.rlim_max);
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().expect("spawn low-limit test worker");
    assert!(
        output.status.success(),
        "worker {mode} failed under {FILE_LIMIT} descriptors:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn deep_workspace_walks_have_bounded_file_descriptors() {
    run_worker("deep");
}

#[test]
fn thousands_of_messages_and_repeated_operations_have_bounded_file_descriptors() {
    run_worker("many");
}

#[test]
#[ignore = "invoked by the isolated low-file-limit parent tests"]
fn file_descriptor_worker() {
    let mode = std::env::var(WORKER_MODE).expect("worker mode");
    let root = tempfile::tempdir().expect("temporary workspace");
    let workspace = Workspace::at(root.path());
    workspace.init().expect("init workspace");
    let initial_descriptor_count = open_descriptor_count();
    match mode.as_str() {
        "deep" => exercise_deep_walks(root.path(), &workspace),
        "many" => exercise_large_mailbox(root.path(), &workspace),
        _ => panic!("unknown worker mode {mode}"),
    }
    assert_eq!(
        open_descriptor_count(),
        initial_descriptor_count,
        "operations must release their descriptors before returning"
    );
}

fn open_descriptor_count() -> usize {
    (0..FILE_LIMIT as libc::c_int)
        // F_GETFD inspects an existing descriptor without opening another one.
        .filter(|descriptor| unsafe { libc::fcntl(*descriptor, libc::F_GETFD) } != -1)
        .count()
}

fn exercise_deep_walks(root: &Path, workspace: &Workspace) {
    cli::write_message(root, "message_deep", 1, None);
    let mut directories = Vec::new();
    let mut leaves = Vec::new();
    for relative in [
        ".afmail/messages/message_deep.files",
        "cases/support/nested/files",
    ] {
        let mut path = root.join(relative);
        fs::create_dir_all(&path).expect("attachment root");
        for _ in 0..128 {
            path = path.join("n");
            fs::create_dir(&path).expect("nested attachment directory");
            directories.push(path.clone());
        }
        let leaf = path.join("payload.txt");
        fs::write(&leaf, b"payload").expect("nested attachment");
        leaves.push(leaf);
    }
    let status = workspace.status().expect("deep storage traversal");
    assert_eq!(status["storage"]["attachment_bytes"], 14);
    workspace
        .init()
        .expect("deep migration preflight traversal");

    // Purge planning traverses the same extracted attachment tree without
    // deleting it. The disposable fixture has only a local spam disposition.
    fs::create_dir_all(root.join("spam/data")).expect("spam collection directory");
    cli::write_json(
        &root.join("spam/data/spam.json"),
        &serde_json::json!({
            "schema_name": "spam",
            "schema_version": 2,
            "collection_uid": "spam",
            "collection_name": "spam",
            "status": "spam",
            "items": [{
                "message_id": "message_deep",
                "added_rfc3339": "2026-05-21T10:00:00Z"
            }]
        }),
    );
    let plan = workspace
        .purge_spam(0, None)
        .expect("deep purge plan traversal");
    assert_eq!(plan["candidate_count"], 1, "{plan}");

    // std's own recursive remove_dir_all can require several descriptors per
    // level, so remove this intentionally deep fixture from the leaves up.
    for leaf in leaves {
        fs::remove_file(leaf).expect("remove nested fixture file");
    }
    for directory in directories.into_iter().rev() {
        fs::remove_dir(directory).expect("remove nested fixture directory");
    }
}

fn exercise_large_mailbox(root: &Path, workspace: &Workspace) {
    for index in 0..MESSAGE_COUNT {
        let message_id = format!("message_fd_{index:05}");
        cli::write_message(root, &message_id, index as u64 + 1, None);
        cli::write_triage(root, &message_id, "");
    }
    for _ in 0..16 {
        let status = workspace.status().expect("large mailbox status");
        assert_eq!(status["message_count"], MESSAGE_COUNT);
        let triage = workspace
            .triage_list(None, false)
            .expect("large triage list");
        assert_eq!(triage["returned_count"], MESSAGE_COUNT);
    }
    for _ in 0..4 {
        workspace.render_refresh().expect("large mailbox rendering");
        let doctor = workspace.doctor().expect("large mailbox doctor");
        assert_eq!(doctor.value()["error_count"], 0, "{doctor:?}");
    }
}
