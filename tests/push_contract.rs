use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(feature = "smtp")]
use std::process::Command;
#[cfg(feature = "smtp")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/cli.rs"]
mod cli;

use cli::{
    parse_one, read_json, run, temp_root, update_message_json, write_json, write_message,
    write_triage,
};
use serde_json::{Value, json};

static NETWORK_TEST_LOCK: Mutex<()> = Mutex::new(());

type AppendedMessages = Arc<Mutex<Vec<(String, bool, String)>>>;
type CopiedMessages = Arc<Mutex<Vec<(String, u64, String)>>>;
type ExpungedMessages = Arc<Mutex<Vec<(String, u64)>>>;
type ImapCommands = Arc<Mutex<Vec<String>>>;
type MovedMessages = Arc<Mutex<Vec<(String, u64, String)>>>;
type StoredMessages = Arc<Mutex<Vec<(String, u64, String)>>>;

fn draft_with_compose(raw: String) -> String {
    let Some((frontmatter, body)) = raw.split_once("\n---\n") else {
        return raw;
    };
    format!(
        "{frontmatter}\n---\n# Draft\n\n<!-- afmail:compose:start -->\n{}\n<!-- afmail:compose:end -->\n",
        body.trim_matches(['\r', '\n'])
    )
}

fn create_case(root: &std::path::Path, name: &str, reason: &str) -> (String, std::path::PathBuf) {
    let (status, stdout) = run(
        root,
        &[
            "case", "create", "--name", name, "--group", "support", "--reason", reason,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let case_uid = value["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty(), "{value}");
    let case_path = value["case_path"].as_str().unwrap_or_default();
    assert!(!case_path.is_empty(), "{value}");
    (case_uid, root.join(case_path))
}

fn create_case_with_message(
    root: &std::path::Path,
    name: &str,
    message_id: &str,
    reason: &str,
) -> (String, std::path::PathBuf) {
    let (status, stdout) = run(
        root,
        &[
            "case",
            "create",
            "--name",
            name,
            "--group",
            "support",
            "--message",
            message_id,
            "--reason",
            reason,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let case_uid = value["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty(), "{value}");
    let case_path = value["case_path"].as_str().unwrap_or_default();
    assert!(!case_path.is_empty(), "{value}");
    (case_uid, root.join(case_path))
}

fn create_notification(
    root: &std::path::Path,
    name: &str,
    message_id: &str,
    summary: &str,
    reason: &str,
) -> (String, std::path::PathBuf, serde_json::Value) {
    let (status, stdout) = run(
        root,
        &[
            "notification",
            "create",
            "--name",
            name,
            "--message",
            message_id,
            "--summary",
            summary,
            "--reason",
            reason,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let notification_uid = value["notification_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!notification_uid.is_empty(), "{value}");
    let path = value["path"].as_str().unwrap_or_default();
    assert!(!path.is_empty(), "{value}");
    (notification_uid, root.join(path), value)
}

fn current_push_plan_id(cwd: &std::path::Path) -> String {
    let (status, stdout) = run(cwd, &["push"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    value["plan_id"].as_str().unwrap_or_default().to_string()
}

fn run_confirmed_push(cwd: &std::path::Path) -> (i32, String) {
    let plan_id = current_push_plan_id(cwd);
    run(cwd, &["push", "--confirm", plan_id.as_str()])
}

fn force_first_move_step_remote_outcome_unknown(root: &std::path::Path) {
    let (plan_status, plan_stdout) = run(root, &["push"]);
    assert_eq!(plan_status, 0, "{plan_stdout}");
    let plan = parse_one(&plan_stdout);
    assert_eq!(plan["count"], 1, "{plan}");
    let planned_item = &plan["items"][0];

    let queued = parse_one(&run(root, &["push", "list"]).1);
    assert_eq!(queued["count"], 1, "{queued}");
    let push_id = queued["items"][0]["push_id"].as_str().unwrap_or_default();
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let mut push_item = read_json(&push_path);
    push_item["message_action_snapshot"] = json!({
        "action": planned_item["action"].clone(),
        "message_ids": planned_item["message_ids"].clone(),
        "locations": planned_item["locations"].clone(),
        "steps": planned_item["steps"].clone()
    });
    push_item["attempt_count"] = json!(2);
    push_item["step_states"] = json!([{
        "index": 0,
        "label": "move_to_mailbox_id_archive",
        "status": "remote_outcome_unknown",
        "started_rfc3339": "2026-07-11T00:00:00Z",
        "result_summary": "remote outcome unknown",
        "error_code": "imap_uid_missing",
        "error": "moved message uid was not found",
        "retryable": false
    }]);
    push_item["last_error"] = json!(
        "push_remote_outcome_unknown: remote outcome is unknown for push step move_to_mailbox_id_archive; refusing to retry automatically"
    );
    write_json(&push_path, &push_item);
}

fn confirmed_push_items(value: &Value) -> &[Value] {
    assert_eq!(value["confirmed"], true, "{value}");
    value["succeeded_items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

#[cfg(feature = "smtp")]
fn run_confirmed_push_with_global_args(
    cwd: &std::path::Path,
    extra_args: &[&str],
) -> (i32, String) {
    let plan_id = current_push_plan_id(cwd);
    let mut args = vec!["push", "--confirm", plan_id.as_str()];
    args.extend(extra_args);
    run(cwd, &args)
}

#[cfg(feature = "smtp")]
fn run_confirmed_push_with_env(cwd: &std::path::Path, envs: &[(&str, &str)]) -> (i32, String) {
    let plan_id = current_push_plan_id(cwd);
    let output = Command::new(env!("CARGO_BIN_EXE_afmail"))
        .current_dir(cwd)
        .args(["push", "--confirm", plan_id.as_str()])
        .envs(envs.iter().copied())
        .output();
    assert!(output.is_ok());
    let output = match output {
        Ok(value) => value,
        Err(_) => return (99, String::new()),
    };
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    assert!(
        stdout.is_empty() || stderr.is_empty(),
        "default split routing produced both streams\nstdout: {stdout}\nstderr: {stderr}"
    );
    (
        output.status.code().unwrap_or(99),
        if stdout.is_empty() { stderr } else { stdout },
    )
}

#[cfg(feature = "smtp")]
fn parse_lines(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_one)
        .collect()
}

#[cfg(feature = "smtp")]
fn parse_raw_json_lines(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn sent_message_id_for_push_id(push_id: &str) -> String {
    let suffix = push_id
        .strip_prefix("push_")
        .unwrap_or(push_id)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("message_sent_{suffix}")
}

fn workspace_runtime_lock_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".afmail/cache/workspace.lock")
}

fn configure_default_identity(root: &std::path::Path) {
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["identities"] =
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}]);
    write_json(&root.join(".afmail/config.json"), &config);
}

#[cfg(feature = "smtp")]
fn configure_smtp_send_only(root: &std::path::Path, smtp_port: u16) {
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["draft"]["send"]["steps"] = json!([{"smtp_send": {}}]);
    config["smtp"]["host"] = json!("127.0.0.1");
    config["smtp"]["port"] = json!(smtp_port);
    config["smtp"]["starttls"] = json!(false);
    config["smtp"]["tls_wrapper"] = json!(false);
    config["smtp"]["username"] = json!(null);
    config["smtp"]["password_secret"] = json!(null);
    config["identities"] =
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}]);
    write_json(&root.join(".afmail/config.json"), &config);
}

#[cfg(feature = "smtp")]
fn configure_smtp_then_sent_append(root: &std::path::Path, smtp_port: u16) {
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["draft"]["send"]["steps"] = json!([
        {"smtp_send": {}},
        {"append_to_mailbox_id": "sent"}
    ]);
    config["smtp"]["host"] = json!("127.0.0.1");
    config["smtp"]["port"] = json!(smtp_port);
    config["smtp"]["starttls"] = json!(false);
    config["smtp"]["tls_wrapper"] = json!(false);
    config["smtp"]["username"] = json!(null);
    config["smtp"]["password_secret"] = json!(null);
    config["identities"] =
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}]);
    write_json(&root.join(".afmail/config.json"), &config);
}

fn configure_imap_without_smtp(root: &std::path::Path, imap_port: u16) {
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["imap"]["host"] = json!("127.0.0.1");
    config["imap"]["port"] = json!(imap_port);
    config["imap"]["tls"] = json!(false);
    config["imap"]["allow_unbounded_body_fetch"] = json!(true);
    config["imap"]["username"] = json!("user");
    config["imap"]["password_secret"] = json!("pass");
    config["smtp"]["host"] = json!(null);
    config["smtp"]["port"] = json!(587);
    config["smtp"]["starttls"] = json!(false);
    config["smtp"]["tls_wrapper"] = json!(false);
    config["smtp"]["username"] = json!(null);
    config["smtp"]["password_secret"] = json!(null);
    config["identities"] =
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}]);
    write_json(&root.join(".afmail/config.json"), &config);
}

fn configure_spam_without_seen(root: &std::path::Path, imap_port: Option<u16>) {
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["message"]["spam"]["steps"] =
        json!([{"add_flags": ["$Junk"]}, {"move_to_mailbox_id": "junk"}]);
    if let Some(port) = imap_port {
        config["imap"]["host"] = json!("127.0.0.1");
        config["imap"]["port"] = json!(port);
        config["imap"]["tls"] = json!(false);
        config["imap"]["allow_unbounded_body_fetch"] = json!(true);
        config["imap"]["username"] = json!("user");
        config["imap"]["password_secret"] = json!("pass");
    }
    write_json(&root.join(".afmail/config.json"), &config);
}

#[cfg(feature = "smtp")]
fn configure_remote(root: &std::path::Path, imap_port: u16, smtp_port: u16) {
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["imap"]["host"] = json!("127.0.0.1");
    config["imap"]["port"] = json!(imap_port);
    config["imap"]["tls"] = json!(false);
    config["imap"]["allow_unbounded_body_fetch"] = json!(true);
    config["imap"]["username"] = json!("user");
    config["imap"]["password_secret"] = json!("pass");
    config["smtp"]["host"] = json!("127.0.0.1");
    config["smtp"]["port"] = json!(smtp_port);
    config["smtp"]["starttls"] = json!(false);
    config["smtp"]["tls_wrapper"] = json!(false);
    config["smtp"]["username"] = json!(null);
    config["smtp"]["password_secret"] = json!(null);
    config["identities"] =
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}]);
    write_json(&root.join(".afmail/config.json"), &config);
}

#[cfg(feature = "smtp")]
fn configure_remote_send_only(root: &std::path::Path, imap_port: u16, smtp_port: u16) {
    configure_remote(root, imap_port, smtp_port);
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["draft"]["send"]["steps"] = json!([{"smtp_send": {}}]);
    write_json(&root.join(".afmail/config.json"), &config);
}

struct ImapTestServer {
    addr: SocketAddr,
    // Read only by the SMTP send tests; the server records appends in every build.
    #[cfg_attr(not(feature = "smtp"), allow(dead_code))]
    appended: AppendedMessages,
    commands: ImapCommands,
    copied: CopiedMessages,
    expunged: ExpungedMessages,
    moved: MovedMessages,
    stored: StoredMessages,
    handle: thread::JoinHandle<()>,
}

// Only the SMTP recovery tests ask for anything but `Found`.
#[cfg_attr(not(feature = "smtp"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default)]
enum SearchResponse {
    #[default]
    Found,
    Absent,
    DraftOnly,
    Ambiguous,
    Failed,
}

#[derive(Clone, Copy, Default)]
struct ImapServerCapabilities {
    search: SearchResponse,
    reject_append_before_literal: bool,
    wait_for_all_connections: bool,
    move_supported: bool,
    uidplus_supported: bool,
}

fn start_imap_server(accept_count: usize) -> Option<ImapTestServer> {
    start_imap_server_with_capabilities(
        accept_count,
        ImapServerCapabilities {
            move_supported: true,
            uidplus_supported: false,
            ..Default::default()
        },
    )
}

fn start_imap_server_with_capabilities(
    accept_count: usize,
    capabilities: ImapServerCapabilities,
) -> Option<ImapTestServer> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let addr = listener.local_addr().ok()?;
    let appended = Arc::new(Mutex::new(Vec::new()));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let copied = Arc::new(Mutex::new(Vec::new()));
    let expunged = Arc::new(Mutex::new(Vec::new()));
    let moved = Arc::new(Mutex::new(Vec::new()));
    let stored = Arc::new(Mutex::new(Vec::new()));
    let server_appended = Arc::clone(&appended);
    let server_commands = Arc::clone(&commands);
    let server_copied = Arc::clone(&copied);
    let server_expunged = Arc::clone(&expunged);
    let server_moved = Arc::clone(&moved);
    let server_stored = Arc::clone(&stored);
    let handle = thread::spawn(move || {
        let _ = listener.set_nonblocking(true);
        let mut served = 0usize;
        let mut last = Instant::now();
        while served < accept_count {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    serve_imap_connection(
                        stream,
                        &server_appended,
                        &server_commands,
                        &server_copied,
                        &server_expunged,
                        &server_moved,
                        &server_stored,
                        capabilities,
                    );
                    served += 1;
                    last = Instant::now();
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    // The first connection waits longer: the clock starts
                    // before the test's `run` queues on the shared command
                    // lock, which other tests can hold for many seconds.
                    let idle_limit = if served == 0 {
                        Duration::from_secs(60)
                    } else {
                        Duration::from_millis(800)
                    };
                    // Multi-attempt tests can wait behind other CLI tests' lock.
                    // Their scripted next connection must not lose a race with
                    // the normal short idle shutdown between attempts.
                    if last.elapsed() > idle_limit
                        && !(served > 0 && capabilities.wait_for_all_connections)
                    {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    Some(ImapTestServer {
        addr,
        appended,
        commands,
        copied,
        expunged,
        moved,
        stored,
        handle,
    })
}

#[allow(clippy::too_many_arguments)]
fn serve_imap_connection(
    mut writer: TcpStream,
    appended: &AppendedMessages,
    commands: &ImapCommands,
    copied: &CopiedMessages,
    expunged: &ExpungedMessages,
    moved: &MovedMessages,
    stored: &StoredMessages,
    capabilities: ImapServerCapabilities,
) {
    let reader_stream = match writer.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    // Only a client that stalls mid-session reaches this; a loaded parallel
    // run can keep afmail busy between two commands for several seconds.
    let _ = writer.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(reader_stream);
    if write_imap(&mut writer, b"* OK afmail test imap ready\r\n").is_err() {
        return;
    }
    let mut selected_mailbox = String::from("INBOX");
    loop {
        let mut line = Vec::new();
        let Ok(read) = reader.read_until(b'\n', &mut line) else {
            return;
        };
        if read == 0 {
            return;
        }
        if capabilities.reject_append_before_literal
            && String::from_utf8_lossy(&line).contains(" APPEND ")
        {
            let tag = tag_from_line(&line).unwrap_or_else(|| "bad".to_string());
            let _ = write_imap(
                &mut writer,
                format!("{tag} NO [OVERQUOTA] Drafts quota exceeded\r\n").as_bytes(),
            );
            continue;
        }
        if handle_append_command(&mut writer, &mut reader, &line, appended).is_some() {
            continue;
        }
        if String::from_utf8_lossy(&line)
            .to_ascii_uppercase()
            .contains(" UID EXPUNGE ")
        {
            let tag = tag_from_line(&line).unwrap_or_else(|| "bad".to_string());
            if let Ok(mut locked) = commands.lock() {
                locked.push("EXPUNGE".to_string());
            }
            if !capabilities.uidplus_supported {
                let _ = write_imap(
                    &mut writer,
                    format!("{tag} BAD UID EXPUNGE unsupported\r\n").as_bytes(),
                );
                return;
            }
            if let Some(uid) = uid_expunge_arg_from_line(&line)
                && let Ok(mut locked) = expunged.lock()
            {
                locked.push((selected_mailbox.clone(), uid));
            }
            if write_imap(
                &mut writer,
                format!("* 1 EXPUNGE\r\n{tag} OK EXPUNGE completed\r\n").as_bytes(),
            )
            .is_err()
            {
                return;
            }
            continue;
        }
        let Some((tag, name)) = parse_imap_command(&line) else {
            let tag = tag_from_line(&line).unwrap_or_else(|| "bad".to_string());
            let _ = write_imap(
                &mut writer,
                format!("{tag} BAD unsupported command\r\n").as_bytes(),
            );
            return;
        };
        if let Ok(mut locked) = commands.lock() {
            locked.push(name.clone());
        }
        match name.as_str() {
            "LOGIN" => {
                if write_imap(&mut writer, format!("{tag} OK Logged in\r\n").as_bytes()).is_err() {
                    return;
                }
            }
            "CAPABILITY" => {
                let mut caps = vec!["IMAP4rev1"];
                if capabilities.move_supported {
                    caps.push("MOVE");
                }
                if capabilities.uidplus_supported {
                    caps.push("UIDPLUS");
                }
                let response = format!(
                    "* CAPABILITY {}\r\n{tag} OK CAPABILITY completed\r\n",
                    caps.join(" ")
                );
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "LIST" => {
                let response = format!(
                    "* LIST (\\Noinferiors) \"/\" \"INBOX\"\r\n* LIST (\\Sent) \"/\" \"Sent\"\r\n* LIST (\\Drafts) \"/\" \"Drafts\"\r\n* LIST (\\Archive) \"/\" \"Archive\"\r\n* LIST (\\Junk) \"/\" \"Junk\"\r\n* LIST (\\Trash) \"/\" \"Trash\"\r\n{tag} OK LIST completed\r\n"
                );
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "SELECT" | "EXAMINE" => {
                if let Some(folder) = folder_arg_from_line(&line, &name) {
                    selected_mailbox = folder;
                }
                let response = format!(
                    "* FLAGS ()\r\n* 0 EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY 44] UIDs valid\r\n* OK [UIDNEXT 100] Next UID\r\n{tag} OK [READ-WRITE] Select completed\r\n"
                );
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "SEARCH" => {
                let response = match capabilities.search {
                    SearchResponse::Found => {
                        format!("* SEARCH 900\r\n{tag} OK SEARCH completed\r\n")
                    }
                    SearchResponse::DraftOnly if selected_mailbox == "Drafts" => {
                        format!("* SEARCH 900\r\n{tag} OK SEARCH completed\r\n")
                    }
                    SearchResponse::DraftOnly | SearchResponse::Absent => {
                        format!("* SEARCH\r\n{tag} OK SEARCH completed\r\n")
                    }
                    SearchResponse::Ambiguous => {
                        format!("* SEARCH 900 901\r\n{tag} OK SEARCH completed\r\n")
                    }
                    SearchResponse::Failed => format!("{tag} NO [UNAVAILABLE] search failed\r\n"),
                };
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "STORE" => {
                if let Some((uid, query)) = uid_store_args_from_line(&line)
                    && let Ok(mut locked) = stored.lock()
                {
                    locked.push((selected_mailbox.clone(), uid, query));
                }
                if write_imap(
                    &mut writer,
                    format!("{tag} OK STORE completed\r\n").as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            "FETCH" => {
                if write_imap(
                    &mut writer,
                    format!("{tag} OK FETCH completed\r\n").as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            "COPY" => {
                if let Some((uid, target)) = uid_copy_args_from_line(&line)
                    && let Ok(mut locked) = copied.lock()
                {
                    locked.push((selected_mailbox.clone(), uid, target));
                }
                if write_imap(
                    &mut writer,
                    format!("{tag} OK COPY completed\r\n").as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            "EXPUNGE" => {
                if !capabilities.uidplus_supported {
                    let _ = write_imap(
                        &mut writer,
                        format!("{tag} BAD UID EXPUNGE unsupported\r\n").as_bytes(),
                    );
                    return;
                }
                if let Some(uid) = uid_expunge_arg_from_line(&line)
                    && let Ok(mut locked) = expunged.lock()
                {
                    locked.push((selected_mailbox.clone(), uid));
                }
                if write_imap(
                    &mut writer,
                    format!("* 1 EXPUNGE\r\n{tag} OK EXPUNGE completed\r\n").as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            "MOVE" => {
                if !capabilities.move_supported {
                    let _ = write_imap(
                        &mut writer,
                        format!("{tag} BAD MOVE unsupported\r\n").as_bytes(),
                    );
                    return;
                }
                if let Some((uid, target)) = uid_move_args_from_line(&line)
                    && let Ok(mut locked) = moved.lock()
                {
                    locked.push((selected_mailbox.clone(), uid, target));
                }
                if write_imap(
                    &mut writer,
                    format!("{tag} OK MOVE completed\r\n").as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            "LOGOUT" => {
                let response = format!("* BYE Logging out\r\n{tag} OK Logout completed\r\n");
                let _ = write_imap(&mut writer, response.as_bytes());
                return;
            }
            _ => {
                let response = format!("{tag} BAD unsupported command\r\n");
                let _ = write_imap(&mut writer, response.as_bytes());
                return;
            }
        }
    }
}

fn handle_append_command(
    writer: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    line: &[u8],
    appended: &AppendedMessages,
) -> Option<()> {
    let text = String::from_utf8_lossy(line);
    if !text.contains(" APPEND ") {
        return None;
    }
    let tag = tag_from_line(line).unwrap_or_else(|| "bad".to_string());
    let folder = folder_arg_from_line(line, "APPEND").unwrap_or_default();
    let draft = text.contains("\\Draft");
    let len = text
        .rsplit_once('{')
        .and_then(|(_, rest)| rest.split_once('}'))
        .and_then(|(digits, _)| digits.parse::<usize>().ok())
        .unwrap_or(0);
    if write_imap(writer, b"+ go ahead\r\n").is_err() {
        return Some(());
    }
    let mut data = vec![0u8; len + 2];
    if std::io::Read::read_exact(reader, &mut data).is_err() {
        return Some(());
    }
    let content = String::from_utf8_lossy(&data[..len]).to_string();
    if let Ok(mut locked) = appended.lock() {
        locked.push((folder, draft, content));
    }
    let _ = write_imap(
        writer,
        format!("{tag} OK [APPENDUID 44 900] APPEND completed\r\n").as_bytes(),
    );
    Some(())
}

fn folder_arg_from_line(line: &[u8], command: &str) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    let (_, rest) = text.split_once(command)?;
    let rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let (folder, _) = stripped.split_once('"')?;
        Some(folder.to_string())
    } else {
        rest.split_whitespace().next().map(ToString::to_string)
    }
}

fn uid_store_args_from_line(line: &[u8]) -> Option<(u64, String)> {
    let text = String::from_utf8_lossy(line);
    let upper = text.to_ascii_uppercase();
    let marker = " UID STORE ";
    let start = upper.find(marker)? + marker.len();
    let rest = text.get(start..)?.trim();
    let (uid_text, query) = rest.split_once(' ')?;
    Some((uid_text.parse::<u64>().ok()?, query.trim().to_string()))
}

fn uid_copy_args_from_line(line: &[u8]) -> Option<(u64, String)> {
    let text = String::from_utf8_lossy(line);
    let upper = text.to_ascii_uppercase();
    let marker = " UID COPY ";
    let start = upper.find(marker)? + marker.len();
    let rest = text.get(start..)?.trim();
    let mut parts = rest.split_whitespace();
    let uid = parts.next()?.parse::<u64>().ok()?;
    let target = if let Some(stripped) = rest.split_once(' ')?.1.trim_start().strip_prefix('"') {
        stripped.split_once('"')?.0.to_string()
    } else {
        parts.next()?.trim().to_string()
    };
    Some((uid, target))
}

fn uid_expunge_arg_from_line(line: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(line);
    let upper = text.to_ascii_uppercase();
    let marker = " UID EXPUNGE ";
    let start = upper.find(marker)? + marker.len();
    text.get(start..)?.trim().parse::<u64>().ok()
}

fn uid_move_args_from_line(line: &[u8]) -> Option<(u64, String)> {
    let text = String::from_utf8_lossy(line);
    let upper = text.to_ascii_uppercase();
    let marker = " UID MOVE ";
    let start = upper.find(marker)? + marker.len();
    let rest = text.get(start..)?.trim();
    let mut parts = rest.split_whitespace();
    let uid = parts.next()?.parse::<u64>().ok()?;
    let target = if let Some(stripped) = rest.split_once(' ')?.1.trim_start().strip_prefix('"') {
        stripped.split_once('"')?.0.to_string()
    } else {
        parts.next()?.trim().to_string()
    };
    Some((uid, target))
}

fn parse_imap_command(line: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(line);
    let mut parts = text.split_whitespace();
    let tag = parts.next()?.to_string();
    let mut name = parts.next()?.to_ascii_uppercase();
    if name == "UID" {
        name = parts.next()?.to_ascii_uppercase();
    }
    Some((tag, name))
}

fn tag_from_line(line: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    text.split_whitespace().next().map(ToString::to_string)
}

fn write_imap(writer: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    writer.write_all(data)?;
    writer.flush()
}

#[cfg(feature = "smtp")]
struct SmtpTestServer {
    addr: SocketAddr,
    data_rx: mpsc::Receiver<String>,
    handle: thread::JoinHandle<()>,
}

#[cfg(feature = "smtp")]
fn start_smtp_server() -> Option<SmtpTestServer> {
    start_smtp_server_connections(1)
}

#[cfg(feature = "smtp")]
fn start_smtp_server_connections(connection_count: usize) -> Option<SmtpTestServer> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let addr = listener.local_addr().ok()?;
    let (data_tx, data_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        for _ in 0..connection_count {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            serve_smtp_connection(stream, data_tx.clone());
        }
    });
    Some(SmtpTestServer {
        addr,
        data_rx,
        handle,
    })
}

#[cfg(feature = "smtp")]
fn serve_smtp_connection(mut writer: TcpStream, data_tx: mpsc::Sender<String>) {
    let reader_stream = match writer.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let _ = writer.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(reader_stream);
    if write_smtp(&mut writer, b"220 afmail test smtp ready\r\n").is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        let Ok(read) = reader.read_line(&mut line) else {
            return;
        };
        if read == 0 {
            return;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            if write_smtp(&mut writer, b"250-localhost\r\n250 OK\r\n").is_err() {
                return;
            }
        } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
            if write_smtp(&mut writer, b"250 OK\r\n").is_err() {
                return;
            }
        } else if upper.starts_with("DATA") {
            if write_smtp(&mut writer, b"354 End data with <CR><LF>.<CR><LF>\r\n").is_err() {
                return;
            }
            let mut data = String::new();
            loop {
                let mut data_line = String::new();
                let Ok(read) = reader.read_line(&mut data_line) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                if data_line == ".\r\n" || data_line == ".\n" {
                    break;
                }
                data.push_str(&data_line);
            }
            let _ = data_tx.send(data);
            if write_smtp(&mut writer, b"250 queued\r\n").is_err() {
                return;
            }
        } else if upper.starts_with("QUIT") {
            let _ = write_smtp(&mut writer, b"221 bye\r\n");
            return;
        } else if write_smtp(&mut writer, b"250 OK\r\n").is_err() {
            return;
        }
    }
}

#[cfg(feature = "smtp")]
fn write_smtp(writer: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    writer.write_all(data)?;
    writer.flush()
}

fn write_case_fixture(
    root: &std::path::Path,
    group: &str,
    case_uid: &str,
    case_name: &str,
    message_ids: &[&str],
) -> std::path::PathBuf {
    let case_dir = root
        .join("cases")
        .join(group)
        .join(format!("{case_uid}-{case_name}"));
    assert!(fs::create_dir_all(case_dir.join("data")).is_ok());
    assert!(fs::create_dir_all(case_dir.join("drafts")).is_ok());
    assert!(fs::create_dir_all(case_dir.join("files")).is_ok());
    assert!(fs::create_dir_all(case_dir.join("views/messages")).is_ok());
    write_json(
        &case_dir.join("data/case.json"),
        &json!({
            "schema_name": "case",
            "schema_version": 3,
            "collection_uid": case_uid,
            "collection_name": case_name,
            "tags": [],
            "created_rfc3339": "2026-05-21T10:00:00Z",
            "updated_rfc3339": "2026-05-21T10:00:00Z",
            "items": message_ids
                .iter()
                .map(|message_id| json!({
                    "message_id": message_id,
                    "summary": "case fixture",
                    "added_rfc3339": "2026-05-21T10:00:00Z"
                }))
                .collect::<Vec<_>>()
        }),
    );
    assert!(fs::write(case_dir.join("case.md"), format!("# {case_name}\n")).is_ok());
    case_dir
}

#[test]
fn push_defaults_to_preview_and_rejects_dry_run_confirm() {
    let root = temp_root("push-preview");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_preview", 42, None);
    write_triage(&root, "message_preview", "phishing note");
    assert_eq!(
        run(
            &root,
            &[
                "message",
                "spam",
                "message_preview",
                "--reason",
                "phishing note",
            ],
        )
        .0,
        0
    );

    let (status, stdout) = run(&root, &["push"]);
    assert_eq!(status, 0, "{stdout}");
    let first = parse_one(&stdout);
    assert_eq!(first["confirmed"], false);
    assert_eq!(first["count"], 1);
    assert_eq!(first["schema_name"], "push_plan");
    assert_eq!(first["schema_version"], 1);
    assert!(first["workspace_uid"].as_str().is_some());
    assert!(first["workspace_path"].as_str().is_some());
    assert!(
        first["plan_id"]
            .as_str()
            .is_some_and(|plan_id| plan_id.starts_with("push_plan_"))
    );

    let (status, stdout) = run(&root, &["push"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["confirmed"], false);
    assert_eq!(value["count"], 1);
    assert_eq!(value["plan_id"], first["plan_id"]);
    assert!(
        value["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("No remote changes were made"))
    );
    let listed = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["plan_id"], first["plan_id"]);

    let (status, stdout) = run(&root, &["push", "--dry-run", "--confirm"]);
    assert_eq!(status, 2, "{stdout}");
    // Previewing and applying are separate registered shapes, so the pair is
    // rejected by the parser instead of by a hand-written check.
    let plan_id = first["plan_id"].as_str().unwrap_or_default();
    let (status, stdout) = run(&root, &["push", "--dry-run", "--confirm", plan_id]);
    assert_eq!(status, 2, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "cli_unregistered_combination");
    let (status, stdout) = run(&root, &["push", "--confirm", "push_plan_wrong"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_plan_changed");
    let _ = fs::remove_dir_all(root);
}

#[cfg(feature = "smtp")]
#[test]
fn push_failure_records_step_level_state() {
    let root = temp_root("push-step-state");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "step-state", "step state case");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Step state\"\nattachments:\n---\n\nHello\n"
    ));
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "new.md"]).0,
        0
    );

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "config_missing");
    assert_eq!(value["partially_applied_count"], 0);
    assert_eq!(value["pushed_count"], 0);
    assert_eq!(value["failed_count"], 1);

    let push = parse_one(&run(&root, &["push", "list"]).1);
    let item = &push["items"][0];
    assert_eq!(item["attempt_count"], 1);
    assert_eq!(
        item["step_states"][0]["label"],
        "append_to_mailbox_id_drafts"
    );
    assert_eq!(item["step_states"][0]["status"], "failed");
    assert!(item["step_states"][0]["error_code"].as_str().is_some());

    let drafts = parse_one(&run(&root, &["case", "draft", "list", &case_uid]).1);
    assert_eq!(drafts["drafts"][0]["status"], "failed");
    assert_eq!(drafts["drafts"][0]["queued_action"], "send");
    assert_eq!(drafts["drafts"][0]["remote_changed"], false);
    let _ = fs::remove_dir_all(root);
}

#[cfg(feature = "smtp")]
#[test]
fn push_remote_outcome_unknown_without_succeeded_steps_is_partial() {
    let root = temp_root("push-remote-outcome-unknown");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["draft"]["send"]["steps"] = json!([{"smtp_send": {}}]);
    write_json(&root.join(".afmail/config.json"), &config);
    let (case_uid, case_path) = create_case(&root, "unknown-outcome", "remote outcome test");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Unknown outcome\"\nattachments:\n---\n\nDo not resend blindly\n"
    ));
    assert!(fs::write(case_path.join("drafts/unknown.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "unknown.md"]).0,
        0
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let mut push_item = read_json(&push_path);
    push_item["step_states"] = json!([{
        "index": 0,
        "label": "smtp_send",
        "status": "remote_outcome_unknown",
        "started_rfc3339": "2026-07-11T00:00:00Z",
        "result_summary": "remote outcome unknown",
        "error_code": "smtp_response_unknown",
        "error": "connection ended after DATA",
        "retryable": false
    }]);
    write_json(&push_path, &push_item);

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 3, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "partial");
    assert_eq!(value["pushed_count"], 0);
    assert_eq!(value["failed_count"], 1);
    assert_eq!(value["partially_applied_count"], 1);
    assert_eq!(
        value["failures"][0]["error"]["code"],
        "push_remote_outcome_unknown"
    );
    assert_eq!(value["failures"][0]["succeeded_step_count"], 0);
    assert_eq!(value["failures"][0]["partially_applied"], true);
    assert_eq!(
        value["failures"][0]["error"]["cause_error"],
        "connection ended after DATA"
    );
    let queued = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(
        queued["items"][0]["step_states"][0]["status"],
        "remote_outcome_unknown"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_move_target_uid_unknown_recovers_pulled_target_without_replaying_move() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("push-move-target-uid-recovery");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_move_recovery", 42, None);
    write_triage(&root, "message_move_recovery", "archive after reply");
    create_notification(
        &root,
        "completed",
        "message_move_recovery",
        "completed request",
        "request completed",
    );
    force_first_move_step_remote_outcome_unknown(&root);
    update_message_json(&root, "message_move_recovery", |message| {
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations[0]["missing_rfc3339"] = json!("2026-07-11T00:01:00Z");
            locations.push(json!({
                "mailbox_name": "Archive",
                "mailbox_id": "archive",
                "uid_validity": 55,
                "uid": 193,
                "flags": ["\\Seen"],
                "observed_rfc3339": "2026-07-11T00:01:00Z"
            }));
        }
    });

    let server = start_imap_server(3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_imap_without_smtp(&root, server.addr.port());

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    let message = read_json(&root.join("messages/message_move_recovery.json"));
    let locations = message["remote"]["locations"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    assert_eq!(locations.len(), 1, "{message}");
    assert_eq!(locations[0]["mailbox_name"], "Archive");
    assert_eq!(locations[0]["uid_validity"], 55);
    assert_eq!(locations[0]["uid"], 193);

    assert!(server.handle.join().is_ok());
    let moved = server
        .moved
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(moved.is_empty(), "recovery replayed UID MOVE: {moved:?}");
    let commands = server
        .commands
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(
        !commands.iter().any(|command| command == "MOVE"),
        "recovery replayed UID MOVE: {commands:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_move_target_uid_unknown_waits_until_every_planned_source_is_reconciled() {
    let root = temp_root("push-move-source-still-active");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = read_json(&root.join(".afmail/config.json"));
    let archive_rule = json!({
        "steps": [{"move_to_mailbox_id": "archive"}]
    });
    config["mailboxes"]["inbox"] = json!({
        "resolve": [{"mailbox_name": "INBOX"}]
    });
    config["mailboxes"]["archive"] = json!({
        "resolve": [{"mailbox_name": "Archive"}]
    });
    config["mailboxes"]["secondary"] = json!({
        "resolve": [{"mailbox_name": "Secondary"}]
    });
    config["actions"]["message"]["notification"]["by_source_mailbox_id"]["inbox"] =
        archive_rule.clone();
    config["actions"]["message"]["notification"]["by_source_mailbox_id"]["secondary"] =
        archive_rule;
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_move_partial", 42, None);
    update_message_json(&root, "message_move_partial", |message| {
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations.push(json!({
                "mailbox_name": "Secondary",
                "mailbox_id": "secondary",
                "uid_validity": 45,
                "uid": 88,
                "flags": ["\\Seen"],
                "observed_rfc3339": "2026-07-11T00:00:00Z"
            }));
        }
    });
    write_triage(&root, "message_move_partial", "archive after reply");
    create_notification(
        &root,
        "completed-partial",
        "message_move_partial",
        "completed request",
        "request completed",
    );
    force_first_move_step_remote_outcome_unknown(&root);
    update_message_json(&root, "message_move_partial", |message| {
        if let Some(locations) = message["remote"]["locations"].as_array_mut() {
            locations[0]["missing_rfc3339"] = json!("2026-07-11T00:01:00Z");
            locations.push(json!({
                "mailbox_name": "Archive",
                "mailbox_id": "archive",
                "uid_validity": 55,
                "uid": 194,
                "flags": ["\\Seen"],
                "observed_rfc3339": "2026-07-11T00:01:00Z"
            }));
        }
    });

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 3, "{stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["code"], "partial", "{result}");
    assert_eq!(result["partially_applied_count"], 1, "{result}");
    assert_eq!(
        result["failures"][0]["error"]["code"], "push_remote_outcome_unknown",
        "{result}"
    );
    let queued = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(queued["count"], 1, "{queued}");
    assert_eq!(
        queued["items"][0]["step_states"][0]["status"], "remote_outcome_unknown",
        "{queued}"
    );
    let message = read_json(&root.join("messages/message_move_partial.json"));
    let locations = message["remote"]["locations"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    assert_eq!(locations.len(), 3, "{message}");
    assert!(locations.iter().any(|location| {
        location["mailbox_name"] == "Secondary" && location.get("missing_rfc3339").is_none()
    }));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_spam_rejects_when_message_becomes_referenced_before_sync() {
    let root = temp_root("spam-push-reference");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam", 42, None);
    write_triage(&root, "message_spam", "phishing note");
    assert_eq!(
        run(
            &root,
            &[
                "message",
                "spam",
                "message_spam",
                "--reason",
                "phishing note",
            ],
        )
        .0,
        0
    );
    let case_uid = "c20260521001";
    let _case_path = write_case_fixture(&root, "support", case_uid, "case-ref", &["message_spam"]);

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "message_referenced");
    assert_eq!(value["partially_applied_count"], 0);
    assert_eq!(value["pushed_count"], 0);
    assert_eq!(value["failed_count"], 1);
    assert_eq!(value["failures"][0]["error"]["code"], "message_referenced");
    assert!(
        value["failures"][0]["error"]["message"]
            .as_str()
            .is_some_and(|error| error
                .contains(&format!("cases/support/{case_uid}-case-ref/data/case.json")))
    );
    assert!(root.join("messages/message_spam.json").exists());
    let blocked = read_json(&root.join("messages/message_spam.json"));
    assert_eq!(blocked["remote"]["locations"][0]["mailbox_name"], "INBOX");
    assert!(
        blocked["workspace"]["push"]["pending"][0]["last_error"]
            .as_str()
            .is_some_and(|error| error.contains("message_referenced"))
    );
    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["count"], 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_spam_marks_local_message_and_queues_junk_move() {
    let root = temp_root("spam");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam", 42, None);
    write_triage(&root, "message_spam", "phishing note");

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "spam",
            "message_spam",
            "--reason",
            "phishing note",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value[0]["message_ids"][0], "message_spam");
    assert_eq!(value[0]["location_count"], 1);
    assert_eq!(value[0]["queued"], true);
    assert!(
        value[0]["push_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(!root.join("triage/message_spam.md").exists());
    assert!(root.join("messages/message_spam.json").exists());
    assert!(!root.join(".afmail/messages/message_spam.txt").exists());
    assert!(root.join(".afmail/messages/message_spam.eml").exists());
    assert!(!root.join("messages/message_junk_44_900.json").exists());
    assert!(root.join("spam/index.md").is_file());
    assert!(root.join("spam/message_spam.md").is_file());

    let spam_value = read_json(&root.join("messages/message_spam.json"));
    assert_eq!(spam_value["message_id"], "message_spam");
    assert_eq!(spam_value["workspace"]["status"], "spam");
    assert_eq!(
        spam_value["workspace"]["push"]["pending"][0]["kind"],
        "message.spam"
    );
    assert!(
        spam_value["workspace"]["push"]["pending"][0]["push_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("push_"))
    );
    assert!(!root.join(".afmail/messages/message_spam.notes.md").exists());
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"message_spam_marked\""));
    assert!(log.contains("\"reason\":\"phishing note\""));

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let push = parse_one(&stdout);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["kind"], "message");
    assert_eq!(push["items"][0]["action"], "spam");
    assert_eq!(
        push["items"][0]["steps"],
        json!([{"add_flags": ["\\Seen", "$Junk"]}, {"move_to_mailbox_id": "junk"}])
    );
    assert_eq!(push["items"][0]["message_ids"][0], "message_spam");
    assert_eq!(push["items"][0]["locations"][0]["mailbox_name"], "INBOX");
    assert_eq!(push["items"][0]["locations"][0]["uid"], 42);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_spam_accepts_multiple_message_ids() {
    let root = temp_root("spam-batch");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam_a", 42, None);
    write_triage(&root, "message_spam_a", "batch spam note");
    write_message(&root, "message_spam_b", 43, None);
    write_triage(&root, "message_spam_b", "batch spam note");

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "spam",
            "message_spam_a",
            "message_spam_b",
            "--reason",
            "batch spam cleanup",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value.as_array().map(Vec::len), Some(2));
    assert!(value[0].get("code").is_none());
    assert_eq!(value[1]["message_id"], "message_spam_b");
    assert!(!root.join("triage/message_spam_a.md").exists());
    assert!(!root.join("triage/message_spam_b.md").exists());
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2);
    assert!(push["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .all(|item| item["kind"] == "message" && item["action"] == "spam")
    }));

    let _ = fs::remove_dir_all(root);
}

fn discarded_message_with_pending_push(
    root: &std::path::Path,
    disposition: &str,
) -> std::path::PathBuf {
    assert!(fs::create_dir_all(root).is_ok());
    assert_eq!(run(root, &["init"]).0, 0);
    assert_eq!(
        run(root, &["config", "set", "imap.host", "imap.example.com"]).0,
        0
    );
    write_message(root, "message_discard", 44, None);
    write_triage(root, "message_discard", "discard fixture");
    let (status, stdout) = run(
        root,
        &[
            "message",
            disposition,
            "message_discard",
            "--reason",
            "discard fixture",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let queue = parse_one(&run(root, &["push", "list"]).1);
    let push_id = queue["items"][0]["push_id"].as_str().unwrap_or_default();
    assert!(!push_id.is_empty(), "{queue}");
    root.join(format!(".afmail/push/{push_id}.json"))
}

#[test]
fn case_add_refuses_a_started_push_before_writing_the_case() {
    let root = temp_root("case-add-started");
    let push_path = discarded_message_with_pending_push(&root, "spam");
    let mut item = read_json(&push_path);
    item["attempt_count"] = json!(1);
    item["step_states"] = json!([{
        "index": 0,
        "label": "add_flags",
        "status": "remote_outcome_unknown",
        "started_rfc3339": "2026-10-09T11:00:00Z",
        "retryable": false,
    }]);
    write_json(&push_path, &item);
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["case"]["add"]["steps"] = json!([{"add_flags": ["$Case"]}]);
    write_json(&root.join(".afmail/config.json"), &config);
    let (case_uid, case_path) = create_case(&root, "started-push", "case fixture");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_discard",
            "--reason",
            "belongs to the case",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_already_started");
    // `case create --message` takes the same check before creating anything.
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "create",
            "--name",
            "started-push-create",
            "--group",
            "support",
            "--message",
            "message_discard",
            "--reason",
            "belongs to a new case",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_already_started");
    assert_eq!(parse_one(&run(&root, &["doctor"]).1)["issue_count"], 0);
    // Refused before any write: the case is untouched and the message keeps
    // its spam disposition rather than ending up half-moved.
    let case_data = read_json(&case_path.join("data/case.json"));
    assert_eq!(
        case_data["items"].as_array().map(Vec::len),
        Some(0),
        "{case_data}"
    );
    assert!(root.join("spam/message_discard.md").is_file());
    assert_eq!(
        read_json(&root.join("messages/message_discard.json"))["workspace"]["status"],
        "spam"
    );
    let _ = fs::remove_dir_all(root);
}

fn write_legacy_discard_undo_transaction(root: &std::path::Path, kind: &str) -> std::path::PathBuf {
    let transaction_id = format!("transaction_20261009T110151Z_{kind}");
    let path = root.join(format!(".afmail/transactions/{transaction_id}.json"));
    write_json(
        &path,
        &json!({
            "schema_name": "local_transaction",
            "schema_version": 1,
            "transaction_id": transaction_id,
            "kind": kind,
            "created_rfc3339": "2026-10-09T11:01:51Z",
            "paths": ["messages/message_discard.json", ".afmail/push"],
        }),
    );
    path
}

#[test]
fn discard_undo_started_push_rejection_and_legacy_repair_preserve_the_original_action() {
    for (disposition, undo, transaction_kind, expected_status) in [
        ("spam", "unspam", "message_unspammed", "spam"),
        ("trash", "untrash", "message_untrashed", "trashed"),
    ] {
        let root = temp_root("discard-undo-started");
        let push_path = discarded_message_with_pending_push(&root, disposition);
        let mut item = read_json(&push_path);
        item["attempt_count"] = json!(1);
        item["step_states"] = json!([{
            "index": 0,
            "label": if disposition == "spam" { "add_flags" } else { "move_to_mailbox_id_trash" },
            "status": "remote_outcome_unknown",
            "started_rfc3339": "2026-10-09T11:00:00Z",
            "retryable": false,
        }]);
        write_json(&push_path, &item);

        let (status, stdout) = run(
            &root,
            &[
                "message",
                undo,
                "message_discard",
                "--reason",
                "undo fixture",
            ],
        );
        assert_eq!(status, 1, "{stdout}");
        assert_eq!(parse_one(&stdout)["code"], "push_already_started");
        assert_eq!(parse_one(&run(&root, &["doctor"]).1)["issue_count"], 0);

        let transaction_path = write_legacy_discard_undo_transaction(&root, transaction_kind);
        let doctor = parse_one(&run(&root, &["doctor"]).1);
        assert_eq!(doctor["repairable_count"], 1, "{doctor}");
        let (status, stdout) = run(&root, &["doctor", "repair", "--confirm"]);
        assert_eq!(status, 0, "{stdout}");
        assert_eq!(parse_one(&stdout)["after_issue_count"], 0);
        assert!(!transaction_path.exists());
        assert_eq!(
            read_json(&push_path),
            item,
            "the original push must remain intact"
        );
        assert_eq!(
            read_json(&root.join("messages/message_discard.json"))["workspace"]["status"],
            expected_status,
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn doctor_refuses_discard_undo_repair_without_evidence_of_a_started_original_push() {
    for (disposition, transaction_kind) in [
        ("spam", "message_unspammed"),
        ("trash", "message_untrashed"),
    ] {
        let root = temp_root("discard-undo-unproven");
        let push_path = discarded_message_with_pending_push(&root, disposition);
        let original = read_json(&push_path);
        let transaction_path = write_legacy_discard_undo_transaction(&root, transaction_kind);
        let doctor = parse_one(&run(&root, &["doctor"]).1);
        assert_eq!(doctor["repairable_count"], 0, "{doctor}");
        let (status, stdout) = run(&root, &["doctor", "repair", "--confirm"]);
        assert_eq!(status, 1, "{stdout}");
        assert_eq!(parse_one(&stdout)["code"], "transaction_incomplete");
        assert!(transaction_path.exists());
        assert_eq!(read_json(&push_path), original);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn message_unspam_untrash_restore_local_state_and_queue_remote_moves() {
    let root = temp_root("unspam-untrash");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    write_message(&root, "message_spam_undo", 44, None);
    write_triage(&root, "message_spam_undo", "spam mistake");
    let spam = parse_one(
        &run(
            &root,
            &[
                "message",
                "spam",
                "message_spam_undo",
                "--reason",
                "looks bad",
            ],
        )
        .1,
    );
    let spam_push_id = spam[0]["push_id"].as_str().unwrap_or_default().to_string();
    assert!(!spam_push_id.is_empty());
    assert!(!root.join("triage/message_spam_undo.md").exists());

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "unspam",
            "message_spam_undo",
            "--reason",
            "not spam",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value[0]["removed_push_count"], 1);
    assert_eq!(value[0]["removed_push_ids"], json!([spam_push_id]));
    assert_eq!(value[0]["queued"], false);
    assert_eq!(value[0]["location_count"], 0);
    assert!(root.join("triage/message_spam_undo.md").exists());
    let message = read_json(&root.join("messages/message_spam_undo.json"));
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"].get("push").is_none() || message["workspace"]["push"].is_null());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    write_message(&root, "message_trash_undo", 45, None);
    write_triage(&root, "message_trash_undo", "trash mistake");
    let trash = parse_one(
        &run(
            &root,
            &[
                "message",
                "trash",
                "message_trash_undo",
                "--reason",
                "discard",
            ],
        )
        .1,
    );
    let trash_push_id = trash[0]["push_id"].as_str().unwrap_or_default().to_string();
    assert!(!trash_push_id.is_empty());
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "untrash",
            "message_trash_undo",
            "--reason",
            "keep it",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value[0]["removed_push_ids"], json!([trash_push_id]));
    assert_eq!(value[0]["queued"], false);
    assert!(root.join("triage/message_trash_undo.md").exists());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    write_message(&root, "message_remote_junk", 46, None);
    write_triage(&root, "message_remote_junk", "false positive");
    update_message_json(&root, "message_remote_junk", |value| {
        value["workspace"]["status"] = json!("spam");
        value["remote"]["locations"][0]["mailbox_name"] = json!("Junk");
        value["remote"]["locations"][0]["mailbox_id"] = json!("junk");
        value["remote"]["locations"][0]["uid"] = json!(46);
    });
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "unspam",
            "message_remote_junk",
            "--reason",
            "false positive",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value[0]["removed_push_count"], 0);
    assert_eq!(value[0]["queued"], true);
    assert!(
        value[0]["push_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(value[0]["location_count"], 1);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["action"], "unspam");
    assert_eq!(
        push["items"][0]["steps"],
        json!([{"move_to_mailbox_id": "inbox"}])
    );
    assert_eq!(push["items"][0]["locations"][0]["mailbox_name"], "Junk");
    assert_eq!(push["items"][0]["locations"][0]["uid"], 46);

    write_message(&root, "message_remote_trash", 47, None);
    write_triage(&root, "message_remote_trash", "keep");
    update_message_json(&root, "message_remote_trash", |value| {
        value["workspace"]["status"] = json!("triage");
        value["remote"]["locations"][0]["mailbox_name"] = json!("Trash");
        value["remote"]["locations"][0]["mailbox_id"] = json!("trash");
        value["remote"]["locations"][0]["uid"] = json!(47);
    });
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "untrash",
            "message_remote_trash",
            "--reason",
            "not trash",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value[0]["from_status"], "triage");
    assert_eq!(value[0]["queued"], true);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2);
    assert!(push["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["action"] == "untrash"
                && item["locations"][0]["mailbox_name"] == "Trash"
                && item["locations"][0]["uid"] == 47
        })
    }));

    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"message_unspammed\""));
    assert!(log.contains("\"kind\":\"message_untrashed\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_spam_can_skip_marking_remote_seen_by_config() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("spam-push-unread");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_spam_without_seen(&root, None);
    write_message(&root, "message_spam", 42, None);
    write_triage(&root, "message_spam", "phishing note");
    assert_eq!(
        run(
            &root,
            &[
                "message",
                "spam",
                "message_spam",
                "--reason",
                "phishing note",
            ],
        )
        .0,
        0
    );
    let server = start_imap_server(3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_spam_without_seen(&root, Some(server.addr.port()));

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    assert!(server.handle.join().is_ok());
    let stored = server.stored.lock();
    assert!(stored.is_ok());
    let stored = stored.map(|items| items.clone()).unwrap_or_default();
    assert!(!stored.iter().any(|(_, _, query)| query.contains("\\Seen")));
    assert!(stored.iter().any(|(_, _, query)| query.contains("$Junk")));
    let moved = server.moved.lock();
    assert!(moved.is_ok());
    assert!(
        moved
            .map(|items| items
                .iter()
                .any(|(source, uid, target)| source == "INBOX" && *uid == 42 && target == "Junk"))
            .unwrap_or(false)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_drains_case_add_remote_steps() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("case-add-push");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let server = start_imap_server(3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_imap_without_smtp(&root, server.addr.port());
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["case"]["add"]["steps"] = json!([{"add_flags": ["$Case"]}]);
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_case_push", 42, None);
    write_triage(&root, "message_case_push", "case push note");
    let (case_uid, _) = create_case(&root, "case-push", "case with remote effect");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_case_push",
            "--reason",
            "belongs to the case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let plan = parse_one(&stdout);
    assert_eq!(plan["count"], 1, "{plan}");
    assert_eq!(plan["items"][0]["action"], "case", "{plan}");

    // A configured case.add effect is ordinary queued work: push applies it
    // and the queue empties, rather than the item lingering undrained.
    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    assert!(server.handle.join().is_ok());
    let stored = server
        .stored
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(
        stored.iter().any(|(mailbox, uid, query)| mailbox == "INBOX"
            && *uid == 42
            && query.contains("$Case")),
        "{stored:?}"
    );
    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["count"], 0, "{stdout}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_spam_moves_remote_to_junk_keeping_local_id() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("spam-push");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam", 42, None);
    write_triage(&root, "message_spam", "phishing note");
    assert_eq!(
        run(
            &root,
            &[
                "message",
                "spam",
                "message_spam",
                "--reason",
                "phishing note",
            ],
        )
        .0,
        0
    );

    let server = start_imap_server(3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_imap_without_smtp(&root, server.addr.port());

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert!(value.get("outcome").is_none());
    assert_eq!(confirmed_push_items(&value).len(), 1);
    assert!(value["succeeded_items"][0]["push_id"].as_str().is_some());
    let progress = read_json(&root.join(".afmail/cache/progress.json"));
    assert_eq!(progress["schema_name"], "workspace_progress");
    assert_eq!(progress["command"], "push");
    assert_eq!(progress["status"], "succeeded");
    assert_eq!(progress["result"]["pushed_count"], 1);
    assert_eq!(
        fs::read_dir(root.join(".afmail/push"))
            .map(|entries| entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("json"))
                .count())
            .unwrap_or(99),
        0
    );
    // The id is immutable: files keep their `message_spam` names; only the
    // recorded remote location moves to Junk. No `message_junk_*` file is created.
    assert!(root.join("messages/message_spam.json").exists());
    assert!(!root.join(".afmail/messages/message_spam.txt").exists());
    assert!(root.join(".afmail/messages/message_spam.eml").exists());
    assert!(!root.join("messages/message_junk_44_900.json").exists());
    let spam_value = read_json(&root.join("messages/message_spam.json"));
    assert_eq!(spam_value["message_id"], "message_spam");
    assert_eq!(spam_value["body_text"], "Body");
    assert_eq!(spam_value["eml_path"], ".afmail/messages/message_spam.eml");
    assert_eq!(spam_value["workspace"]["status"], "spam");
    assert_eq!(spam_value["remote"]["locations"][0]["mailbox_name"], "Junk");
    assert_eq!(spam_value["remote"]["locations"][0]["uid_validity"], 44);
    assert_eq!(spam_value["remote"]["locations"][0]["uid"], 900);
    assert_eq!(
        spam_value["remote"]["locations"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(
        spam_value["workspace"]["push"]
            .get("pending")
            .and_then(serde_json::Value::as_array)
            .is_none_or(Vec::is_empty)
    );
    assert!(
        spam_value["workspace"]["push"]["last_completed_rfc3339"]
            .as_str()
            .is_some()
    );
    assert!(!root.join(".afmail/messages/message_spam.notes.md").exists());
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"message_spam_marked\""));
    assert!(log.contains("\"reason\":\"phishing note\""));
    assert!(server.handle.join().is_ok());
    let moved = server.moved.lock();
    assert!(moved.is_ok());
    assert!(
        moved
            .map(|items| items
                .iter()
                .any(|(source, uid, target)| source == "INBOX" && *uid == 42 && target == "Junk"))
            .unwrap_or(false)
    );
    let stored = server.stored.lock();
    assert!(stored.is_ok());
    let stored = stored.map(|items| items.clone()).unwrap_or_default();
    assert!(stored.iter().any(|(source, uid, query)| {
        source == "INBOX" && *uid == 42 && query.contains("\\Seen")
    }));
    assert!(stored.iter().any(|(source, uid, query)| {
        source == "INBOX" && *uid == 42 && query.contains("$Junk")
    }));
    let commands = server
        .commands
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    let login_count = commands
        .iter()
        .filter(|cmd| cmd.as_str() == "LOGIN")
        .count();
    assert_eq!(
        login_count, 1,
        "confirmed push should reuse one IMAP session: {commands:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_archive_and_trash_use_configured_action_steps() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("special-use-push");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_arch", 43, None);
    write_triage(&root, "message_arch", "done");
    write_message(&root, "message_trash", 44, None);
    write_triage(&root, "message_trash", "delete");

    create_notification(&root, "done", "message_arch", "done", "done");
    assert_eq!(
        run(
            &root,
            &["message", "trash", "message_trash", "--reason", "delete"],
        )
        .0,
        0
    );
    assert!(root.join("trash/index.md").is_file());
    assert!(root.join("trash/message_trash.md").is_file());
    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let push = parse_one(&stdout);
    assert_eq!(push["count"], 2);
    assert!(push["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "message"
                && item["action"] == "notification"
                && item["steps"][0]["move_to_mailbox_id"] == "archive"
        })
    }));
    assert!(push["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == "message"
                && item["action"] == "trash"
                && item["steps"][0]["move_to_mailbox_id"] == "trash"
        })
    }));

    let server = start_imap_server(4);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_imap_without_smtp(&root, server.addr.port());

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 2, "{value}");

    assert!(server.handle.join().is_ok());
    let moved = server.moved.lock();
    assert!(moved.is_ok());
    let moved = moved.map(|items| items.clone()).unwrap_or_default();

    // Ids are immutable: archive/trash moves keep the original message ids and
    // only update the recorded remote location.
    assert!(root.join("messages/message_arch.json").exists());
    assert!(
        !root
            .join("messages/message_notification_44_900.json")
            .exists()
    );
    assert!(root.join("messages/message_trash.json").exists());
    assert!(!root.join("messages/message_trash_44_900.json").exists());
    let arch = read_json(&root.join("messages/message_arch.json"));
    assert_eq!(arch["remote"]["locations"][0]["mailbox_name"], "Archive");
    assert!(
        moved
            .iter()
            .any(|(source, uid, target)| source == "INBOX" && *uid == 43 && target == "Archive")
    );
    assert!(
        moved
            .iter()
            .any(|(source, uid, target)| source == "INBOX" && *uid == 44 && target == "Trash")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_archive_falls_back_to_uidplus_copy_delete_when_move_is_unsupported() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("archive-push-copy-fallback");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_arch", 43, None);
    write_triage(&root, "message_arch", "done");
    create_notification(&root, "done", "message_arch", "done", "done");

    let server = start_imap_server_with_capabilities(
        3,
        ImapServerCapabilities {
            move_supported: false,
            uidplus_supported: true,
            ..Default::default()
        },
    );
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_imap_without_smtp(&root, server.addr.port());

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    assert!(server.handle.join().is_ok());
    let moved = server
        .moved
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(moved.is_empty());
    let copied = server
        .copied
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(
        copied
            .iter()
            .any(|(source, uid, target)| source == "INBOX" && *uid == 43 && target == "Archive")
    );
    let stored = server
        .stored
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(stored.iter().any(|(source, uid, query)| {
        source == "INBOX" && *uid == 43 && query.contains("\\Deleted")
    }));
    let expunged = server
        .expunged
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(
        expunged
            .iter()
            .any(|(source, uid)| source == "INBOX" && *uid == 43)
    );
    let message = read_json(&root.join("messages/message_arch.json"));
    assert_eq!(message["remote"]["locations"][0]["mailbox_name"], "Archive");
    assert_eq!(message["remote"]["locations"][0]["uid"], 900);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_archive_without_move_or_uidplus_fails_before_copy_delete_fallback() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("archive-push-no-safe-fallback");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_arch", 43, None);
    write_triage(&root, "message_arch", "done");
    create_notification(&root, "done", "message_arch", "done", "done");

    let server = start_imap_server_with_capabilities(
        3,
        ImapServerCapabilities {
            move_supported: false,
            uidplus_supported: false,
            ..Default::default()
        },
    );
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    configure_imap_without_smtp(&root, server.addr.port());

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "imap_uidplus_required");
    assert_eq!(value["partially_applied_count"], 0);
    assert_eq!(value["pushed_count"], 0, "{value}");
    assert_eq!(value["failed_count"], 1, "{value}");
    assert_eq!(value["failures"].as_array().map(Vec::len), Some(1));
    assert!(value["failures"][0]["error"]["code"].as_str().is_some());
    assert!(value["failures"][0]["push_id"].as_str().is_some());
    assert_eq!(value["failures"][0]["error"]["retryable"], false);
    assert!(value["failures"][0]["error"]["hint"].as_str().is_some());
    assert_eq!(value["failures"][0]["partially_applied"], false);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(
        push["items"][0]["step_states"][0]["error_code"],
        "imap_uidplus_required"
    );

    assert!(server.handle.join().is_ok());
    let copied = server
        .copied
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    let stored = server
        .stored
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    assert!(copied.is_empty());
    assert!(stored.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn queued_send_case_archive_transitions_through_archiving_after_push() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("queued-send-archive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_archive_after_send", 42, None);
    write_triage(&root, "message_archive_after_send", "reply then archive");

    let imap = start_imap_server(4);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    configure_remote_send_only(&root, imap.addr.port(), smtp.addr.port());

    let (case_uid, case_path) = create_case_with_message(
        &root,
        "archive-after-send",
        "message_archive_after_send",
        "customer needs a reply",
    );

    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: reply\nreply_to_message_id: message_archive_after_send\nto:\n  - bob@example.com\ncc: []\nsubject: \"Done\"\nattachments:\n---\n\nHandled\n"
    ));
    assert!(fs::write(case_path.join("drafts/reply.md"), draft).is_ok());
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &["case", "archive", &case_uid, "--reason", "reply queued"],
    );
    assert_eq!(status, 0, "{stdout}");
    let archived = parse_one(&stdout);
    assert_eq!(archived["status"], "archiving");
    let archived_path = root.join(format!(
        "archived-cases/support/{case_uid}-archive-after-send"
    ));
    assert!(!case_path.exists());
    assert!(archived_path.exists());
    assert!(archived_path.join("drafts/reply.md").exists());

    let list = parse_one(&run(&root, &["case", "list"]).1);
    assert_eq!(list["scope"], "active");
    assert_eq!(list["count"], 1, "{list}");
    assert_eq!(list["items"][0]["case_uid"], case_uid);
    assert_eq!(list["items"][0]["status"], "archiving");
    assert_eq!(
        parse_one(&run(&root, &["case", "list", "--archived"]).1)["count"],
        0
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2, "{push}");
    assert_eq!(push["items"][0]["kind"], "draft", "{push}");
    assert_eq!(push["items"][0]["action"], "send", "{push}");
    assert_eq!(push["items"][1]["kind"], "message", "{push}");
    assert_eq!(push["items"][1]["action"], "notification", "{push}");

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 2, "{value}");
    assert!(
        smtp.data_rx
            .recv_timeout(Duration::from_secs(5))
            .map(|body| body.contains("Handled"))
            .unwrap_or(false)
    );
    assert!(smtp.handle.join().is_ok());
    assert!(imap.handle.join().is_ok());
    let moved = imap.moved.lock();
    assert!(moved.is_ok());
    assert!(
        moved
            .map(|items| items.iter().any(|(source, uid, target)| {
                source == "INBOX" && *uid == 42 && target == "Archive"
            }))
            .unwrap_or(false)
    );

    assert!(archived_path.exists());
    assert!(!archived_path.join("drafts/reply.md").exists());
    let drafts = read_json(&archived_path.join("data/drafts.json"));
    assert!(
        drafts["drafts"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
    );
    let case_data = read_json(&archived_path.join("data/case.json"));
    assert!(case_data["archived_rfc3339"].as_str().is_some());
    assert_eq!(case_data["status"], "archived");
    let list = parse_one(&run(&root, &["case", "list", "--archived"]).1);
    assert_eq!(list["count"], 1);
    assert_eq!(list["items"][0]["status"], "archived");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archived_case_unarchive_removes_archive_push_and_preserves_queued_send() {
    let root = temp_root("archived-unarchive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);
    write_message(&root, "message_archived_unarchive", 42, None);
    write_triage(
        &root,
        "message_archived_unarchive",
        "reply then maybe archive",
    );

    let (case_uid, case_path) = create_case_with_message(
        &root,
        "archived-unarchive",
        "message_archived_unarchive",
        "customer needs a reply",
    );
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Done\"\nattachments:\n---\n\nHandled\n"
    ));
    assert!(fs::write(case_path.join("drafts/reply.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "reply.md"]).0,
        0
    );

    let (status, stdout) = run(
        &root,
        &["case", "archive", &case_uid, "--reason", "reply queued"],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    let archived_path = root.join(format!(
        "archived-cases/support/{case_uid}-archived-unarchive"
    ));
    assert!(archived_path.exists());
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2, "{push}");
    assert!(push["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["kind"] == "draft" && item["action"] == "send")
    }));
    assert!(push["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["kind"] == "message" && item["action"] == "notification")
    }));

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "unarchive",
            &case_uid,
            "--group",
            "support",
            "--reason",
            "revise before push",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["source_status"], "archiving");
    assert_eq!(value["removed_push_count"], 1);
    let restored_path = root.join(format!("cases/support/{case_uid}-archived-unarchive"));
    assert!(restored_path.exists());
    assert!(restored_path.join("drafts/reply.md").exists());
    assert!(!archived_path.exists());

    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1, "{push}");
    assert_eq!(push["items"][0]["kind"], "draft");
    assert_eq!(push["items"][0]["action"], "send");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "change",
            &case_uid,
            "reply.md",
            "--body",
            "Updated before push",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_archive_defers_remote_archive_until_all_case_refs_are_archived() {
    let root = temp_root("case-archive-refs");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["case"]["add"]["steps"] = json!([]);
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_shared", 42, None);
    write_triage(&root, "message_shared", "shared case");
    let (case_one_uid, _case_one_path) =
        create_case_with_message(&root, "case-one", "message_shared", "shared case");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let case_two_uid = "c20260521002";
    write_case_fixture(
        &root,
        "support",
        case_two_uid,
        "case-two",
        &["message_shared"],
    );

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "archive",
            &case_one_uid,
            "--reason",
            "done with case one",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["queued"], false);
    assert_eq!(value["eligible_message_ids"], json!([]));
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "archive",
            case_two_uid,
            "--reason",
            "done with case two",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["queued"], true);
    assert_eq!(value["eligible_message_ids"], json!(["message_shared"]));
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["kind"], "message");
    assert_eq!(push["items"][0]["action"], "notification");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "unarchive",
            &case_one_uid,
            "--group",
            "support",
            "--reason",
            "new reply",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["removed_push_count"], 1);
    assert_eq!(value["push_ids"], json!([push["items"][0]["push_id"]]));
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "tag",
            &case_one_uid,
            "legal",
            "--reason",
            "legal review",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["tags"], json!(["legal"]));
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "untag",
            &case_one_uid,
            "legal",
            "--reason",
            "legal review done",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["tags"], json!([]));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archived_case_remove_and_delete_empty_case() {
    let root = temp_root("archive-case-remove-delete");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_archive_remove", 42, None);
    write_triage(&root, "message_archive_remove", "archived case message");

    let (case_uid, _case_path) = create_case_with_message(
        &root,
        "archive-remove-delete",
        "message_archive_remove",
        "case fixture",
    );
    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 0, "{stdout}");
    let archived_path = root.join(format!(
        "archived-cases/support/{case_uid}-archive-remove-delete"
    ));
    assert!(archived_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "delete",
            &case_uid,
            "--reason",
            "delete empty archived case",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_not_empty");

    let push_before = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push_before["items"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["push_id"].as_str())
        .unwrap_or_default()
        .to_string();

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "remove",
            &case_uid,
            "message_archive_remove",
            "--reason",
            "does not belong here",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["status"], "archiving");
    if !push_id.is_empty() {
        assert_eq!(value["push_ids"], json!([push_id]));
    }
    assert!(root.join("triage/message_archive_remove.md").exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "delete",
            &case_uid,
            "--reason",
            "empty archived case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    assert!(!archived_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn queued_save_draft_blocks_case_archive() {
    let root = temp_root("save-draft-archive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);
    let (case_uid, case_path) = create_case(&root, "save-block", "save draft blocks archive");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Draft\"\nattachments:\n---\n\nNot sent\n"
    ));
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "upload", &case_uid, "new.md"]).0,
        0
    );
    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "case_has_local_drafts");
    assert!(case_path.exists());
    assert!(case_path.join("drafts/new.md").exists());
    assert!(
        !root
            .join(format!("archived-cases/support/{case_uid}-save-block"))
            .exists()
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1, "{push}");
    assert_eq!(push["items"][0]["kind"], "draft");
    assert_eq!(push["items"][0]["action"], "save_draft");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn special_use_push_list_and_push_lock_work() {
    let root = temp_root("special-use-push");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");

    let (_notification_uid, _archive_path, value) = create_notification(
        &root,
        "done",
        "message_001",
        "archive later",
        "archive later",
    );
    assert_eq!(value["queued"], true);
    assert_eq!(value["location_count"], 1);
    let push_id = value["push_id"].as_str().unwrap_or_default().to_string();
    assert!(!push_id.is_empty());

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 1);
    assert_eq!(value["items"][0]["kind"], "message");
    assert_eq!(value["items"][0]["action"], "notification");
    assert_eq!(
        value["items"][0]["steps"],
        json!([{"move_to_mailbox_id": "archive"}])
    );
    let confirmed_plan_id = value["plan_id"].as_str().unwrap_or_default().to_string();

    let workspace_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(workspace_runtime_lock_path(&root));
    assert!(workspace_lock.is_ok());
    let Ok(workspace_lock) = workspace_lock else {
        return;
    };
    assert!(workspace_lock.try_lock().is_ok());
    let (status, stdout) = run(&root, &["push", "--confirm", &confirmed_plan_id]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "workspace_locked");
    assert_eq!(value["retryable"], true);
    assert!(workspace_lock.unlock().is_ok());

    let (status, stdout) = run(&root, &["push", "remove", &push_id]);
    assert_eq!(status, 2);
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "cli_unknown_command");
    // The rejection does not quote the token back; what points the caller at
    // the mistake is the hint naming the command it was written under.
    assert_eq!(error["message"], "unknown command");
    assert_eq!(
        error["hint"],
        "run `afmail push --help` to list the registered commands"
    );

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["count"], 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_archive_already_in_archive_marks_archived_without_push() {
    let root = temp_root("archive-noop");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_notificationd", 7, None);
    update_message_json(&root, "message_notificationd", |message| {
        message["remote"]["locations"][0]["mailbox_name"] = json!("Archive");
        message["remote"]["locations"][0]["mailbox_id"] = json!("archive");
    });
    write_triage(&root, "message_notificationd", "already handled");

    let (notification_uid, _archive_path, value) = create_notification(
        &root,
        "done",
        "message_notificationd",
        "already archived",
        "already archived",
    );
    assert_eq!(value["queued"], false);
    assert_eq!(value["location_count"], 1);
    assert_eq!(value["queued_location_count"], 0);
    assert!(!root.join("triage/message_notificationd.md").exists());
    assert_eq!(
        root.join(".afmail/push")
            .read_dir()
            .map(|entries| entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("json"))
                .count())
            .unwrap_or(99),
        0
    );
    let message = read_json(&root.join("messages/message_notificationd.json"));
    assert_eq!(message["workspace"]["status"], "notification");
    assert_eq!(message["workspace"]["notification_uid"], notification_uid);
    assert!(message["workspace"].get("buckets").is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_archive_files_without_case_and_queues_archive_mapping() {
    let root = temp_root("triage-archive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "invoice notice");

    let (notification_uid, archive_path, value) = create_notification(
        &root,
        "billing",
        "message_notice",
        "invoice notice",
        "invoice notice",
    );
    assert_eq!(value["notification_uid"], notification_uid);
    assert_eq!(value["eligible_message_ids"], json!(["message_notice"]));
    assert_eq!(value["queued"], true);
    assert!(!root.join("triage/message_notice.md").exists());
    assert!(
        !root
            .join("cases")
            .read_dir()
            .map(|mut entries| entries.any(|_| true))
            .unwrap_or(false)
    );

    let message = read_json(&root.join("messages/message_notice.json"));
    assert_eq!(message["workspace"]["status"], "notification");
    assert_eq!(message["workspace"]["notification_uid"], notification_uid);
    assert!(message["workspace"].get("buckets").is_none());
    assert_eq!(
        message["workspace"]["remote_sync"]["archive_eligible"],
        true
    );
    assert!(
        archive_path
            .join("views/messages/message_notice.md")
            .exists()
    );
    assert!(
        fs::read_to_string(archive_path.join("notification.md"))
            .unwrap_or_default()
            .contains("invoice notice")
    );

    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["kind"], "message");
    assert_eq!(push["items"][0]["action"], "notification");
    assert_eq!(
        push["items"][0]["steps"],
        json!([{"move_to_mailbox_id": "archive"}])
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn notification_remove_undoes_direct_archive_and_removes_pending_push() {
    let root = temp_root("remove-notification-message");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "invoice notice");

    let (notification_uid, archive_path, archived) = create_notification(
        &root,
        "billing",
        "message_notice",
        "invoice notice",
        "invoice notice",
    );
    let push_id = archived["push_id"].as_str().unwrap_or_default().to_string();
    assert!(!push_id.is_empty());
    assert!(
        archive_path
            .join("views/messages/message_notice.md")
            .exists()
    );

    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "remove",
            &notification_uid,
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["notification_uid"], notification_uid);
    assert_eq!(value["removed_push_count"], 1);
    assert_eq!(value["push_ids"], json!([push_id]));
    assert!(root.join("triage/message_notice.md").exists());
    assert!(
        !archive_path
            .join("views/messages/message_notice.md")
            .exists()
    );
    let message = read_json(&root.join("messages/message_notice.json"));
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"]["notification_uid"].is_null());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_trash_discards_entire_related_conversation() {
    let root = temp_root("case-trash-related");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_trash_parent", 71, None);
    write_message(&root, "message_trash_reply", 72, None);
    update_message_json(&root, "message_trash_reply", |value| {
        value["direction"] = json!("outbound");
        value["subject"] = json!("Re: Contract renewal");
        value["from"] = json!("me@example.com");
        value["received_rfc3339"] = serde_json::Value::Null;
        value["sent_rfc3339"] = json!("2026-05-21T10:05:00Z");
        value["in_reply_to"] = json!("<message_trash_parent@example.com>");
        value["references"] = json!(["<message_trash_parent@example.com>"]);
        value["remote"]["locations"][0]["mailbox_id"] = json!("sent");
        value["remote"]["locations"][0]["mailbox_name"] = json!("Sent");
    });
    write_triage(&root, "message_trash_parent", "parent");
    write_triage(&root, "message_trash_reply", "reply");

    let (case_uid, case_path) = create_case_with_message(
        &root,
        "trash-conversation",
        "message_trash_parent",
        "conversation needs case",
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_trash_reply",
            "--reason",
            "reply belongs to same case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "trash",
            &case_uid,
            "--reason",
            "test conversation can be discarded",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["source_status"], "");
    assert_eq!(value["trashed_message_count"], 2);
    assert_eq!(
        value["message_ids"],
        json!(["message_trash_parent", "message_trash_reply"])
    );
    assert_eq!(
        value["eligible_message_ids"],
        json!(["message_trash_parent", "message_trash_reply"])
    );
    assert_eq!(value["location_count"], 2);
    assert_eq!(value["queued_location_count"], 2);
    assert_eq!(value["queued"], true);
    assert!(!case_path.exists());

    for message_id in ["message_trash_parent", "message_trash_reply"] {
        let message = read_json(&root.join(format!("messages/{message_id}.json")));
        assert_eq!(message["workspace"]["status"], "trashed");
        assert!(root.join(format!("trash/{message_id}.md")).exists());
        assert!(!root.join(format!("triage/{message_id}.md")).exists());
    }
    let trash = read_json(&root.join("trash/data/trash.json"));
    let trash_items = trash["items"].as_array().cloned().unwrap_or_default();
    for message_id in ["message_trash_parent", "message_trash_reply"] {
        assert!(
            trash_items
                .iter()
                .any(|item| item["message_id"] == message_id)
        );
    }
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2);
    assert!(push["items"].as_array().is_some_and(|items| {
        ["message_trash_parent", "message_trash_reply"]
            .iter()
            .all(|message_id| {
                items.iter().any(|item| {
                    item["kind"] == "message"
                        && item["action"] == "trash"
                        && item["message_ids"] == json!([message_id])
                })
            })
    }));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archived_case_trash_discards_case_without_unarchive() {
    let root = temp_root("archive-case-trash");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["message"]["notification"]["by_source_mailbox_id"]["inbox"]["steps"] =
        json!([]);
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_notification_case_trash", 81, None);
    write_triage(&root, "message_notification_case_trash", "done");

    let (case_uid, _case_path) = create_case_with_message(
        &root,
        "archived-trash",
        "message_notification_case_trash",
        "conversation done",
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "archive",
            &case_uid,
            "--reason",
            "conversation complete",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let archived = parse_one(&stdout);
    assert_eq!(archived["queued"], false);
    let archived_path = root.join(archived["case_path"].as_str().unwrap_or_default());
    assert!(archived_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "trash",
            &case_uid,
            "--reason",
            "archived test case can be discarded",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["source_status"], "archived");
    assert_eq!(
        value["message_ids"],
        json!(["message_notification_case_trash"])
    );
    assert_eq!(
        value["eligible_message_ids"],
        json!(["message_notification_case_trash"])
    );
    assert!(!archived_path.exists());
    let message = read_json(&root.join("messages/message_notification_case_trash.json"));
    assert_eq!(message["workspace"]["status"], "trashed");
    assert!(
        root.join("trash/message_notification_case_trash.md")
            .exists()
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["action"], "trash");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_trash_rejects_cases_with_local_drafts() {
    let root = temp_root("case-trash-draft");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_case_with_draft", 91, None);
    write_triage(&root, "message_case_with_draft", "needs reply");
    let (case_uid, case_path) = create_case_with_message(
        &root,
        "case-with-draft",
        "message_case_with_draft",
        "needs a draft",
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "friend@example.com",
            "--subject",
            "Test",
            "--body",
            "Draft body",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &["case", "trash", &case_uid, "--reason", "discard test case"],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "case_has_local_drafts");
    assert!(
        value["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("archive/merge/trash")
    );
    assert!(
        value["suggested_commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .unwrap_or_default()
                .contains("afmail case draft remove")))
    );
    assert!(case_path.exists());
    let message = read_json(&root.join("messages/message_case_with_draft.json"));
    assert_ne!(message["workspace"]["status"], "trashed");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn push_confirmation_rejects_changed_identity_attachment_and_draft_before_smtp() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("push-plan-binding");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["actions"]["draft"]["send"]["steps"] = json!([{"smtp_send": {}}]);
    config["identities"] =
        json!([{"identity": "me", "name": "Me", "email": "me@example.com", "default": true}]);
    write_json(&root.join(".afmail/config.json"), &config);
    let (case_uid, case_path) = create_case(&root, "plan-binding", "plan binding test");
    assert!(fs::create_dir_all(case_path.join("files")).is_ok());
    let attachment_path = case_path.join("files/review.txt");
    assert!(fs::write(&attachment_path, "version one\n").is_ok());
    let draft_path = case_path.join("drafts/bound.md");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Bound plan\"\nattachments:\n  - files/review.txt\n---\n\nOriginal reviewed body\n"
    ));
    assert!(fs::write(&draft_path, &draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "bound.md"]).0,
        0
    );

    let first = parse_one(&run(&root, &["push"]).1);
    assert_eq!(
        first["items"][0]["outbound"]["attachments"][0]["filename"],
        "review.txt"
    );
    assert_eq!(
        first["items"][0]["outbound"]["attachments"][0]["size_bytes"],
        12
    );
    let first_plan_id = first["plan_id"].as_str().unwrap_or_default();
    config["identities"][0]["name"] = json!("Changed Sender");
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run(&root, &["push", "--confirm", first_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    let changed = parse_one(&stdout);
    assert_eq!(changed["code"], "push_plan_changed");
    assert_eq!(changed["remote_calls_started"], false);

    let second_plan_id = current_push_plan_id(&root);
    assert!(fs::write(&attachment_path, "version two changed\n").is_ok());
    let (status, stdout) = run(&root, &["push", "--confirm", &second_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_plan_changed");

    let third_plan_id = current_push_plan_id(&root);
    let current_draft = fs::read_to_string(&draft_path).unwrap_or_default();
    assert!(
        fs::write(
            &draft_path,
            current_draft.replace("Original reviewed body", "Latest approved body")
        )
        .is_ok()
    );
    let (status, stdout) = run(&root, &["push", "--confirm", &third_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_plan_changed");

    let fourth_plan_id = current_push_plan_id(&root);
    config["actions"]["draft"]["send"]["steps"] =
        json!([{"smtp_send": {}}, {"append_to_mailbox_id": "sent"}]);
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run(&root, &["push", "--confirm", &fourth_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    let changed = parse_one(&stdout);
    assert_eq!(changed["code"], "push_plan_changed");
    assert_eq!(changed["remote_calls_started"], false);
    config["actions"]["draft"]["send"]["steps"] = json!([{"smtp_send": {}}]);
    write_json(&root.join(".afmail/config.json"), &config);

    let fifth_plan_id = current_push_plan_id(&root);
    assert_eq!(
        run(&root, &["case", "draft", "upload", &case_uid, "bound.md"]).0,
        0
    );
    let (status, stdout) = run(&root, &["push", "--confirm", &fifth_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_plan_changed");
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "bound.md"]).0,
        0
    );

    let queue_plan = parse_one(&run(&root, &["push"]).1);
    let sixth_plan_id = queue_plan["plan_id"].as_str().unwrap_or_default();
    let push_id = queue_plan["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default();
    let queue_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let queue_bytes = fs::read(&queue_path).unwrap_or_default();
    let mut rewritten_queue = queue_bytes.clone();
    rewritten_queue.push(b' ');
    assert!(fs::write(&queue_path, &rewritten_queue).is_ok());
    let (status, stdout) = run(&root, &["push", "--confirm", sixth_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_plan_changed");
    assert!(fs::write(&queue_path, &queue_bytes).is_ok());

    let seventh_plan_id = current_push_plan_id(&root);
    assert!(fs::remove_file(&queue_path).is_ok());
    let (status, stdout) = run(&root, &["push", "--confirm", &seventh_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    let changed = parse_one(&stdout);
    assert_eq!(changed["code"], "push_plan_changed");
    assert_eq!(changed["remote_calls_started"], false);
    assert!(fs::write(&queue_path, &queue_bytes).is_ok());

    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let Some(smtp) = smtp else {
        return;
    };
    config["smtp"]["host"] = json!("127.0.0.1");
    config["smtp"]["port"] = json!(smtp.addr.port());
    config["smtp"]["starttls"] = json!(false);
    config["smtp"]["tls_wrapper"] = json!(false);
    write_json(&root.join(".afmail/config.json"), &config);

    let confirmed_plan_id = current_push_plan_id(&root);
    let (status, stdout) = run(&root, &["push", "--confirm", &confirmed_plan_id]);
    assert_eq!(status, 0, "{stdout}");
    assert!(
        smtp.data_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "{stdout}"
    );
    assert!(smtp.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn push_drafts_send_new_message_skips_reply_to_step() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("new-send-no-reply-step");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path) = create_case(&root, "new-thread", "new outbound case");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"New outbound\"\nattachments:\n---\n\nHi Bob\n"
    ));
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());

    let imap = start_imap_server(4);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    configure_remote(&root, imap.addr.port(), smtp.addr.port());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "new.md"]).0,
        0
    );

    let (status, stdout) = run(&root, &["push", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    let dry_run = parse_one(&stdout);
    assert_eq!(
        dry_run["items"][0]["actions"],
        json!([
            "append_to_mailbox_id_drafts",
            "smtp_send",
            "append_to_mailbox_id_sent"
        ])
    );

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert!(value.get("outcome").is_none());
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    assert!(
        smtp.data_rx
            .recv_timeout(Duration::from_secs(5))
            .map(|body| body.contains("Hi Bob"))
            .unwrap_or(false)
    );
    assert!(smtp.handle.join().is_ok());
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    assert!(
        appended
            .map(
                |items| items.iter().any(|(folder, draft, body)| folder == "Sent"
                    && !*draft
                    && body.contains("Hi Bob"))
            )
            .unwrap_or(false)
    );
    let stored = imap.stored.lock();
    assert!(stored.is_ok());
    assert!(stored.map(|items| items.is_empty()).unwrap_or(false));
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    assert!(imap.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn push_drafts_send_does_not_require_imap_move() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("no-move");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");
    let (case_uid, case_path) =
        create_case_with_message(&root, "case-one", "message_001", "reply test case");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nreply_to_message_id: message_001\nto:\n  - alice@example.com\nsubject: \"Re: Contract\"\nattachments:\n---\n\nHi\n"
    ));
    assert!(fs::write(case_path.join("drafts/reply.md"), draft).is_ok());
    let imap = start_imap_server(3);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    configure_remote(&root, imap.addr.port(), smtp.addr.port());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "reply.md"]).0,
        0
    );
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "reply.md"]).0,
        0
    );
    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert!(value.get("outcome").is_none());
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    assert!(
        smtp.data_rx
            .recv_timeout(Duration::from_secs(5))
            .map(|body| body.contains("Hi"))
            .unwrap_or(false)
    );
    assert!(smtp.handle.join().is_ok());
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    assert!(
        appended
            .map(|items| items
                .iter()
                .any(|(folder, draft, body)| folder == "Sent" && !*draft && body.contains("Hi")))
            .unwrap_or(false)
    );
    let stored = imap.stored.lock();
    assert!(stored.is_ok());
    assert!(
        stored
            .map(|items| items
                .iter()
                .any(|(_, _, query)| query.contains("\\Answered")))
            .unwrap_or(false)
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 0);
    assert!(imap.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn push_multiple_sends_logs_incremental_refresh_progress() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("multi-send-render-progress");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let smtp = start_smtp_server_connections(2);
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    configure_smtp_send_only(&root, smtp.addr.port());

    let (case_one_uid, case_one_path) = create_case(&root, "multi-one", "first send");
    let draft_one = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_one_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"One\"\nattachments:\n---\n\nFirst body\n"
    ));
    assert!(fs::write(case_one_path.join("drafts/one.md"), draft_one).is_ok());
    let (case_two_uid, case_two_path) = create_case(&root, "multi-two", "second send");
    let draft_two = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_two_uid}\nsend_intent: new\nto:\n  - sue@example.com\ncc: []\nsubject: \"Two\"\nattachments:\n---\n\nSecond body\n"
    ));
    assert!(fs::write(case_two_path.join("drafts/two.md"), draft_two).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_one_uid, "one.md"]).0,
        0
    );
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_two_uid, "two.md"]).0,
        0
    );

    let (status, stdout) =
        run_confirmed_push_with_global_args(&root, &["--output-to", "stdout", "--log", "progress"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    let progress = lines
        .iter()
        .filter(|line| line["event"] == "progress")
        .collect::<Vec<_>>();
    assert!(
        progress
            .iter()
            .any(|line| line["phase"] == "push_local_refresh_start"),
        "{stdout}"
    );
    assert!(
        progress
            .iter()
            .any(|line| line["phase"] == "push_local_refresh_done"),
        "{stdout}"
    );
    assert!(
        progress
            .iter()
            .all(|line| line["phase"] != "render_start" && line["phase"] != "push_render_start"),
        "{stdout}"
    );
    let result = lines.last().cloned().unwrap_or(serde_json::Value::Null);
    assert_eq!(confirmed_push_items(&result).len(), 2, "{result}");
    assert!(result.get("render").is_none(), "{result}");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let sent_bodies = [
        smtp.data_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default(),
        smtp.data_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default(),
    ];
    assert!(sent_bodies.iter().any(|body| body.contains("First body")));
    assert!(sent_bodies.iter().any(|body| body.contains("Second body")));
    assert!(smtp.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn push_send_recovers_from_each_local_finalize_stage() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for stage in [
        "remote_succeeded",
        "outbound_recorded",
        "draft_removed",
        "pending_cleared",
        "audit_pending",
    ] {
        let root = temp_root(&format!("push-finalize-recover-{stage}"));
        assert!(fs::create_dir_all(&root).is_ok());
        assert_eq!(run(&root, &["init"]).0, 0);
        let smtp = start_smtp_server();
        assert!(smtp.is_some());
        let smtp = match smtp {
            Some(server) => server,
            None => return,
        };
        configure_smtp_send_only(&root, smtp.addr.port());
        // A reply-only step is resolved from the draft, which finalization
        // removes; resuming must not need the draft again.
        let config_path = root.join(".afmail/config.json");
        let mut config = read_json(&config_path);
        config["actions"]["draft"]["send"]["steps"] = json!([
            {"smtp_send": {}},
            {"add_flags": ["\\Answered"], "on": "reply_to_message"}
        ]);
        write_json(&config_path, &config);
        let (case_uid, case_path) = create_case(&root, &format!("recover-{stage}"), "recover send");
        let draft_path = case_path.join("drafts/recover.md");
        let draft = draft_with_compose(format!(
            "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Recover {stage}\"\nattachments:\n---\n\nRecover body {stage}\n"
        ));
        assert!(fs::write(&draft_path, draft).is_ok());
        assert_eq!(
            run(&root, &["case", "draft", "send", &case_uid, "recover.md"]).0,
            0
        );
        let push = parse_one(&run(&root, &["push", "list"]).1);
        let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
        let message_id = sent_message_id_for_push_id(push_id);

        let (status, stdout) = if stage == "remote_succeeded" {
            run_confirmed_push_with_env(
                &root,
                &[
                    ("AFMAIL_TEST_FAIL_PUSH_LOCAL_STAGE_AFTER", stage),
                    ("AFMAIL_TEST_FAIL_PUSH_FAILURE_PERSIST", "push_item"),
                ],
            )
        } else {
            run_confirmed_push_with_env(
                &root,
                &[("AFMAIL_TEST_FAIL_PUSH_LOCAL_STAGE_AFTER", stage)],
            )
        };
        assert_eq!(status, 3, "{stdout}");
        let error = parse_one(&stdout);
        assert_eq!(error["code"], "partial");
        assert_eq!(error["partially_applied_count"], 1);
        assert_eq!(
            error["failures"][0]["error"]["code"],
            "push_local_stage_test_failure"
        );
        let persistence_failures = error["failures"][0]["state_persistence_failures"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if stage == "remote_succeeded" {
            assert_eq!(persistence_failures.len(), 1, "{error}");
            assert_eq!(persistence_failures[0]["stage"], "push_item");
            assert_eq!(
                persistence_failures[0]["error"]["code"],
                "push_failure_persistence_test_failure"
            );
        } else {
            assert!(persistence_failures.is_empty(), "{error}");
        }
        assert!(
            smtp.data_rx
                .recv_timeout(Duration::from_secs(5))
                .map(|body| body.contains(stage))
                .unwrap_or(false)
        );
        assert!(smtp.handle.join().is_ok());
        let failed_push = parse_one(&run(&root, &["push", "list"]).1);
        assert_eq!(failed_push["count"], 1, "{failed_push}");
        assert_eq!(failed_push["items"][0]["local_stage"], stage);

        let (status, stdout) = run_confirmed_push(&root);
        assert_eq!(status, 0, "{stage}: {stdout}");
        let value = parse_one(&stdout);
        assert_eq!(confirmed_push_items(&value).len(), 1, "{stage}: {value}");
        assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
        assert!(!draft_path.exists(), "{stage}");
        assert!(
            root.join(format!(".afmail/messages/{message_id}.eml"))
                .is_file()
        );
        let transactions = fs::read_dir(root.join(".afmail/transactions"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry.path().extension().and_then(|s| s.to_str()) == Some("json")
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(transactions, 0, "{stage}");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
#[cfg(feature = "smtp")]
fn push_audit_warning_is_partial_after_remote_effect_succeeds() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("push-audit-warning");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let Some(smtp) = smtp else {
        return;
    };
    configure_smtp_send_only(&root, smtp.addr.port());
    let (case_uid, case_path) = create_case(&root, "audit-warning", "audit warning test");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Audit warning\"\nattachments:\n---\n\nRemote effect succeeds\n"
    ));
    assert!(fs::write(case_path.join("drafts/audit.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "audit.md"]).0,
        0
    );

    let (status, stdout) = run_confirmed_push_with_env(
        &root,
        &[("AFMAIL_TEST_FAIL_PUSH_AUDIT_AFTER_APPEND", "push_succeeded")],
    );
    assert_eq!(status, 3, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "partial");
    assert_eq!(value["pushed_count"], 1);
    assert_eq!(value["failed_count"], 0);
    assert_eq!(value["partially_applied_count"], 0);
    assert_eq!(value["audit_pending_count"], 1);
    assert_eq!(value["stopped_after_failure"], true);
    assert_eq!(value["audit_warnings"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        value["audit_warnings"][0]["error_code"],
        "push_audit_after_append_test_failure"
    );
    assert!(
        smtp.data_rx
            .recv_timeout(Duration::from_secs(5))
            .map(|body| body.contains("Remote effect succeeds"))
            .unwrap_or(false)
    );
    assert!(smtp.handle.join().is_ok());
    let pending = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(pending["count"], 1);
    assert_eq!(pending["items"][0]["audit_pending"], true);
    assert_eq!(pending["items"][0]["effect_scope"], "local_recovery");
    assert_eq!(pending["items"][0]["local_stage"], "audit_pending");
    let progress = read_json(&root.join(".afmail/cache/progress.json"));
    assert_eq!(progress["status"], "partial");
    assert!(progress["fields"].get("outcome").is_none());
    assert_eq!(progress["error"]["code"], "partial");
    assert!(progress["error"].get("retryable").is_none());

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 3, "{stdout}");
    let doctor = parse_one(&stdout);
    assert!(doctor["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue["code"] == "push_audit_pending")
    }));

    // Recovery is bound only to the durable audit identity. Even if action
    // config changes after the remote effect and the draft has been removed,
    // it must not resolve or execute any new remote step.
    let mut config = read_json(&root.join(".afmail/config.json"));
    config["smtp"]["port"] = json!(1);
    config["actions"]["draft"]["send"]["steps"] = json!([
        {"smtp_send": {}},
        {"append_to_mailbox_id": "sent"}
    ]);
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let recovered = parse_one(&stdout);
    assert!(recovered.get("outcome").is_none());
    assert_eq!(confirmed_push_items(&recovered).len(), 1);
    assert_eq!(recovered["succeeded_items"][0]["audit_pending"], false);
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    let succeeded_events = parse_raw_json_lines(&audit)
        .into_iter()
        .filter(|event| {
            event["kind"] == "push_succeeded"
                && event["push_id"] == value["succeeded_items"][0]["push_id"]
        })
        .count();
    assert_eq!(succeeded_events, 1);
    let _ = fs::remove_dir_all(root);
}

#[cfg(feature = "smtp")]
#[test]
fn push_drafts_send_resumes_after_completed_smtp_step() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("send-resume-after-smtp");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);
    let (case_uid, case_path) = create_case(&root, "resume-thread", "resume outbound case");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Resume outbound\"\nattachments:\n---\n\nResume me\n"
    ));
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "new.md"]).0,
        0
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let mut push_item = read_json(&push_path);
    push_item["step_states"] = json!([
        {
            "index": 0,
            "label": "append_to_mailbox_id_drafts",
            "status": "succeeded",
            "started_rfc3339": "2026-06-09T00:00:00Z",
            "completed_rfc3339": "2026-06-09T00:00:01Z",
            "result_summary": "append succeeded"
        },
        {
            "index": 1,
            "label": "smtp_send",
            "status": "succeeded",
            "started_rfc3339": "2026-06-09T00:00:01Z",
            "completed_rfc3339": "2026-06-09T00:00:02Z",
            "result_summary": "smtp_send succeeded"
        }
    ]);
    write_json(&push_path, &push_item);

    let imap = start_imap_server(3);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    // No SMTP host is configured. If step_states is ignored, this push fails
    // before it can append to Sent.
    configure_imap_without_smtp(&root, imap.addr.port());

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(confirmed_push_items(&value).len(), 1, "{value}");
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    assert!(
        appended
            .map(
                |items| items.iter().any(|(folder, draft, body)| folder == "Sent"
                    && !*draft
                    && body.contains("Resume me"))
            )
            .unwrap_or(false)
    );
    assert!(imap.handle.join().is_ok());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "smtp")]
fn push_confirm_stops_after_first_failure() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("push-fail-fast");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    configure_smtp_then_sent_append(&root, smtp.addr.port());

    let (case_one_uid, case_one_path) = create_case(&root, "fail-one", "first send");
    let draft_one = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_one_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"One\"\nattachments:\n---\n\nFirst body\n"
    ));
    assert!(fs::write(case_one_path.join("drafts/one.md"), draft_one).is_ok());
    let (case_two_uid, case_two_path) = create_case(&root, "fail-two", "second send");
    let draft_two = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_two_uid}\nsend_intent: new\nto:\n  - sue@example.com\ncc: []\nsubject: \"Two\"\nattachments:\n---\n\nSecond body\n"
    ));
    assert!(fs::write(case_two_path.join("drafts/two.md"), draft_two).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_one_uid, "one.md"]).0,
        0
    );
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_two_uid, "two.md"]).0,
        0
    );

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 3, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "partial");
    assert_eq!(value["partially_applied_count"], 1);
    assert_eq!(value["pushed_count"], 0, "{value}");
    assert_eq!(value["failed_count"], 1, "{value}");
    assert_eq!(value["stopped_after_failure"], true);
    assert_eq!(value["unattempted_count"], 1);
    assert_eq!(value["pending_queue_count"], 2);
    assert_eq!(value["recovery_required_count"], 1);
    assert_eq!(value["failures"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["failures"][0]["partially_applied"], true);
    assert_eq!(value["failures"][0]["succeeded_step_count"], 1);
    let progress = read_json(&root.join(".afmail/cache/progress.json"));
    assert_eq!(progress["status"], "partial");
    assert!(progress["fields"].get("outcome").is_none());
    assert_eq!(progress["fields"]["success"], false);
    assert!(progress["result"].get("outcome").is_none());
    assert_eq!(
        progress["error"]["code"],
        value["failures"][0]["error"]["code"]
    );
    assert_eq!(
        progress["error"]["retryable"],
        value["failures"][0]["error"]["retryable"]
    );
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let status_value = parse_one(&stdout);
    assert_eq!(status_value["progress"]["status"], "partial");
    assert!(
        status_value["progress"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("partially completed"))
    );
    let sent_body = smtp
        .data_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    assert!(
        sent_body.contains("First body") || sent_body.contains("Second body"),
        "{sent_body}"
    );
    assert!(smtp.handle.join().is_ok());

    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2);
    let items = push["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items
            .iter()
            .filter(|item| item["attempt_count"] == 1)
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["attempt_count"] == 0
                && item["step_states"]
                    .as_array()
                    .is_some_and(|states| states.is_empty()))
            .count(),
        1
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_list_orders_items_by_enqueue_sequence_not_push_id() {
    let root = temp_root("push-fifo-order");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    // push_ids are derived from message_id (push_message_<id>), so enqueueing in this
    // order produces push_ids in reverse lexicographic order (push_message_message_c,
    // push_message_message_b, push_message_message_a). All three enqueues happen
    // within the same wall-clock second, so created_rfc3339 cannot distinguish them:
    // only enqueue_seq can recover true enqueue order.
    for message_id in ["message_c", "message_b", "message_a"] {
        write_message(&root, message_id, 40, None);
        write_triage(&root, message_id, "reverse order fixture");
        let (status, stdout) = run(
            &root,
            &[
                "message",
                "spam",
                message_id,
                "--reason",
                "reverse order fixture",
            ],
        );
        assert_eq!(status, 0, "{stdout}");
    }

    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 3);
    let message_ids = push["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["message_ids"][0]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        message_ids,
        vec![
            "message_c".to_string(),
            "message_b".to_string(),
            "message_a".to_string(),
        ]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_reenqueue_preserves_original_enqueue_seq_and_position() {
    let root = temp_root("push-reenqueue-seq");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);

    // First draft queued for send: gets enqueue_seq 1.
    let (case_uid_first, case_path_first) = create_case(&root, "reenqueue-first", "first case");
    let draft_first = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid_first}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"First\"\nattachments:\n---\n\nFirst body\n"
    ));
    assert!(fs::write(case_path_first.join("drafts/new.md"), draft_first).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid_first, "new.md"]).0,
        0
    );

    // Second draft queued for send: gets enqueue_seq 2, so it currently sorts after
    // the first draft's push item.
    let (case_uid_second, case_path_second) = create_case(&root, "reenqueue-second", "second case");
    let draft_second = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid_second}\nsend_intent: new\nto:\n  - carol@example.com\ncc: []\nsubject: \"Second\"\nattachments:\n---\n\nSecond body\n"
    ));
    assert!(fs::write(case_path_second.join("drafts/new.md"), draft_second).is_ok());
    assert_eq!(
        run(
            &root,
            &["case", "draft", "send", &case_uid_second, "new.md"]
        )
        .0,
        0
    );

    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 2);
    let first_push_id = push["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let second_push_id = push["items"][1]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        first_push_id.strip_prefix("push_draft_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        ),
        "{first_push_id}"
    );
    assert!(
        second_push_id.strip_prefix("push_draft_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        ),
        "{second_push_id}"
    );
    assert_ne!(first_push_id, second_push_id);

    // Re-enqueue the first draft's send (e.g. re-running draft send after an edit).
    // This must preserve the original enqueue_seq, not bump it to the back of the
    // queue behind the second draft.
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid_first, "new.md"]).0,
        0
    );

    let push_after = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push_after["count"], 2);
    assert_eq!(push_after["items"][0]["push_id"], first_push_id);
    assert_eq!(push_after["items"][1]["push_id"], second_push_id);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_new_queue_lifecycle_for_the_same_draft_gets_a_new_outbound_identity() {
    let root = temp_root("push-new-draft-identity");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);
    let (case_uid, case_path) = create_case(&root, "identity", "identity");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Identity\"\nattachments:\n---\n\nIdentity body\n"
    ));
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "new.md"]).0,
        0
    );
    let first = parse_one(&run(&root, &["push", "list"]).1);
    let first_push_id = first["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        fs::remove_file(
            root.join(".afmail/push")
                .join(format!("{first_push_id}.json"))
        )
        .is_ok()
    );

    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "new.md"]).0,
        0
    );
    let second = parse_one(&run(&root, &["push", "list"]).1);
    let second_push_id = second["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(first_push_id, second_push_id);
    assert_ne!(
        sent_message_id_for_push_id(&first_push_id),
        sent_message_id_for_push_id(&second_push_id)
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(feature = "smtp")]
fn uncertain_draft_fixture(name: &str) -> (std::path::PathBuf, String, std::path::PathBuf) {
    let root = temp_root(name);
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    configure_default_identity(&root);
    let (case_uid, case_path) = create_case(&root, "recover", "recover uncertain draft append");
    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: Recovery\nattachments:\n---\n\nRecovery body\n"
    ));
    assert!(fs::write(case_path.join("drafts/recover.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "send", &case_uid, "recover.md"]).0,
        0
    );
    let plan = parse_one(&run(&root, &["push"]).1);
    let push_id = plan["items"][0]["push_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!push_id.is_empty(), "{plan}");
    let push_path = root.join(format!(".afmail/push/{push_id}.json"));
    let mut item = read_json(&push_path);
    item["attempt_count"] = json!(1);
    item["step_states"] = json!([
        {"index":0,"label":"append_to_mailbox_id_drafts","status":"remote_outcome_unknown", "started_rfc3339":"2026-10-19T09:32:17Z", "error_code":"imap_append_failed", "error":"Could not append mail to mailbox", "retryable":false},
        {"index":1,"label":"smtp_send","status":"pending"},
        {"index":2,"label":"append_to_mailbox_id_sent","status":"pending"}
    ]);
    write_json(&push_path, &item);
    (root, push_id, push_path)
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_rechecks_drafts_and_sent_and_changes_only_local_state() {
    let _guard = NETWORK_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (search, resolution, step_status) in [
        (SearchResponse::Absent, "draft_absent", "pending"),
        (SearchResponse::DraftOnly, "draft_found", "succeeded"),
    ] {
        let (root, push_id, path) = uncertain_draft_fixture(&format!("recover-{search:?}"));
        let server = start_imap_server_with_capabilities(
            3,
            ImapServerCapabilities {
                search,
                ..Default::default()
            },
        )
        .unwrap();
        configure_imap_without_smtp(&root, server.addr.port());
        let before = fs::read(&path).unwrap();
        let old_plan_id = current_push_plan_id(&root);
        let (status, output) = run(&root, &["push", "recover", &push_id]);
        assert_eq!(status, 0, "{output}");
        let value = parse_one(&output);
        assert_eq!(value["resolution"], resolution);
        assert_eq!(value["confirmed"], false);
        assert_eq!(value["evidence"].as_array().unwrap().len(), 2);
        assert_eq!(value["recovery_kind"], "drafts_append");
        assert_eq!(fs::read(&path).unwrap(), before);
        let plan_id = value["plan_id"].as_str().unwrap().to_string();
        let (status, output) = run(&root, &["push", "recover", &push_id, "--confirm", &plan_id]);
        assert_eq!(status, 0, "{output}");
        let value = parse_one(&output);
        assert_eq!(value["confirmed"], true);
        let item = read_json(&path);
        assert_eq!(item["step_states"][0]["status"], step_status);
        assert_eq!(item["step_states"][1]["status"], "pending");
        assert_eq!(item["push_id"], push_id);
        if resolution == "draft_absent" {
            assert_eq!(
                item["step_states"][0]["error"],
                "Could not append mail to mailbox"
            );
            assert_eq!(item["step_states"][0]["retryable"], true);
            assert!(item["step_states"][0].get("started_rfc3339").is_none());
            assert!(item["step_states"][0].get("completed_rfc3339").is_none());
        }
        assert_ne!(current_push_plan_id(&root), old_plan_id);
        let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap();
        assert!(audit.contains("push_recovery_checked"));
        assert!(server.appended.lock().unwrap().is_empty());
        assert!(server.stored.lock().unwrap().is_empty());
        assert!(server.moved.lock().unwrap().is_empty());
        let commands = server.commands.lock().unwrap().clone();
        assert_eq!(
            commands.iter().filter(|c| c.as_str() == "SEARCH").count(),
            4
        );
        // Ordinary confirmed push resumes at the repaired step. SMTP is not
        // configured, so it stops before sending; a found draft is not appended again.
        let (status, output) = run_confirmed_push(&root);
        assert_eq!(status, 3, "{output}");
        assert_eq!(read_json(&path)["step_states"][0]["status"], "succeeded");
        assert_eq!(
            server.appended.lock().unwrap().len(),
            usize::from(resolution == "draft_absent")
        );
        assert!(server.handle.join().is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_refuses_sent_copy_ambiguous_results_and_search_errors() {
    let _guard = NETWORK_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (search, code) in [
        (SearchResponse::Found, "push_recovery_sent_copy_found"),
        (SearchResponse::Ambiguous, "imap_uid_ambiguous"),
        (SearchResponse::Failed, "imap_search_failed"),
    ] {
        let (root, push_id, path) = uncertain_draft_fixture(&format!("recover-refuse-{search:?}"));
        let server = start_imap_server_with_capabilities(
            1,
            ImapServerCapabilities {
                search,
                ..Default::default()
            },
        )
        .unwrap();
        configure_imap_without_smtp(&root, server.addr.port());
        let before = fs::read(&path).unwrap();
        let (status, output) = run(
            &root,
            &[
                "push",
                "recover",
                &push_id,
                "--confirm",
                "push_recover_plan_unseen",
            ],
        );
        assert_eq!(status, 1, "{output}");
        assert_eq!(parse_one(&output)["code"], code, "{output}");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(server.appended.lock().unwrap().is_empty());
        server.handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_refuses_smtp_progress_incomplete_evidence_and_changed_plans() {
    for (variant, code) in [
        ("smtp", "push_recovery_unsupported"),
        ("started", "push_recovery_unsupported"),
        ("truncated", "push_recovery_unsupported"),
        ("changed", "push_plan_changed_after_start"),
        ("local", "push_recovery_unsupported"),
    ] {
        let (root, push_id, path) = uncertain_draft_fixture(&format!("recover-{variant}"));
        let mut item = read_json(&path);
        match variant {
            "smtp" => item["step_states"][1]["status"] = json!("remote_outcome_unknown"),
            "started" => item["step_states"][1]["started_rfc3339"] = json!("2026-10-19T09:32:18Z"),
            "truncated" => {
                item["step_states"].as_array_mut().unwrap().pop();
            }
            "changed" => {
                let mut config = read_json(&root.join(".afmail/config.json"));
                config["actions"]["draft"]["send"]["steps"] = json!([{"smtp_send": {}}]);
                write_json(&root.join(".afmail/config.json"), &config);
            }
            "local" => item["local_stage"] = json!("remote_succeeded"),
            _ => unreachable!(),
        }
        write_json(&path, &item);
        let before = fs::read(&path).unwrap();
        let (status, output) = run(
            &root,
            &[
                "push",
                "recover",
                &push_id,
                "--confirm",
                "push_recover_plan_unseen",
            ],
        );
        assert_eq!(status, 1, "{output}");
        assert_eq!(parse_one(&output)["code"], code, "{output}");
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(feature = "smtp")]
#[test]
fn push_append_rejection_before_literal_keeps_server_response_and_allows_retry() {
    let _guard = NETWORK_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (root, push_id, path) = uncertain_draft_fixture("append-preflight-rejection");
    let mut item = read_json(&path);
    item["step_states"] = json!([]);
    write_json(&path, &item);
    let server = start_imap_server_with_capabilities(
        2,
        ImapServerCapabilities {
            reject_append_before_literal: true,
            wait_for_all_connections: true,
            ..Default::default()
        },
    )
    .unwrap();
    configure_imap_without_smtp(&root, server.addr.port());
    for _ in 0..2 {
        let (status, output) = run_confirmed_push(&root);
        assert_eq!(status, 1, "{output}");
        let result = parse_one(&output);
        assert_eq!(result["failures"][0]["push_id"], push_id);
        assert_eq!(result["failures"][0]["error"]["code"], "imap_append_failed");
        assert!(
            result["failures"][0]["error"]["server_response"]
                .as_str()
                .unwrap()
                .contains("[OVERQUOTA] Drafts quota exceeded")
        );
        let item = read_json(&path);
        assert_eq!(item["step_states"][0]["status"], "failed");
        assert!(
            item["step_states"][0]["error"]
                .as_str()
                .unwrap()
                .contains("[OVERQUOTA]")
        );
        assert_eq!(item["step_states"][1]["status"], "pending");
    }
    assert!(server.appended.lock().unwrap().is_empty());
    server.handle.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_absence_releases_the_draft_for_normal_local_cancellation() {
    let _guard = NETWORK_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (root, push_id, path) = uncertain_draft_fixture("recover-cancel");
    let item = read_json(&path);
    let server = start_imap_server_with_capabilities(
        2,
        ImapServerCapabilities {
            search: SearchResponse::Absent,
            ..Default::default()
        },
    )
    .unwrap();
    configure_imap_without_smtp(&root, server.addr.port());
    let preview = parse_one(&run(&root, &["push", "recover", &push_id]).1);
    let plan_id = preview["plan_id"].as_str().unwrap_or_default().to_string();
    let (status, output) = run(&root, &["push", "recover", &push_id, "--confirm", &plan_id]);
    assert_eq!(status, 0, "{output}");
    let (status, output) = run(
        &root,
        &[
            "case",
            "draft",
            "remove",
            item["case_uid"].as_str().unwrap(),
            item["draft_name"].as_str().unwrap(),
            "--reason",
            "cancel after explicit remote absence verification",
        ],
    );
    assert_eq!(status, 0, "{output}");
    assert!(!path.exists());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap();
    assert!(audit.contains("Could not append mail to mailbox"));
    assert!(server.appended.lock().unwrap().is_empty());
    server.handle.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// An uncertain SMTP send after a completed Drafts APPEND: the shape every
/// send-settlement test starts from.
#[cfg(feature = "smtp")]
fn uncertain_send_fixture(name: &str) -> (std::path::PathBuf, String, std::path::PathBuf) {
    let (root, push_id, path) = uncertain_draft_fixture(name);
    let mut item = read_json(&path);
    item["step_states"] = json!([
        {"index":0,"label":"append_to_mailbox_id_drafts","status":"succeeded", "started_rfc3339":"2026-10-19T09:32:17Z", "completed_rfc3339":"2026-10-19T09:32:18Z"},
        {"index":1,"label":"smtp_send","status":"remote_outcome_unknown", "started_rfc3339":"2026-10-19T09:32:19Z", "error_code":"smtp_send_failed", "error":"connection reset after DATA", "retryable":false},
        {"index":2,"label":"append_to_mailbox_id_sent","status":"pending"}
    ]);
    write_json(&path, &item);
    (root, push_id, path)
}

#[cfg(feature = "smtp")]
fn recover_send_preview(root: &std::path::Path, push_id: &str) -> Value {
    let (status, output) = run(root, &["push", "recover", push_id]);
    assert_eq!(status, 0, "{output}");
    parse_one(&output)
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_previews_an_uncertain_send_with_its_evidence_and_changes_nothing() {
    let (root, push_id, path) = uncertain_send_fixture("recover-send-preview");
    configure_imap_without_smtp(&root, 9);
    let before = fs::read(&path).unwrap();
    let value = recover_send_preview(&root, &push_id);
    assert_eq!(value["recovery_kind"], "send_outcome");
    assert_eq!(value["confirmed"], false);
    assert_eq!(value["settlement"], Value::Null);
    assert_eq!(value["evidence"]["step_index"], 2);
    assert_eq!(value["evidence"]["label"], "smtp_send");
    assert_eq!(value["evidence"]["error_code"], "smtp_send_failed");
    assert_eq!(value["evidence"]["envelope_to"], json!(["bob@example.com"]));
    assert!(
        value["evidence"]["rfc822_message_id"]
            .as_str()
            .unwrap()
            .ends_with("@afmail.local>")
    );
    assert!(
        value["plan_id"]
            .as_str()
            .unwrap()
            .starts_with("push_recover_plan_")
    );
    // The check is deterministic and read-only.
    assert_eq!(
        recover_send_preview(&root, &push_id)["plan_id"],
        value["plan_id"]
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    // A settlement always names its plan, and only one assertion at a time.
    for args in [
        vec!["--sent"],
        vec!["--not-sent"],
        vec![
            "--sent",
            "--not-sent",
            "--confirm",
            value["plan_id"].as_str().unwrap(),
        ],
    ] {
        let mut argv = vec!["push", "recover", push_id.as_str()];
        argv.extend(args);
        let (status, output) = run(&root, &argv);
        assert_ne!(status, 0, "{output}");
    }
    // Confirming without an assertion settles nothing.
    let (status, output) = run(
        &root,
        &[
            "push",
            "recover",
            &push_id,
            "--confirm",
            value["plan_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(status, 1, "{output}");
    assert_eq!(
        parse_one(&output)["code"],
        "push_recovery_settlement_required"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_sent_completes_only_the_send_and_the_next_push_runs_later_steps() {
    let _guard = NETWORK_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (root, push_id, path) = uncertain_send_fixture("recover-send-sent");
    let server = start_imap_server(1).unwrap();
    // SMTP is unconfigured: a replayed send could not succeed.
    configure_imap_without_smtp(&root, server.addr.port());
    let plan_id = recover_send_preview(&root, &push_id)["plan_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, output) = run(
        &root,
        &["push", "recover", &push_id, "--sent", "--confirm", &plan_id],
    );
    assert_eq!(status, 0, "{output}");
    let value = parse_one(&output);
    assert_eq!(value["confirmed"], true);
    assert_eq!(value["settlement"], "sent");
    assert_eq!(value["asserted_by"], "person");
    let item = read_json(&path);
    assert_eq!(item["step_states"][1]["status"], "succeeded");
    assert_eq!(item["step_states"][2]["status"], "pending");
    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap();
    let event: Value = audit
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|event| event["kind"] == "push_recovery_settled")
        .unwrap();
    assert_eq!(event["settlement"], "sent");
    assert_eq!(event["asserted_by"], "person");
    assert_eq!(
        event["previous_step_states"][1]["status"],
        "remote_outcome_unknown"
    );
    // The same plan cannot be applied twice.
    let (status, output) = run(
        &root,
        &["push", "recover", &push_id, "--sent", "--confirm", &plan_id],
    );
    assert_eq!(status, 1, "{output}");
    // The next confirmed push appends the Sent copy and never sends.
    let (status, output) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{output}");
    assert_eq!(server.appended.lock().unwrap().len(), 1);
    assert!(!path.exists());
    server.handle.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_not_sent_requires_a_new_confirmed_push_before_sending_again() {
    let (root, push_id, path) = uncertain_send_fixture("recover-send-not-sent");
    configure_imap_without_smtp(&root, 9);
    let old_push_plan_id = current_push_plan_id(&root);
    let plan_id = recover_send_preview(&root, &push_id)["plan_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, output) = run(
        &root,
        &[
            "push",
            "recover",
            &push_id,
            "--not-sent",
            "--confirm",
            &plan_id,
        ],
    );
    assert_eq!(status, 0, "{output}");
    assert_eq!(parse_one(&output)["settlement"], "not_sent");
    let item = read_json(&path);
    assert_eq!(item["step_states"][0]["status"], "succeeded");
    assert_eq!(item["step_states"][1]["status"], "pending");
    assert_eq!(item["step_states"][1]["retryable"], true);
    assert!(item["step_states"][1].get("started_rfc3339").is_none());
    let audit = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap();
    assert!(audit.contains("push_recovery_settled"));
    // A push plan reviewed before the settlement no longer applies.
    assert_ne!(current_push_plan_id(&root), old_push_plan_id);
    let (status, output) = run(&root, &["push", "--confirm", &old_push_plan_id]);
    assert_eq!(status, 1, "{output}");
    assert_eq!(parse_one(&output)["code"], "push_plan_changed");
    assert_eq!(read_json(&path)["step_states"][1]["status"], "pending");
    // Nothing uncertain remains to settle.
    let (status, output) = run(
        &root,
        &["push", "recover", &push_id, "--sent", "--confirm", &plan_id],
    );
    assert_eq!(status, 1, "{output}");
    assert_eq!(
        parse_one(&output)["code"],
        "push_recovery_settlement_unsupported"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "smtp")]
#[test]
fn push_recover_refuses_a_stale_settlement_plan() {
    let (root, push_id, path) = uncertain_send_fixture("recover-send-stale");
    configure_imap_without_smtp(&root, 9);
    let plan_id = recover_send_preview(&root, &push_id)["plan_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut item = read_json(&path);
    item["step_states"][1]["error"] = json!("connection reset before DATA");
    write_json(&path, &item);
    let before = fs::read(&path).unwrap();
    for settlement in ["--sent", "--not-sent"] {
        let (status, output) = run(
            &root,
            &[
                "push",
                "recover",
                &push_id,
                settlement,
                "--confirm",
                &plan_id,
            ],
        );
        assert_eq!(status, 1, "{output}");
        let error = parse_one(&output);
        assert_eq!(error["code"], "push_recovery_plan_changed", "{output}");
        assert_ne!(error["current_plan_id"], json!(plan_id));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    fs::remove_dir_all(root).unwrap();
}
