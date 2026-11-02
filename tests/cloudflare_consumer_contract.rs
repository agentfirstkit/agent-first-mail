#![cfg(feature = "cloudflare")]
#![allow(clippy::unwrap_used, reason = "contract fixture assertions")]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[allow(dead_code)]
#[path = "support/cli.rs"]
mod cli;
#[path = "support/fake_queue.rs"]
mod fake_queue;
#[path = "support/fake_s3.rs"]
mod fake_s3;
use cli::{parse_one, read_json, run, write_json};
const RAW: &[u8] = b"Message-ID: <mail@example.net>\r\nFrom: sender@example.net\r\nTo: visible@example.net\r\nSubject: mail; $(touch PWNED)\r\n\r\nHello\r\n";

fn configure_tree_handler(f: &Fixture, outcome: &str) {
    let hook = f.root().join("exiting-tree.py");
    fs::write(
        &hook,
        r#"import pathlib, subprocess, sys, time
if sys.argv[1] == 'child':
    pathlib.Path('tree-started').touch()
    time.sleep(2)
    pathlib.Path('tree-survived').touch()
else:
    subprocess.Popen([sys.executable, __file__, 'child'])
    while not pathlib.Path('tree-started').exists(): time.sleep(.01)
    if sys.argv[1] == 'fail': sys.exit(7)
    if sys.argv[1] == 'success': sys.exit(0)
    time.sleep(10)
"#,
    )
    .unwrap();
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["consumer"]["command"] = json!(["python3", hook, outcome]);
    write_json(&path, &config);
    f.s3.put("inbox/tree.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/tree.eml", RAW, "tree");
}

#[test]
fn exiting_handler_reclaims_descendants_before_success_or_retry() {
    for outcome in ["fail", "success"] {
        let f = Fixture::new();
        configure_tree_handler(&f, outcome);
        let result = f.tick(false);
        assert!(f.root().join("tree-started").exists());
        assert_eq!(result["dispatched"], outcome == "success");
        if outcome == "fail" {
            assert_eq!(result["warnings"][0]["error"]["exit_code"], 7);
            assert_eq!(result["pending_message_count"], 1);
        }
        std::thread::sleep(std::time::Duration::from_millis(2200));
        assert!(
            !f.root().join("tree-survived").exists(),
            "{outcome}: descendant outlived handler"
        );
    }
}

#[cfg(unix)]
#[test]
fn interrupting_consume_or_watch_reclaims_the_tree_and_retains_the_task() {
    for args in [["cloudflare", "consume"], ["cloudflare", "watch"]] {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let f = Fixture::new();
            configure_tree_handler(&f, "wait");
            let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_afmail"))
                .args(args)
                .current_dir(f.root())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let started = std::time::Instant::now();
            while !f.root().join("tree-started").exists() && started.elapsed().as_secs() < 10 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !f.root().join("tree-started").exists() {
                let _ = child.kill();
                let _ = child.wait();
                assert!(
                    f.root().join("tree-started").exists(),
                    "handler never started"
                );
            }
            assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
            let stopped = std::time::Instant::now();
            while child.try_wait().unwrap().is_none() && stopped.elapsed().as_secs() < 5 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let exited = child.try_wait().unwrap().is_some();
            if !exited {
                let _ = child.kill();
            }
            let output = child.wait_with_output().unwrap();
            assert!(exited, "consumer ignored shutdown");
            assert_eq!(output.status.code(), Some(1));
            let terminal = String::from_utf8([output.stdout, output.stderr].concat()).unwrap();
            assert!(terminal.contains("consumer_interrupted"), "{terminal}");
            let journal = read_json(&f.journal_path());
            assert_eq!(
                journal["journal"]["active"]["last_error_code"],
                "consumer_interrupted"
            );
            assert_eq!(
                journal["journal"]["tasks"]
                    .as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()["status"],
                "pending"
            );
            std::thread::sleep(std::time::Duration::from_millis(2200));
            assert!(!f.root().join("tree-survived").exists());
        }
    }
}
#[cfg(unix)]
#[test]
fn idle_watch_interrupt_does_not_wait_for_the_poll_interval() {
    use std::io::BufRead as _;
    let f = Fixture::new();
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["consumer"]["poll_interval_s"] = json!(30);
    write_json(&path, &config);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_afmail"))
        .args(["cloudflare", "watch"])
        .current_dir(f.root())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    output.read_line(&mut ready).unwrap();
    assert!(ready.contains("cloudflare_consumer_ready"), "{ready}");
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() && started.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let stopped = child.try_wait().unwrap().is_some();
    if !stopped {
        let _ = child.kill();
    }
    let _ = child.wait();
    assert!(stopped, "idle watch ignored interrupt");
}

struct Fixture {
    root: tempfile::TempDir,
    s3: fake_s3::FakeS3,
    queue: fake_queue::FakeQueue,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let s3 = fake_s3::FakeS3::new();
        let queue = fake_queue::FakeQueue::new();
        assert_eq!(run(root.path(), &["init"]).0, 0);
        let hook = root.path().join("handler ; file.py");
        fs::write(&hook, r#"import json, os, pathlib, subprocess, sys
root = pathlib.Path.cwd()
task = json.loads(pathlib.Path(os.environ['AFMAIL_TASK_PATH']).read_text())
assert task['task_id'] == os.environ['AFMAIL_TASK_ID']
with (root / 'calls.jsonl').open('a') as output:
    output.write(json.dumps(task) + '\n')
for message in task['messages']:
    assert (root / message['raw_path']).is_file()
    result = subprocess.run([sys.argv[1], 'message', 'show', message['message_id']], capture_output=True)
    assert result.returncode == 0, result.stderr
if (root / 'fail-handler').exists(): sys.exit(1)
"#).unwrap();
        write_json(
            &root.path().join(".afmail/config.json"),
            &json!({
                "cloudflare":{
                    "account_id":"a".repeat(32),
                    "r2":{"endpoint_url":s3.endpoint,"bucket":"spool","access_key_id":"test-access","secret_access_key_secret":"test-secret"},
                    "queue":{"queue_id":"b".repeat(32),"api_token_secret":"queue-test-token","endpoint_url":queue.endpoint},
                    "consumer":{"command":["python3",hook,env!("CARGO_BIN_EXE_afmail")],"retry_delay_s":1}
                },"identities":[{"identity":"me","name":"Me","email":"hidden@example.com","default":true}]
            }),
        );
        Self { root, s3, queue }
    }
    fn root(&self) -> &Path {
        self.root.path()
    }
    fn tick(&self, reconcile: bool) -> Value {
        let args = if reconcile {
            vec!["cloudflare", "consume", "--reconcile"]
        } else {
            vec!["cloudflare", "consume"]
        };
        let (status, output) = run(self.root(), &args);
        assert_eq!(status, 0, "{output}");
        assert!(!output.contains("queue-test-token"));
        parse_one(&output)
    }
    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.root().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn journal_path(&self) -> PathBuf {
        self.root().join(".afmail/cloudflare-consumer/journal.json")
    }
    fn retry_now(&self) {
        let mut value = read_json(&self.journal_path());
        value["journal"]["active"]["retry_after_epoch_s"] = json!(0);
        write_json(&self.journal_path(), &value);
    }
}
#[test]
fn invalid_completion_evidence_neither_replays_nor_loses_an_active_batch() {
    let f = Fixture::new();
    fs::write(f.root().join("fail-handler"), b"").unwrap();
    f.s3.put("inbox/marker.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/marker.eml", RAW, "marker");
    assert_eq!(f.tick(false)["pending_message_count"], 1);
    fs::remove_file(f.root().join("fail-handler")).unwrap();
    f.retry_now();
    let state = read_json(&f.journal_path());
    let task_id = state["journal"]["active"]["task_id"].as_str().unwrap();
    let marker = f.root().join(format!(
        ".afmail/cloudflare-consumer/batches/{task_id}.completed"
    ));
    fs::write(&marker, b"{}").unwrap();
    let before = fs::read(f.journal_path()).unwrap();
    let (code, output) = run(f.root(), &["cloudflare", "consume"]);
    assert_eq!(code, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "consumer_completion_invalid");
    assert_eq!(f.calls().len(), 1);
    assert_eq!(fs::read(f.journal_path()).unwrap(), before);
    write_json(
        &marker,
        &json!({"schema_name":"mail_received_task_completed","schema_version":1,"task_id":task_id}),
    );
    assert_eq!(f.tick(false)["pending_message_count"], 0);
    assert_eq!(f.calls().len(), 1);
}

#[test]
fn idle_ticks_never_launch_agent_and_reconciliation_recovers_unnotified_mail() {
    let f = Fixture::new();
    assert_eq!(f.tick(false)["received_event_count"], 0);
    assert!(f.calls().is_empty());
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    assert_eq!(f.tick(false)["imported_count"], 0);
    assert_eq!(f.tick(true)["imported_count"], 1);
    assert_eq!(f.calls().len(), 1);
    assert_eq!(f.tick(true)["dispatched"], false);
    assert_eq!(f.calls().len(), 1);
}
#[test]
fn events_import_directly_coalesce_duplicates_and_release_workspace_before_handler() {
    let f = Fixture::new();
    for (key, recipient, lease) in [
        ("inbox/a.eml", "hidden@example.com", "a"),
        ("inbox/b.eml", "alias@example.com", "b"),
    ] {
        f.s3.put(key, RAW, recipient);
        f.queue.notify(key, RAW, lease);
    }
    let result = f.tick(false);
    assert_eq!(result["acknowledged_event_count"], 2);
    assert_eq!(result["imported_count"], 1);
    assert_eq!(result["pending_message_count"], 0);
    assert_eq!(result["warnings"], json!([]), "{result}");
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["messages"].as_array().unwrap().len(), 1);
    assert!(!f.root().join("PWNED").exists());
    assert!(
        !f.s3
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(method, key)| method == "GET" && key.is_empty())
    );
    let id = calls[0]["messages"][0]["message_id"].as_str().unwrap();
    let raw = fs::read(f.root().join(format!(".afmail/messages/{id}.eml"))).unwrap();
    assert_eq!(raw, RAW);
    let message = parse_one(&run(f.root(), &["message", "show", id]).1);
    assert!(message.to_string().contains("alias@example.com"));
    f.queue.notify("inbox/a.eml", RAW, "replay");
    let replay = f.tick(false);
    assert_eq!(
        replay["acknowledged_event_count"],
        1,
        "{replay}; requests: {:?}",
        f.queue.state.lock().unwrap().requests
    );
    assert_eq!(f.calls().len(), 1);
}
#[test]
fn handler_failure_retries_stable_task_after_remote_mail_and_event_are_gone() {
    let f = Fixture::new();
    fs::write(f.root().join("fail-handler"), "").unwrap();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let result = f.tick(false);
    assert_eq!(result["dispatched"], false);
    assert_eq!(result["pending_message_count"], 1);
    assert!(f.s3.state.lock().unwrap().objects.is_empty());
    assert!(
        f.queue.state.lock().unwrap().messages.is_empty(),
        "{result}"
    );
    assert_eq!(f.calls().len(), 1);
    let journal_before = fs::read(f.journal_path()).unwrap();
    let (code, output) = run(f.root(), &["status"]);
    assert_eq!(code, 0, "{output}");
    let state = parse_one(&output)["cloudflare_consumer"].clone();
    assert_eq!(state["pending_message_count"], 1);
    assert_eq!(state["active_batch"]["attempt_count"], 1);
    assert_eq!(
        state["active_batch"]["last_error_code"],
        "consumer_command_failed"
    );
    assert_eq!(state["active_batch"]["last_exit_code"], 1);
    assert_eq!(
        state["active_batch"]["message_ids"],
        json!([f.calls()[0]["messages"][0]["message_id"]])
    );
    assert!(state["active_batch"]["last_attempt_rfc3339"].is_string());
    let (code, output) = run(f.root(), &["doctor"]);
    assert_eq!(code, 3, "{output}");
    assert!(
        parse_one(&output)["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "consumer_batch_failed")
    );
    assert_eq!(fs::read(f.journal_path()).unwrap(), journal_before);
    fs::remove_file(f.root().join("fail-handler")).unwrap();
    f.retry_now();
    assert_eq!(f.tick(false)["dispatched"], true);
    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    f.tick(false);
    assert_eq!(f.calls().len(), 2);
    assert!(
        parse_one(&run(f.root(), &["status"]).1)["cloudflare_consumer"]["active_batch"].is_null()
    );
}

#[test]
fn timed_out_handler_cannot_leave_a_descendant_running_into_the_retry() {
    let f = Fixture::new();
    let hook = f.root().join("process-tree.py");
    fs::write(
        &hook,
        r#"import pathlib, subprocess, sys, time
if len(sys.argv) > 1:
    pathlib.Path('descendant-started').touch()
    time.sleep(2)
    pathlib.Path('descendant-survived').touch()
else:
    subprocess.Popen([sys.executable, __file__, 'child'])
    time.sleep(10)
"#,
    )
    .unwrap();
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["consumer"]["command"] = json!(["python3", hook]);
    config["cloudflare"]["consumer"]["command_timeout_s"] = json!(1);
    write_json(&path, &config);
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let result = f.tick(false);
    assert_eq!(
        result["warnings"][0]["error"]["code"],
        "consumer_command_timeout"
    );
    assert!(f.root().join("descendant-started").is_file());
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(!f.root().join("descendant-survived").exists());
    assert_eq!(result["pending_message_count"], 1);
}
#[test]
fn cleanup_failure_and_lost_queue_ack_do_not_repeat_agent_work() {
    let f = Fixture::new();
    f.s3.state.lock().unwrap().delete_failures = 1;
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let first = f.tick(false);
    assert_eq!(
        first["retried_event_count"],
        1,
        "{first}; queue requests: {:?}; s3 requests: {:?}",
        f.queue.state.lock().unwrap().requests,
        f.s3.state.lock().unwrap().requests
    );
    assert_eq!(f.calls().len(), 1);
    f.queue.state.lock().unwrap().ack_failures = 1;
    let result = f.tick(false);
    assert!(!result["warnings"].as_array().unwrap().is_empty());
    assert!(f.s3.state.lock().unwrap().objects.is_empty());
    let replay = f.tick(false);
    assert_eq!(
        replay["acknowledged_event_count"],
        1,
        "{replay}; requests: {:?}",
        f.queue.state.lock().unwrap().requests
    );
    assert_eq!(f.calls().len(), 1);
}
#[test]
fn missing_object_is_never_silently_acknowledged() {
    let f = Fixture::new();
    f.queue.notify("inbox/missing.eml", RAW, "missing");
    let result = f.tick(false);
    assert_eq!(result["acknowledged_event_count"], 0);
    assert_eq!(result["retried_event_count"], 1);
    assert!(f.calls().is_empty());
}
#[test]
fn malformed_and_wrong_scope_events_do_not_block_valid_mail() {
    let f = Fixture::new();
    f.queue.state.lock().unwrap().messages = vec![
        json!({"body":"invalid", "lease_id":"bad"}),
        json!({"body":{"account":"a".repeat(32),"action":"PutObject","bucket":"elsewhere","object":{"key":"inbox/x","size":1}},"lease_id":"wrong"}),
    ];
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let result = f.tick(false);
    assert_eq!(result["acknowledged_event_count"], 1);
    assert_eq!(result["retried_event_count"], 2);
    assert_eq!(f.calls().len(), 1);
}
#[test]
fn reconciliation_refuses_objects_without_a_reported_size() {
    let f = Fixture::new();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.s3.state.lock().unwrap().omit_list_size = true;
    let result = f.tick(true);
    assert_eq!(result["imported_count"], 0);
    assert_eq!(result["dispatched"], false);
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["error"]["code"] == "cloudflare_r2_failed"),
        "{result}"
    );
    assert!(f.calls().is_empty());
    assert!(
        f.s3.state
            .lock()
            .unwrap()
            .objects
            .contains_key("inbox/a.eml")
    );
}

#[test]
fn queue_outage_does_not_stop_r2_reconciliation_or_local_dispatch() {
    let f = Fixture::new();
    f.queue.state.lock().unwrap().fail_pull = true;
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    let result = f.tick(true);
    assert_eq!(result["imported_count"], 1);
    assert_eq!(result["dispatched"], true);
    assert!(!result["warnings"].as_array().unwrap().is_empty());
}
#[test]
fn durable_preparing_intent_survives_a_crash_between_import_and_task_readiness() {
    let f = Fixture::new();
    // Retain the source, then reconstruct the persisted boundary immediately
    // after raw import and before the notification was made ready.
    f.s3.state.lock().unwrap().delete_failures = 1;
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    fs::write(f.root().join("fail-handler"), "").unwrap();
    f.tick(false);
    fs::remove_file(f.root().join("fail-handler")).unwrap();
    let mut journal = read_json(&f.journal_path());
    journal["journal"]["active"] = Value::Null;
    journal["journal"]["receipts"] = json!({});
    for task in journal["journal"]["tasks"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        task["status"] = json!("preparing");
    }
    write_json(&f.journal_path(), &journal);
    assert_eq!(f.tick(false)["dispatched"], true);
    assert_eq!(f.calls().len(), 2);
    assert_eq!(f.calls()[0]["messages"], f.calls()[1]["messages"]);
}
#[test]
fn queue_token_redacts_and_consumer_config_rejects_unsafe_ranges() {
    let f = Fixture::new();
    for format in ["json", "yaml", "plain"] {
        let (status, output) = run(f.root(), &["config", "show", "--output", format]);
        assert_eq!(status, 0);
        assert!(!output.contains("queue-test-token"));
    }
    for (key, value) in [
        ("cloudflare.queue.batch_size", "0"),
        ("cloudflare.queue.batch_size", "101"),
        ("cloudflare.queue.visibility_timeout_s", "43201"),
        ("cloudflare.consumer.poll_interval_s", "0"),
        ("cloudflare.consumer.command_timeout_s", "0"),
    ] {
        assert_eq!(run(f.root(), &["config", "set", key, value]).0, 1);
    }
}

#[test]
fn managed_cleanup_repairs_quota_after_lost_response_without_relaunching_agent() {
    let f = Fixture::new();
    f.s3.state.lock().unwrap().quota_managed = true;
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["spool"] =
        json!({"control_url":f.s3.endpoint,"token_secret":"q".repeat(32)});
    write_json(&path, &config);
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    assert_eq!(f.tick(false)["dispatched"], true);
    let mut journal = read_json(&f.journal_path());
    for receipt in journal["journal"]["receipts"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        receipt["cleanup_complete"] = json!(false);
    }
    write_json(&f.journal_path(), &journal);
    f.queue.notify("inbox/a.eml", RAW, "again");
    assert_eq!(f.tick(false)["acknowledged_event_count"], 1);
    assert_eq!(f.calls().len(), 1);
    assert!(
        !f.s3
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(method, _)| method == "DELETE")
    );
}

#[test]
fn handler_timeout_retains_task_and_consumer_lock_excludes_a_second_tick() {
    let f = Fixture::new();
    let slow = f.root().join("slow.py");
    fs::write(
        &slow,
        "import pathlib,time\npathlib.Path('handler-started').touch()\ntime.sleep(10)\n",
    )
    .unwrap();
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["consumer"]["command"] = json!(["python3", slow]);
    config["cloudflare"]["consumer"]["command_timeout_s"] = json!(1);
    write_json(&path, &config);
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_afmail"))
        .args(["cloudflare", "consume"])
        .current_dir(f.root())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while !f.root().join("handler-started").exists() && started.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(f.root().join("handler-started").exists());
    let second = std::process::Command::new(env!("CARGO_BIN_EXE_afmail"))
        .args(["cloudflare", "consume"])
        .current_dir(f.root())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    assert_eq!(
        parse_one(&String::from_utf8(second.stderr).unwrap())["code"],
        "consumer_locked"
    );
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let result = parse_one(&String::from_utf8(output.stdout).unwrap());
    assert_eq!(result["pending_message_count"], 1);
    assert_eq!(
        result["warnings"][0]["error"]["code"],
        "consumer_command_timeout"
    );
}

#[test]
fn watch_reports_readiness_stays_quiet_when_idle_and_processes_new_events() {
    use std::io::BufRead as _;
    let f = Fixture::new();
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["consumer"]["poll_interval_s"] = json!(1);
    write_json(&path, &config);
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_afmail"))
            .args(["cloudflare", "watch"])
            .current_dir(f.root())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let ready: Value =
        serde_json::from_str(&rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap())
            .unwrap();
    assert_eq!(ready["progress"]["phase"], "cloudflare_consumer_ready");
    assert!(f.calls().is_empty());
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(1200))
            .is_err()
    );
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let tick: Value =
        serde_json::from_str(&rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap())
            .unwrap();
    assert_eq!(tick["progress"]["phase"], "cloudflare_consumer_tick");
    assert_eq!(tick["progress"]["dispatched"], true);
    assert_eq!(f.calls().len(), 1);
    drop(child);
    reader.join().unwrap();
}

const RAW_B: &[u8] = b"Message-ID: <mail-b@example.net>\r\nFrom: sender@example.net\r\nTo: visible@example.net\r\nSubject: second\r\n\r\nHello again\r\n";

#[test]
fn unparseable_object_is_reported_and_left_without_blocking_newer_mail() {
    let f = Fixture::new();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.s3.put("inbox/b.eml", b"", "hidden@example.com");
    f.s3.put("inbox/c.eml", RAW_B, "hidden@example.com");
    let (status, output) = run(f.root(), &["pull", "--cloudflare"]);
    assert_eq!(status, 0, "{output}");
    let pulled = parse_one(&output);
    assert_eq!(pulled["new_message_count"], 2, "{pulled}");
    let warnings = pulled["cloudflare"]["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{pulled}");
    assert_eq!(warnings[0]["key"], "inbox/b.eml");
    assert_eq!(warnings[0]["code"], "mime_parse_failed");
    let objects = f.s3.state.lock().unwrap().objects.clone();
    assert_eq!(objects.keys().collect::<Vec<_>>(), vec!["inbox/b.eml"]);
    let journal = read_json(&f.journal_path());
    let tasks = journal["journal"]["tasks"].as_object().unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.values().all(|task| task["status"] != "preparing"));
    // The reconciling consumer keeps going past the same object.
    let tick = f.tick(true);
    assert!(tick["reconciled"].as_bool().unwrap(), "{tick}");
    assert!(
        tick["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["key"] == "inbox/b.eml"),
        "{tick}"
    );
    assert_eq!(tick["pending_message_count"], 0, "{tick}");
}

#[test]
fn locked_workspace_never_leases_queue_events() {
    use fs4::FileExt;
    let f = Fixture::new();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(f.root().join(".afmail/cache/workspace.lock"))
        .unwrap();
    FileExt::try_lock(&lock).unwrap();
    let (status, output) = run(f.root(), &["cloudflare", "consume"]);
    assert_eq!(status, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "workspace_locked");
    assert!(
        f.queue.state.lock().unwrap().requests.is_empty(),
        "no lease may be taken without the workspace lock"
    );
    FileExt::unlock(&lock).unwrap();
    assert_eq!(f.tick(false)["acknowledged_event_count"], 1);
}

#[test]
fn handler_outcome_survives_a_workspace_lock_held_past_handler_exit() {
    let f = Fixture::new();
    let hook = f.root().join("hold.py");
    fs::write(
        &hook,
        r#"import fcntl, os, pathlib, subprocess, sys, time
holder = "import fcntl, sys, time\nf = open(sys.argv[1], 'a')\nfcntl.flock(f, fcntl.LOCK_EX)\nopen(sys.argv[2], 'w').close()\ntime.sleep(2)\n"
root = pathlib.Path.cwd()
ready = root / 'holder-ready'
subprocess.Popen([sys.executable, '-c', holder, str(root / '.afmail/cache/workspace.lock'), str(ready)])
while not ready.exists(): time.sleep(0.01)
with (root / 'calls.jsonl').open('a') as output:
    output.write(os.environ['AFMAIL_TASK_ID'] + '\n')
ready.unlink()
if (root / 'fail-handler').exists(): sys.exit(1)
"#,
    )
    .unwrap();
    let path = f.root().join(".afmail/config.json");
    let mut config = read_json(&path);
    config["cloudflare"]["consumer"]["command"] = json!(["python3", hook]);
    write_json(&path, &config);
    let calls = || {
        fs::read_to_string(f.root().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    fs::write(f.root().join("fail-handler"), "").unwrap();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    let failed = f.tick(false);
    assert_eq!(
        failed["warnings"][0]["error"]["code"], "consumer_command_failed",
        "{failed}"
    );
    let retry_after = read_json(&f.journal_path())["journal"]["active"]["retry_after_epoch_s"]
        .as_i64()
        .unwrap();
    assert!(
        retry_after <= chrono::Utc::now().timestamp() + 2,
        "{retry_after}"
    );
    fs::remove_file(f.root().join("fail-handler")).unwrap();
    f.retry_now();
    let done = f.tick(false);
    assert_eq!(done["dispatched"], true, "{done}");
    assert_eq!(done["pending_message_count"], 0, "{done}");
    assert_eq!(
        read_json(&f.journal_path())["journal"]["active"],
        Value::Null
    );
    assert_eq!(calls(), 2);
    assert_eq!(f.tick(false)["dispatched"], false);
    assert_eq!(calls(), 2);
}

#[test]
fn purged_message_is_dropped_from_its_batch_without_blocking_dispatch() {
    let f = Fixture::new();
    fs::write(f.root().join("fail-handler"), "").unwrap();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    f.tick(false);
    let purged = f.calls()[0]["messages"][0]["message_id"]
        .as_str()
        .unwrap()
        .to_string();
    fs::remove_file(f.root().join(format!(".afmail/messages/{purged}.eml"))).unwrap();
    fs::remove_file(f.root().join("fail-handler")).unwrap();
    f.retry_now();
    let cleared = f.tick(false);
    assert_eq!(cleared["dispatched"], false, "{cleared}");
    assert_eq!(
        cleared["warnings"][0]["error"]["code"],
        "consumer_message_missing"
    );
    assert_eq!(
        cleared["warnings"][0]["error"]["message_id"],
        purged.as_str()
    );
    assert_eq!(
        read_json(&f.journal_path())["journal"]["active"],
        Value::Null
    );
    f.s3.put("inbox/b.eml", RAW_B, "hidden@example.com");
    f.queue.notify("inbox/b.eml", RAW_B, "b");
    let next = f.tick(false);
    assert_eq!(next["dispatched"], true, "{next}");
    assert_eq!(f.calls().len(), 2);
    assert_ne!(f.calls()[1]["messages"][0]["message_id"], purged.as_str());
}

#[test]
fn consumer_runs_after_the_cache_directory_is_removed() {
    let f = Fixture::new();
    fs::remove_dir_all(f.root().join(".afmail/cache")).unwrap();
    assert_eq!(f.tick(false)["received_event_count"], 0);
}

/// Cloudflare caps Queue retention at 14 days; afmail keeps one day of margin.
const RECEIPT_RETENTION_S: i64 = 15 * 86_400;
fn now_epoch_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn age_receipts(f: &Fixture, age_s: i64) {
    let mut journal = read_json(&f.journal_path());
    for receipt in journal["journal"]["receipts"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        receipt["imported_epoch_s"] = json!(now_epoch_s() - age_s);
    }
    write_json(&f.journal_path(), &journal);
}
fn distinct_raw(index: usize) -> Vec<u8> {
    format!("Message-ID: <mail-{index}@example.net>\r\nFrom: sender@example.net\r\nTo: visible@example.net\r\nSubject: mail {index}\r\n\r\nHello\r\n").into_bytes()
}

#[test]
fn a_receipt_inside_the_queue_retention_window_still_answers_a_redelivery() {
    let f = Fixture::new();
    f.s3.put("inbox/a.eml", RAW, "hidden@example.com");
    f.queue.notify("inbox/a.eml", RAW, "a");
    assert_eq!(f.tick(false)["dispatched"], true);
    let journal = read_json(&f.journal_path());
    assert_eq!(journal["schema_version"], 2);
    assert!(journal["journal"]["tasks"].as_object().unwrap().is_empty());
    age_receipts(&f, RECEIPT_RETENTION_S - 3600);
    // The object is gone; only the receipt can settle the redelivered event.
    f.queue.notify("inbox/a.eml", RAW, "again");
    let result = f.tick(false);
    assert_eq!(result["acknowledged_event_count"], 1, "{result}");
    assert_eq!(result["retried_event_count"], 0, "{result}");
    assert_eq!(f.calls().len(), 1);
    let receipts = read_json(&f.journal_path())["journal"]["receipts"].clone();
    assert_eq!(receipts.as_object().unwrap().len(), 1, "{receipts}");
}

#[test]
fn cleaned_receipts_past_the_window_are_pruned_and_the_journal_stays_bounded() {
    let f = Fixture::new();
    let mut sizes = Vec::new();
    for index in 0..12 {
        let key = format!("inbox/m{index:02}.eml");
        let raw = distinct_raw(index);
        f.s3.put(&key, &raw, "hidden@example.com");
        f.queue.notify(&key, &raw, &format!("lease-{index}"));
        assert_eq!(f.tick(false)["dispatched"], true);
        let journal = read_json(&f.journal_path());
        // The receipt just written is inside the window; every earlier one was
        // aged past it and dropped at this tick's first save.
        let receipts = journal["journal"]["receipts"].as_object().unwrap();
        assert_eq!(receipts.len(), 1, "{journal}");
        assert!(journal["journal"]["tasks"].as_object().unwrap().is_empty());
        sizes.push(fs::metadata(f.journal_path()).unwrap().len());
        age_receipts(&f, RECEIPT_RETENTION_S + 60);
    }
    assert_eq!(f.calls().len(), 12);
    assert!(sizes.iter().all(|size| *size == sizes[0]), "{sizes:?}");
}
