use fs4::FileExt;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NETWORK_TEST_LOCK: Mutex<()> = Mutex::new(());
static COMMAND_TEST_LOCK: Mutex<()> = Mutex::new(());

type ImapCommands = Arc<Mutex<Vec<String>>>;
type AppendedMessages = Arc<Mutex<Vec<(String, bool, String)>>>;
type CreatedFolders = Arc<Mutex<Vec<String>>>;
type MovedMessages = Arc<Mutex<Vec<(String, u64, String)>>>;
type StoredMessages = Arc<Mutex<Vec<(String, u64, String)>>>;

fn temp_root(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("afmail-it-{name}-{}-{stamp}", std::process::id()))
}

fn bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_afmail") {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(env!("CARGO_BIN_EXE_afmail")),
    }
}

fn run(cwd: &Path, args: &[&str]) -> (i32, String) {
    let _command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let output = Command::new(bin()).current_dir(cwd).args(args).output();
    assert!(output.is_ok());
    let output = match output {
        Ok(v) => v,
        Err(_) => return (99, String::new()),
    };
    let status = output.status.code().unwrap_or(99);
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    assert!(
        output.stderr.is_empty(),
        "stderr must stay empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (status, stdout)
}

fn parse_one(stdout: &str) -> Value {
    let parsed = serde_json::from_str(stdout.trim());
    assert!(parsed.is_ok(), "stdout was not JSON: {stdout}");
    match parsed {
        Ok(v) => v,
        Err(_) => Value::Null,
    }
}

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_one)
        .collect()
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        assert!(fs::create_dir_all(parent).is_ok());
    }
    let data = serde_json::to_string_pretty(value).unwrap_or_default();
    assert!(fs::write(path, data).is_ok());
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_default()).unwrap_or(Value::Null)
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
    assert_eq!(value["code"], "version");
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert!(value.get("trace").is_none(), "{stdout}");

    let (status, stdout) = run(&root, &["--version", "--output", "yaml"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.starts_with("---\n"), "{stdout}");
    assert!(stdout.contains("code: \"version\""), "{stdout}");
    assert!(
        stdout.contains(&format!("version: \"{}\"", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );
    assert!(!stdout
        .lines()
        .any(|line| line == format!("afmail {}", env!("CARGO_PKG_VERSION"))));

    let (status, stdout) = run(&root, &["--version", "--output", "plain"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.contains("code=version"), "{stdout}");
    assert!(
        stdout.contains(&format!("version={}", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );
    assert!(!stdout
        .lines()
        .any(|line| line == format!("afmail {}", env!("CARGO_PKG_VERSION"))));

    let _ = fs::remove_dir_all(root);
}

fn test_config(imap_port: Option<u16>, smtp_port: Option<u16>) -> Value {
    json!({
        "schema_name": "config",
        "schema_version": 1,
        "imap": {
            "host": imap_port.map(|_| "127.0.0.1"),
            "port": imap_port.unwrap_or(993),
            "tls": false,
            "username": imap_port.map(|_| "user"),
            "password_secret": imap_port.map(|_| "pass")
        },
        "mailboxes": {
            "inbox": {"mailbox_name": "INBOX", "special_use": null},
            "sent": {"mailbox_name": null, "special_use": "\\Sent"},
            "archive": {"mailbox_name": null, "special_use": "\\Archive"},
            "junk": {"mailbox_name": null, "special_use": "\\Junk"},
            "trash": {"mailbox_name": null, "special_use": "\\Trash"},
            "drafts": {"mailbox_name": null, "special_use": "\\Drafts"}
        },
        "actions": {
            "pull": {
                "default_mailbox_ids": ["inbox", "sent", "archive", "junk", "trash"],
                "by_mailbox_id": {
                    "inbox": {"import_as": "triage", "direction": "inbound"},
                    "sent": {"import_as": "triage", "direction": "outbound"},
                    "archive": {"import_as": "triage", "direction": "inbound"},
                    "junk": {"import_as": "spam", "direction": "inbound"},
                    "trash": {"import_as": "trashed", "direction": "inbound"},
                    "drafts": {"import_as": "triage", "direction": "outbound"}
                }
            },
            "case.add": {"steps": []},
            "draft.save": {"steps": [{"append_to_mailbox_id": "drafts"}]},
            "draft.send": {"steps": [
                {"smtp_send": {}},
                {"append_to_mailbox_id": "sent"},
                {"add_flags": ["\\Seen", "\\Answered"], "on": "reply_to_message"}
            ]},
            "message.spam": {"steps": [
                {"add_flags": ["\\Seen", "$Junk"]},
                {"move_to_mailbox_id": "junk"}
            ]},
            "message.trash": {"steps": [{"move_to_mailbox_id": "trash"}]},
            "message.archive": {"by_source_mailbox_id": {
                "inbox": {"steps": [{"move_to_mailbox_id": "archive"}]},
                "sent": {"steps": []},
                "archive": {"steps": []},
                "junk": {"steps": []},
                "trash": {"steps": []},
                "drafts": {"steps": []}
            }}
        },
        "case": {"default_group": "open"},
        "audit": {"reason_mode": "required"},
        "smtp": {
            "host": smtp_port.map(|_| "127.0.0.1"),
            "port": smtp_port.unwrap_or(587),
            "starttls": false,
            "tls_wrapper": false,
            "username": null,
            "password_secret": null,
            "from": "Me <me@example.com>"
        },
        "workspace": {"language_bcp47": null, "timezone_utc_offset": "UTC"}
    })
}

fn write_message(root: &Path, message_id: &str, uid: u64, source_path: Option<&str>) {
    let mut attachment = json!({
        "part_id": "2",
        "filename": "pricing.txt",
        "content_type": "text/plain",
        "size_bytes": 7,
        "fetched": false
    });
    if let Some(source) = source_path {
        attachment["source_path"] = json!(source);
    }
    let msg = json!({
        "schema_name": "message",
        "schema_version": 1,
        "message_id": message_id,
        "rfc822_message_id": format!("<{message_id}@example.com>"),
        "remote": {"locations": [{
            "mailbox_name": "INBOX",
            "mailbox_id": "inbox",
            "uid_validity": 1,
            "uid": uid,
            "flags": [],
            "observed_rfc3339": "2026-05-21T10:00:00Z"
        }]},
        "direction": "inbound",
        "subject": "Contract renewal",
        "from": "alice@example.com",
        "to": ["me@example.com"],
        "cc": [],
        "received_rfc3339": "2026-05-21T10:00:00Z",
        "body_text": "Body",
        "eml_path": format!(".afmail/messages/{message_id}.eml"),
        "attachments": [attachment],
        "workspace": {"status": "triage"}
    });
    write_json(&root.join(format!("messages/{message_id}.json")), &msg);
    write_json(
        &root.join(format!(".afmail/messages/{message_id}.state.json")),
        &json!({
            "schema_name": "message_state",
            "schema_version": 1,
            "message_id": message_id,
            "status": "triage",
            "updated_rfc3339": "2026-05-21T10:00:00Z"
        }),
    );
    write_json(
        &root.join(format!(".afmail/messages/{message_id}.remote.json")),
        &json!({
            "schema_name": "message_remote",
            "schema_version": 1,
            "message_id": message_id,
            "locations": msg["remote"]["locations"].clone()
        }),
    );
    assert!(fs::write(
        root.join(format!(".afmail/messages/{message_id}.eml")),
        format!(
            "Message-ID: <{message_id}@example.com>\r\nFrom: alice@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Contract renewal\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"afmail-test\"\r\n\r\n--afmail-test\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody\r\n--afmail-test\r\nContent-Type: text/plain; name=\"pricing.txt\"\r\nContent-Disposition: attachment; filename=\"pricing.txt\"\r\n\r\nPricing\r\n--afmail-test--\r\n"
        )
    )
    .is_ok());
}

fn update_message_json(root: &Path, message_id: &str, update: impl FnOnce(&mut Value)) {
    let path = root.join(format!("messages/{message_id}.json"));
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    update(&mut value);
    if let Some(workspace) = value.get("workspace") {
        write_json(
            &root.join(format!(".afmail/messages/{message_id}.state.json")),
            &json!({
                "schema_name": "message_state",
                "schema_version": 1,
                "message_id": message_id,
                "status": workspace.get("status").and_then(Value::as_str).unwrap_or("triage"),
                "archive_uid": workspace.get("archive_uid").cloned().unwrap_or(Value::Null),
                "archived_rfc3339": workspace.get("archived_rfc3339").cloned().unwrap_or(Value::Null),
                "origin": workspace.get("origin").cloned().unwrap_or(Value::Null),
                "updated_rfc3339": "2026-05-21T10:00:00Z"
            }),
        );
    }
    if value.get("remote").is_some() {
        write_json(
            &root.join(format!(".afmail/messages/{message_id}.remote.json")),
            &json!({
                "schema_name": "message_remote",
                "schema_version": 1,
                "message_id": message_id,
                "locations": value["remote"]["locations"].clone()
            }),
        );
    }
    write_json(&path, &value);
}

fn set_message_state_updated(root: &Path, message_id: &str, updated_rfc3339: &str) {
    let path = root.join(format!(".afmail/messages/{message_id}.state.json"));
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    value["updated_rfc3339"] = json!(updated_rfc3339);
    write_json(&path, &value);
}

/// Resolve the on-disk message id for a known RFC822 Message-ID.
///
/// Message ids are derived from the Message-ID, so tests look the id up from the
/// stored `rfc822_message_id` rather than hardcoding the (now opaque) format.
fn message_id_for_rfc822(root: &Path, rfc822: &str) -> String {
    let trim = |value: &str| {
        value
            .trim()
            .trim_matches(|ch| ch == '<' || ch == '>')
            .to_ascii_lowercase()
    };
    let needle = trim(rfc822);
    let dir = root.join("messages");
    let Ok(entries) = fs::read_dir(&dir) else {
        return String::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if let Some(stored) = value["rfc822_message_id"].as_str() {
            if trim(stored) == needle {
                return path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default();
            }
        }
    }
    // Returns empty when not found; the caller's path assertion then fails clearly.
    String::new()
}

fn write_triage(root: &Path, message_id: &str, notes: &str) {
    let text = format!(
        "---\nkind: triage_view\nmessage_id: {message_id}\nmessage_ids:\n  - {message_id}\ngenerated_rfc3339: \"2026-05-21T10:00:02Z\"\nmessage_count: 1\nattachment_count: 1\n---\n\n# Contract renewal\n\n<!-- afmail:conversation:start -->\n\n### {message_id} - 2026-05-21T10:00:00Z - Alice <alice@example.com>\n\n```text\nBody for {message_id}\n```\n\n<!-- afmail:conversation:end -->\n"
    );
    let _ = notes;
    assert!(fs::write(root.join(format!("triage/{message_id}.md")), text).is_ok());
}

fn create_case(
    root: &Path,
    name: &str,
    group: Option<&str>,
    message_id: Option<&str>,
    reason: Option<&str>,
) -> (String, PathBuf, Value) {
    let mut args = vec![
        "case".to_string(),
        "create".to_string(),
        "--name".to_string(),
        name.to_string(),
    ];
    if let Some(group) = group {
        args.push("--group".to_string());
        args.push(group.to_string());
    }
    if let Some(message_id) = message_id {
        args.push("--message".to_string());
        args.push(message_id.to_string());
    }
    if let Some(reason) = reason {
        args.push("--reason".to_string());
        args.push(reason.to_string());
    }
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let (status, stdout) = run(root, &refs);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let case_uid = value["case_uid"].as_str().unwrap_or_default().to_string();
    assert!(!case_uid.is_empty(), "{value}");
    let case_path = value["case_path"].as_str().unwrap_or_default();
    assert!(!case_path.is_empty(), "{value}");
    (case_uid, root.join(case_path), value)
}

fn create_archive_message(
    root: &Path,
    name: &str,
    message_id: Option<&str>,
    summary: Option<&str>,
    reason: Option<&str>,
) -> (String, PathBuf, Value) {
    let mut args = vec![
        "archive".to_string(),
        "message".to_string(),
        "create".to_string(),
        "--name".to_string(),
        name.to_string(),
    ];
    if let Some(message_id) = message_id {
        args.push("--message".to_string());
        args.push(message_id.to_string());
    }
    if let Some(summary) = summary {
        args.push("--summary".to_string());
        args.push(summary.to_string());
    }
    if let Some(reason) = reason {
        args.push("--reason".to_string());
        args.push(reason.to_string());
    }
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let (status, stdout) = run(root, &refs);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let archive_uid = value["archive_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!archive_uid.is_empty(), "{value}");
    let path = value["path"].as_str().unwrap_or_default();
    assert!(!path.is_empty(), "{value}");
    (archive_uid, root.join(path), value)
}

fn write_case_fixture(
    root: &Path,
    group: &str,
    case_uid: &str,
    case_name: &str,
    message_ids: &[&str],
) -> PathBuf {
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
            "kind": "case",
            "case_uid": case_uid,
            "case_name": case_name,
            "status": "active",
            "tags": [],
            "created_rfc3339": "2026-05-21T10:00:00Z",
            "updated_rfc3339": "2026-05-21T10:00:00Z",
            "message_count": message_ids.len(),
            "thread_count": 0,
            "attachment_count": 0,
            "last_message_rfc3339": "2026-05-21T10:00:00Z"
        }),
    );
    assert!(fs::write(case_dir.join("case.md"), format!("# {case_name}\n")).is_ok());
    write_json(
        &case_dir.join("data/messages.json"),
        &json!({
            "schema_name": "case_messages",
            "schema_version": 1,
            "case_uid": case_uid,
            "message_ids": message_ids
        }),
    );
    case_dir
}

fn bind_loopback_listener() -> Option<TcpListener> {
    let _command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for _ in 0..200 {
        if let Ok(listener) = TcpListener::bind("127.0.0.1:0") {
            return Some(listener);
        }
        thread::sleep(Duration::from_millis(20));
    }
    None
}

struct ImapTestServer {
    addr: SocketAddr,
    commands: ImapCommands,
    appended: AppendedMessages,
    moved: MovedMessages,
    stored: StoredMessages,
    handle: thread::JoinHandle<()>,
}

fn start_imap_server(messages: Vec<(u64, Vec<u8>)>, accept_count: usize) -> Option<ImapTestServer> {
    start_imap_server_with_move(messages, accept_count, true)
}

fn start_imap_server_with_move(
    messages: Vec<(u64, Vec<u8>)>,
    accept_count: usize,
    move_supported: bool,
) -> Option<ImapTestServer> {
    let listener = bind_loopback_listener()?;
    let addr = listener.local_addr().ok()?;
    let commands = Arc::new(Mutex::new(Vec::new()));
    let appended = Arc::new(Mutex::new(Vec::new()));
    let created = Arc::new(Mutex::new(Vec::new()));
    let moved = Arc::new(Mutex::new(Vec::new()));
    let stored = Arc::new(Mutex::new(Vec::new()));
    let server_commands = Arc::clone(&commands);
    let server_appended = Arc::clone(&appended);
    let server_created = Arc::clone(&created);
    let server_moved = Arc::clone(&moved);
    let server_stored = Arc::clone(&stored);
    let handle = thread::spawn(move || {
        // `accept_count` is an upper bound: serve up to that many connections, but
        // exit once the client goes idle so a pull that opens fewer connections
        // doesn't block join() forever.
        let _ = listener.set_nonblocking(true);
        let mut served = 0usize;
        let mut last = Instant::now();
        while served < accept_count {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    serve_imap_connection(
                        stream,
                        &messages,
                        &server_commands,
                        &server_appended,
                        &server_created,
                        &server_moved,
                        &server_stored,
                        move_supported,
                    );
                    served += 1;
                    last = Instant::now();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if last.elapsed() > Duration::from_millis(800) {
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
        commands,
        appended,
        moved,
        stored,
        handle,
    })
}

#[allow(clippy::too_many_arguments)]
fn serve_imap_connection(
    mut writer: TcpStream,
    messages: &[(u64, Vec<u8>)],
    commands: &ImapCommands,
    appended: &AppendedMessages,
    created: &CreatedFolders,
    moved: &MovedMessages,
    stored: &StoredMessages,
    move_supported: bool,
) {
    let reader_stream = match writer.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let _ = writer.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(reader_stream);
    if write_imap(&mut writer, b"* OK afmail test imap ready\r\n").is_err() {
        return;
    }
    let mut selected_mailbox = String::from("INBOX");
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line);
        let Ok(read) = read else {
            return;
        };
        if read == 0 {
            return;
        }
        if handle_append_command(&mut writer, &mut reader, &line, commands, appended).is_some() {
            continue;
        }
        let parsed = parse_imap_command(&line);
        let Some((tag, name, is_uid_fetch)) = parsed else {
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
                let caps = if move_supported {
                    "IMAP4rev1 MOVE"
                } else {
                    "IMAP4rev1"
                };
                let response = format!("* CAPABILITY {caps}\r\n{tag} OK CAPABILITY completed\r\n");
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "LIST" => {
                let response = format!(
                    "* LIST (\\Noinferiors) \"/\" \"INBOX\"\r\n* LIST (\\Sent) \"/\" \"Sent\"\r\n* LIST (\\Drafts) \"/\" \"Drafts\"\r\n* LIST (\\Archive) \"/\" \"Archive\"\r\n* LIST (\\Junk) \"/\" \"Junk\"\r\n* LIST (\\Trash) \"/\" \"Trash\"\r\n* LIST (\\All) \"/\" \"All Mail\"\r\n* LIST (\\Flagged) \"/\" \"Flagged\"\r\n{tag} OK LIST completed\r\n"
                );
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "CREATE" => {
                if let Some(folder) = folder_arg_from_line(&line, "CREATE") {
                    if let Ok(mut locked) = created.lock() {
                        locked.push(folder);
                    }
                }
                if write_imap(
                    &mut writer,
                    format!("{tag} OK CREATE completed\r\n").as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            "SELECT" | "EXAMINE" => {
                if let Some(folder) = folder_arg_from_line(&line, &name) {
                    selected_mailbox = folder;
                }
                let exists = messages.len();
                let response = format!(
                    "* FLAGS ()\r\n* {exists} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY 44] UIDs valid\r\n* OK [UIDNEXT 100] Next UID\r\n{tag} OK [READ-WRITE] Select completed\r\n"
                );
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "FETCH" if is_uid_fetch => {
                if write_imap_fetch(&mut writer, &tag, messages, &line).is_err() {
                    return;
                }
            }
            "SEARCH" => {
                let text = String::from_utf8_lossy(&line);
                let upper = text.to_ascii_uppercase();
                let uids = if upper.contains(" UID SEARCH ALL") {
                    messages
                        .iter()
                        .map(|(uid, _)| uid.to_string())
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    "900".to_string()
                };
                let response = format!("* SEARCH {uids}\r\n{tag} OK SEARCH completed\r\n");
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "STORE" => {
                if let Some((uid, query)) = uid_store_args_from_line(&line) {
                    if let Ok(mut locked) = stored.lock() {
                        locked.push((selected_mailbox.clone(), uid, query));
                    }
                }
                let response = format!("{tag} OK STORE completed\r\n");
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "MOVE" => {
                if !move_supported {
                    let _ = write_imap(
                        &mut writer,
                        format!("{tag} BAD MOVE unsupported\r\n").as_bytes(),
                    );
                    return;
                }
                if let Some((uid, target)) = uid_move_args_from_line(&line) {
                    if let Ok(mut locked) = moved.lock() {
                        locked.push((selected_mailbox.clone(), uid, target));
                    }
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
    commands: &ImapCommands,
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
    if let Ok(mut locked) = commands.lock() {
        locked.push("APPEND".to_string());
    }
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
    let _ = write_imap(writer, format!("{tag} OK APPEND completed\r\n").as_bytes());
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

fn uid_store_args_from_line(line: &[u8]) -> Option<(u64, String)> {
    let text = String::from_utf8_lossy(line);
    let upper = text.to_ascii_uppercase();
    let marker = " UID STORE ";
    let start = upper.find(marker)? + marker.len();
    let rest = text.get(start..)?.trim();
    let (uid_text, query) = rest.split_once(' ')?;
    Some((uid_text.parse::<u64>().ok()?, query.trim().to_string()))
}

fn parse_imap_command(line: &[u8]) -> Option<(String, String, bool)> {
    use imap_codec::decode::Decoder;
    use imap_codec::imap_types::command::CommandBody;
    let decoded = imap_codec::CommandCodec::default().decode(line).ok()?;
    if !decoded.0.is_empty() {
        return None;
    }
    let command = decoded.1;
    let is_uid_fetch = matches!(&command.body, CommandBody::Fetch { uid: true, .. });
    Some((
        command.tag.inner().to_string(),
        command.name().to_string(),
        is_uid_fetch,
    ))
}

fn tag_from_line(line: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    text.split_whitespace().next().map(ToString::to_string)
}

fn write_imap(writer: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    writer.write_all(data)?;
    writer.flush()
}

fn write_imap_fetch(
    writer: &mut TcpStream,
    tag: &str,
    messages: &[(u64, Vec<u8>)],
    line: &[u8],
) -> std::io::Result<()> {
    let query = String::from_utf8_lossy(line).to_ascii_uppercase();
    let header_only = query.contains("BODY.PEEK[HEADER]");
    let header_fields_only = query.contains("BODY.PEEK[HEADER.FIELDS");
    for (idx, (uid, raw)) in messages.iter().enumerate() {
        let seq = idx + 1;
        let body = if header_only || header_fields_only {
            raw_message_header(raw)
        } else {
            raw.as_slice()
        };
        let len = body.len();
        let raw_len = raw.len();
        let flags = test_flags_from_raw(raw);
        let body_key = if header_fields_only {
            "BODY[HEADER.FIELDS (MESSAGE-ID IN-REPLY-TO REFERENCES DATE)]"
        } else if header_only {
            "BODY[HEADER]"
        } else {
            "BODY[]"
        };
        write!(writer, "* {seq} FETCH (UID {uid} FLAGS ({flags})")?;
        if query.contains("RFC822.SIZE") {
            write!(writer, " RFC822.SIZE {raw_len}")?;
        }
        write!(writer, " {body_key} {{{len}}}\r\n")?;
        writer.write_all(body)?;
        writer.write_all(b")\r\n")?;
    }
    write!(writer, "{tag} OK FETCH completed\r\n")?;
    writer.flush()
}

fn raw_message_header(raw: &[u8]) -> &[u8] {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| &raw[..pos + 4])
        .unwrap_or(raw)
}

fn test_flags_from_raw(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    text.lines()
        .find_map(|line| line.strip_prefix("X-Afmail-Test-Flags:"))
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

struct SmtpTestServer {
    addr: SocketAddr,
    data_rx: mpsc::Receiver<String>,
    handle: thread::JoinHandle<()>,
}

fn start_smtp_server() -> Option<SmtpTestServer> {
    let listener = bind_loopback_listener()?;
    let addr = listener.local_addr().ok()?;
    let (data_tx, data_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let accepted = listener.accept();
        let Ok((stream, _)) = accepted else {
            return;
        };
        serve_smtp_connection(stream, data_tx);
    });
    Some(SmtpTestServer {
        addr,
        data_rx,
        handle,
    })
}

fn start_smtp_server_with_accept_timeout(timeout: Duration) -> Option<SmtpTestServer> {
    let listener = bind_loopback_listener()?;
    listener.set_nonblocking(true).ok()?;
    let addr = listener.local_addr().ok()?;
    let (data_tx, data_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    serve_smtp_connection(stream, data_tx);
                    return;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    Some(SmtpTestServer {
        addr,
        data_rx,
        handle,
    })
}

fn serve_smtp_connection(mut writer: TcpStream, data_tx: mpsc::Sender<String>) {
    let reader_stream = match writer.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let _ = writer.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(reader_stream);
    if write_smtp(&mut writer, b"220 afmail test smtp ready\r\n").is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line);
        let Ok(read) = read else {
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
                let read = reader.read_line(&mut data_line);
                let Ok(read) = read else {
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
        } else {
            if write_smtp(&mut writer, b"250 OK\r\n").is_err() {
                return;
            }
        }
    }
}

fn write_smtp(writer: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    writer.write_all(data)?;
    writer.flush()
}

#[test]
fn init_status_and_afdata_output_formats_work() {
    let root = temp_root("init");
    assert!(fs::create_dir_all(&root).is_ok());
    let (status, stdout) = run(&root, &["--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("Agent-First Mail"));
    assert!(stdout.contains("afmail case"));
    assert!(stdout.contains("afmail pull"));
    assert!(stdout.contains("afmail push"));
    assert!(!stdout.contains("redirect"));
    assert!(!stdout.contains("List queued local push items"));
    assert!(!stdout.contains("message_inbox_607146690_25 ignore"));
    assert!(!stdout.contains("outbox"));
    assert!(!stdout.to_ascii_lowercase().contains("bucket"));
    let (status, stdout) = run(&root, &["outbox", "list"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "error");
    assert!(value["trace"]["duration_ms"].as_u64().is_some(), "{stdout}");
    let (status, stdout) = run(&root, &["message", "--help"]);
    assert_eq!(status, 0);
    assert!(!stdout.contains("ignore"));
    let (status, stdout) = run(&root, &["case", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("show"));
    assert!(stdout.contains("add"));
    assert!(stdout.contains("archive"));
    assert!(stdout.contains("reply"));
    assert!(stdout.contains("draft"));
    assert!(stdout.contains("notes"));
    assert!(stdout.contains("merge"));
    let (status, stdout) = run(&root, &["case", "add", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("<CASE_REF> <MESSAGE_ID>"));
    assert!(stdout.contains("--reason"));
    let (status, stdout) = run(&root, &["archive", "message", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("create"));
    assert!(stdout.contains("show"));
    assert!(stdout.contains("restore"));
    assert!(stdout.contains("move"));
    assert!(stdout.contains("rename"));
    assert!(stdout.contains("set-summary"));
    assert!(stdout.contains("notes"));
    let (status, stdout) = run(&root, &["archive", "case", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("show"));
    assert!(stdout.contains("restore"));
    assert!(stdout.contains("rename"));
    assert!(stdout.contains("notes"));
    let (status, stdout) = run(&root, &["message", "ignore", "message_001"]);
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");
    let (status, stdout) = run(&root, &["message", "message_001", "show"]);
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");
    let (status, stdout) = run(&root, &["case", "c20260603001", "add", "message_001"]);
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");
    let (status, stdout) = run(&root, &["--help", "--output", "markdown"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("# afmail"));
    assert!(stdout.contains("afmail case show c20260603001"));
    assert!(stdout.contains("afmail archive case show REF"));
    assert!(stdout.contains("afmail archive message show a20260603001"));
    assert!(!stdout.contains("afmail message ignore"));
    assert!(!stdout.contains("--mode"));
    let (status, stdout) = run(&root, &["--mode", "pipe"]);
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");
    assert!(!stdout.to_ascii_lowercase().contains("bucket"));

    let (status, stdout) = run(&root, &["init"]);
    assert_eq!(status, 0);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "workspace_initialized");
    assert_eq!(value["agent_skill_created"], true);
    assert_eq!(value["agent_skill_updated"], false);
    assert_eq!(value["agent_skill_path"], "AGENTS.md");
    assert_eq!(value["gitignore_created"], true);
    assert_eq!(value["gitignore_updated"], false);
    assert_eq!(value["gitignore_path"], ".gitignore");
    assert_eq!(value["do_not_edit_created"], true);
    assert_eq!(value["do_not_edit_path"], ".afmail/DO_NOT_EDIT.txt");
    assert!(root.join(".afmail/transactions").is_dir());
    assert!(!root.join("buckets").exists());
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    assert!(gitignore.contains("# BEGIN afmail managed"));
    assert!(!gitignore.contains(".afmail/messages/"));
    assert!(!gitignore.contains(".afmail/push/"));
    assert!(gitignore.contains(".afmail/workspace.progress.json"));
    assert!(gitignore.contains("messages/*.json"));
    assert!(gitignore.contains("triage/*.md"));
    assert!(gitignore.contains("spam/*.md"));
    assert!(gitignore.contains("trash/*.md"));
    assert!(gitignore.contains("deleted/*.md"));
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "status");
    assert_eq!(value["progress"]["status"], "idle");
    let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap_or_default();
    assert!(agents.contains("Mailbox Agent Notes"));
    assert!(agents.contains("<!-- BEGIN afmail managed -->"));
    assert!(agents.contains("afmail skill install"));
    let do_not_edit = root.join(".afmail/DO_NOT_EDIT.txt");
    assert!(do_not_edit.is_file());
    assert!(fs::write(&do_not_edit, "custom warning").is_ok());
    assert!(fs::write(root.join("AGENTS.md"), "custom agents file").is_ok());
    assert!(fs::write(root.join(".gitignore"), "*.tmp\n").is_ok());
    let (status, stdout) = run(&root, &["init"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["agent_skill_created"], false);
    assert_eq!(value["agent_skill_updated"], true);
    assert_eq!(value["gitignore_created"], false);
    assert_eq!(value["gitignore_updated"], true);
    assert_eq!(value["do_not_edit_created"], false);
    let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap_or_default();
    assert!(agents.contains("custom agents file"));
    assert!(agents.contains("<!-- BEGIN afmail managed -->"));
    assert!(agents.contains("afmail skill install"));
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    assert!(gitignore.contains("*.tmp"));
    assert!(gitignore.contains("# BEGIN afmail managed"));
    assert!(gitignore.contains("messages/*.json"));
    assert!(gitignore.contains("archive/notifications/*/views/**/*.md"));
    assert_eq!(
        fs::read_to_string(root.join(".afmail/DO_NOT_EDIT.txt")).unwrap_or_default(),
        "custom warning"
    );
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "message_001",
            "promote",
            "case-one",
            "--group",
            "open",
        ],
    );
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");
    let config = fs::read_to_string(root.join(".afmail/config.json")).unwrap_or_default();
    assert!(config.contains("\"imap\""));
    assert!(config.contains("\"case\""));
    assert!(config.contains("\"audit\""));
    assert!(config.contains("\"reason_mode\": \"required\""));
    assert!(config.contains("\"mailboxes\""));
    assert!(config.contains("\"actions\""));
    assert!(config.contains("\"move_to_mailbox_id\": \"archive\""));
    let config_json: Value = serde_json::from_str(&config).unwrap_or(Value::Null);
    assert_eq!(config_json["schema_name"], "config");
    assert_eq!(config_json["schema_version"], 1);
    assert!(config_json.get("code").is_none());
    assert!(config_json.get("special_use").is_none());
    assert!(config_json.get("imap_mailboxes").is_none());
    assert!(config_json.get("pull").is_none());
    assert!(config_json.get("push").is_none());
    assert!(config_json["mailboxes"].get("all").is_none());
    assert!(config_json["mailboxes"].get("flagged").is_none());
    assert!(!config.contains("\"folders\": {"));
    assert!(!config.contains("imap_host"));

    let (status, stdout) = run(&root, &["config", "pull", "add", "Sent"]);
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");
    let (status, stdout) = run(
        &root,
        &["config", "get", "actions.pull.default_mailbox_ids"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["value"],
        json!(["inbox", "sent", "archive", "junk", "trash"])
    );
    let (status, stdout) = run(&root, &["config", "set", "imap.host", "imap.example.com"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["key"], "imap.host");
    let (status, stdout) = run(
        &root,
        &["config", "set", "imap.password_secret", "super-secret"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "***");
    let (status, stdout) = run(&root, &["config", "get", "imap.password_secret"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "***");
    let config = fs::read_to_string(root.join(".afmail/config.json")).unwrap_or_default();
    assert!(config.contains("super-secret"));
    let (status, stdout) = run(
        &root,
        &[
            "config",
            "set",
            "imap.password_secret_env",
            "AFMAIL_IMAP_PASSWORD_SECRET",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "AFMAIL_IMAP_PASSWORD_SECRET");
    let (status, stdout) = run(&root, &["config", "get", "imap.password_secret"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], Value::Null);
    let (status, stdout) = run(
        &root,
        &["config", "set", "mailboxes.archive.mailbox_name", "Archive"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "Archive");
    let (status, stdout) = run(&root, &["config", "get", "case.default_group"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "open");
    let (status, stdout) = run(&root, &["config", "set", "case.default_group", "todo"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "todo");
    let (status, stdout) = run(&root, &["config", "get", "audit.reason_mode"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "required");
    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "optional"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "optional");
    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "required"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "required");

    let (status, stdout) = run(&root, &["--output", "plain", "status"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("code=status"));
    assert!(stdout.contains("triage_count=0"));

    let (status, stdout) = run(&root, &["--log", "startup,request,progress", "status"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    assert_eq!(lines.len(), 4, "{stdout}");
    assert_eq!(lines[0]["code"], "log");
    assert_eq!(lines[0]["event"], "startup");
    assert_eq!(lines[0]["command"], "status");
    assert_eq!(lines[1]["event"], "request");
    assert_eq!(lines[2]["event"], "progress");
    assert_eq!(lines[2]["success"], true);
    for line in &lines[0..3] {
        assert_eq!(line["code"], "log");
        assert_eq!(line["level"], "info");
        assert!(line["event"].as_str().is_some(), "{stdout}");
        assert!(line["timestamp_epoch_ms"].as_u64().is_some(), "{stdout}");
        assert!(line["message"].as_str().is_some(), "{stdout}");
        assert!(line["trace"]["duration_ms"].as_u64().is_some(), "{stdout}");
    }
    assert_eq!(lines[3]["code"], "status");
    assert!(lines[3]["trace"]["duration_ms"].as_u64().is_some());

    let (status, stdout) = run(&root, &["--verbose", "status"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    assert_eq!(lines.last().unwrap_or(&Value::Null)["code"], "status");

    let (status, stdout) = run(
        &root,
        &[
            "--log",
            "startup",
            "config",
            "set",
            "imap.password_secret",
            "log-secret",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert!(!stdout.contains("log-secret"), "{stdout}");
    let lines = parse_lines(&stdout);
    let argv = lines[0]["argv"].as_array().cloned().unwrap_or_default();
    assert!(argv.iter().any(|arg| arg == "***"), "{stdout}");

    let (status, stdout) = run(&root, &["--log", "bogus", "status"]);
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");

    let (status, stdout) = run(&root, &["--log", "redirect", "status"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "error");
    assert!(value["trace"]["duration_ms"].as_u64().is_some(), "{stdout}");
    let error = value["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("--log unsupported category 'redirect'"),
        "{stdout}"
    );
    assert!(
        error.contains("expected one of: startup, request, progress, retry"),
        "{stdout}"
    );
    assert!(!error.contains("retry, redirect"), "{stdout}");

    let (status, stdout) = run(&root, &["--output", "xml", "status"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "error");
    assert!(value["trace"]["duration_ms"].as_u64().is_some(), "{stdout}");
    let (status, stdout) = run(&root, &["--output", "yaml", "nope"]);
    assert_eq!(status, 2);
    assert!(stdout.starts_with("---\n"), "{stdout}");
    assert!(stdout.contains("code: \"error\""), "{stdout}");
    assert!(stdout.contains("duration:"), "{stdout}");
    assert!(!stdout.contains("error_code:"), "{stdout}");
    let (status, stdout) = run(&root, &["--output=plain", "nope"]);
    assert_eq!(status, 2);
    assert!(stdout.contains("code=error"), "{stdout}");
    assert!(stdout.contains("trace.duration="), "{stdout}");
    assert!(!stdout.contains("error_code="), "{stdout}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn status_progress_uses_progress_percent_field() {
    let root = temp_root("status-progress-percent");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(
        &root.join(".afmail/workspace.progress.json"),
        &json!({
            "schema_name": "workspace_progress",
            "schema_version": 1,
            "command": "pull",
            "status": "running",
            "phase": "pull_mailbox_bodies_progress",
            "started_rfc3339": "2026-06-11T00:00:00Z",
            "updated_rfc3339": "2026-06-11T00:00:01Z",
            "elapsed_ms": 1000,
            "fields": {
                "mailbox_id": "inbox",
                "mailbox_name": "INBOX",
                "processed_count": 1,
                "uid_count": 4
            },
            "result": null,
            "error": null
        }),
    );

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["progress"]["progress_percent"], 25.0);
    assert!(
        value["progress"]
            .as_object()
            .is_some_and(|progress| !progress.contains_key("percent")),
        "{stdout}"
    );

    let (status, stdout) = run(&root, &["--output", "plain", "status"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(stdout.contains("progress.progress=25%"), "{stdout}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn doctor_reports_and_repairs_afmail_state_only() {
    let root = temp_root("doctor");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_doctor", 1, None);
    assert!(fs::remove_file(root.join(".afmail/messages/message_doctor.state.json")).is_ok());

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "doctor");
    assert_eq!(value["checks"]["git_checked"], false);
    assert_eq!(value["ok"], false);
    assert!(value["issues"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .any(|issue| issue["code"] == "message_state_missing"));

    let (status, stdout) = run(&root, &["doctor", "repair"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "confirm_required");

    let (status, stdout) = run(&root, &["doctor", "repair", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let repaired = parse_one(&stdout);
    assert_eq!(repaired["code"], "doctor_repair");
    assert!(root
        .join(".afmail/messages/message_doctor.state.json")
        .is_file());
}

#[test]
fn incomplete_transaction_blocks_writers_and_doctor_reports_it() {
    let root = temp_root("transaction-block");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(
        &root.join(".afmail/transactions/transaction_test.json"),
        &json!({
            "schema_name": "local_transaction",
            "schema_version": 1,
            "transaction_id": "transaction_test",
            "kind": "message_archive",
            "created_rfc3339": "2026-06-09T00:00:00Z",
            "paths": ["messages/message_1.json"]
        }),
    );

    let (status, stdout) = run(&root, &["config", "set", "case.default_group", "waiting"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "transaction_incomplete");

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 0, "{stdout}");
    let doctor = parse_one(&stdout);
    assert!(doctor["issues"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .any(|issue| issue["code"] == "transaction_incomplete"));
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
    assert_eq!(value["code"], "skill_status");
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
    assert_eq!(value["code"], "skill_install");
    let skill_path = skills_dir.join("agent-first-mail/SKILL.md");
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
    assert_eq!(parse_one(&stdout)["code"], "skill_uninstall");
    assert!(!skill_path.exists());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(skills_dir);
}

#[test]
fn reason_mode_controls_required_reason_and_audit_writes() {
    let root = temp_root("reason-mode");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_optional", 1, None);
    write_triage(&root, "message_optional", "optional");

    let (status, stdout) = run(&root, &["message", "trash", "message_optional"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "reason_required");
    assert!(root.join("triage/message_optional.md").exists());
    assert!(!root
        .join(".afmail/messages/message_optional.notes.md")
        .exists());

    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "optional"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "optional");
    let (status, stdout) = run(&root, &["message", "trash", "message_optional"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "message_trashed");
    assert!(!root
        .join(".afmail/messages/message_optional.notes.md")
        .exists());

    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "required"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "required");
    write_message(&root, "message_case", 2, None);
    write_triage(&root, "message_case", "case");
    let (case_uid, case_path, _) =
        create_case(&root, "case-reason", None, None, Some("setup case"));
    let (status, stdout) = run(&root, &["case", "add", &case_uid, "message_case"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "reason_required");
    assert!(root.join("triage/message_case.md").exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_case",
            "--reason",
            "belongs to the case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_message_added");
    let notes = fs::read_to_string(case_path.join("notes.md")).unwrap_or_default();
    assert_eq!(notes, "# Notes\n\n");
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"case_message_added\""));
    assert!(log.contains("\"reason\":\"belongs to the case\""));

    write_message(&root, "message_archive_reason", 3, None);
    write_triage(&root, "message_archive_reason", "archive");
    let (_archive_uid, archive_path, value) = create_archive_message(
        &root,
        "receipts",
        Some("message_archive_reason"),
        Some("receipt notification"),
        Some("receipt notification"),
    );
    assert_eq!(value["code"], "archive_message_created");
    assert!(!root
        .join(".afmail/messages/message_archive_reason.notes.md")
        .exists());
    assert!(archive_path
        .join("views/messages/message_archive_reason.md")
        .exists());
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"archive_message_created\""));
    assert!(log.contains("\"reason\":\"receipt notification\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_imports_imap_mail_with_codec_backed_test_server() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = concat!(
        "Message-ID: <imap-7@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: IMAP pull\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Hello from IMAP.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = start_imap_server(vec![(7, raw)], 6);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "pull_result");
    assert_eq!(value["new_message_count"], 1);
    let progress = read_json(&root.join(".afmail/workspace.progress.json"));
    assert_eq!(progress["schema_name"], "workspace_progress");
    assert_eq!(progress["command"], "pull");
    assert_eq!(progress["status"], "succeeded");
    assert_eq!(progress["result"]["code"], "pull_result");
    assert_eq!(progress["result"]["new_message_count"], 1);
    let id = message_id_for_rfc822(&root, "imap-7@example.com");
    assert!(root.join(format!(".afmail/messages/{id}.eml")).is_file());
    assert!(!root.join(format!(".afmail/messages/{id}.txt")).exists());
    assert!(root.join(format!("messages/{id}.json")).is_file());
    assert!(root.join(format!("triage/{id}.md")).is_file());
    let message_json = fs::read_to_string(root.join(format!("messages/{id}.json")));
    let message_value: Value =
        serde_json::from_str(&message_json.unwrap_or_default()).unwrap_or(Value::Null);
    assert!(message_value["body_text"]
        .as_str()
        .is_some_and(|text| text.contains("Hello from IMAP.")));

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["new_message_count"], 0);
    assert_eq!(value["triage_created_count"], 0);
    assert!(server.handle.join().is_ok());
    let commands = server.commands.lock();
    assert!(commands.is_ok());
    let commands = commands.map(|items| items.clone()).unwrap_or_default();
    assert!(commands.iter().any(|cmd| cmd == "LOGIN"));
    assert!(commands.iter().any(|cmd| cmd == "EXAMINE"));
    assert!(commands.iter().any(|cmd| cmd == "FETCH"));
    assert!(!commands.iter().any(|cmd| cmd == "STORE"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_failure_writes_workspace_progress_snapshot() {
    let root = temp_root("pull-progress-failure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "error");
    let progress = read_json(&root.join(".afmail/workspace.progress.json"));
    assert_eq!(progress["command"], "pull");
    assert_eq!(progress["status"], "failed");
    assert!(
        progress["error"]["error_code"].as_str().is_some(),
        "{progress}"
    );
    assert!(progress["error"]["error"].as_str().is_some(), "{progress}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_progress_logs_real_phases_when_requested() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-progress");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = concat!(
        "Message-ID: <progress-1@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: Progress pull\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Progress body.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = match start_imap_server(vec![(71, raw)], 3) {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["--log", "progress", "pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    assert_eq!(lines.last().unwrap_or(&Value::Null)["code"], "pull_result");
    let phases = lines
        .iter()
        .filter(|line| line["code"] == "log" && line["event"] == "progress")
        .filter_map(|line| line["phase"].as_str())
        .collect::<Vec<_>>();
    for phase in [
        "pull_resolve_targets",
        "pull_mailbox_headers_start",
        "pull_mailbox_headers_done",
        "pull_mailbox_bodies_start",
        "pull_mailbox_bodies_progress",
        "pull_mailbox_bodies_done",
        "pull_reconcile_start",
        "pull_reconcile_done",
        "pull_render_start",
        "pull_render_done",
        "finish",
    ] {
        assert!(phases.contains(&phase), "{stdout}");
    }
    let header_done = lines
        .iter()
        .find(|line| line["phase"] == "pull_mailbox_headers_done")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(header_done["mailbox_id"], "inbox");
    assert_eq!(header_done["mailbox_name"], "INBOX");
    assert_eq!(header_done["index"], 1);
    assert_eq!(header_done["mailbox_count"], 1);
    assert_eq!(header_done["fetched_count"], 1);
    let body_progress = lines
        .iter()
        .find(|line| {
            line["phase"] == "pull_mailbox_bodies_progress" && line["stage"] == "fetch_done"
        })
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(body_progress["level"], "info");
    assert!(body_progress["trace"]["duration_ms"].as_u64().is_some());
    assert_ne!(body_progress["message"], "afmail command finished");
    assert!(
        body_progress["message"]
            .as_str()
            .is_some_and(|message| message.contains("pull_mailbox_bodies_progress")),
        "{stdout}"
    );
    assert_eq!(body_progress["processed_count"], 1);
    assert_eq!(body_progress["uid_count"], 1);
    assert_eq!(body_progress["batch_index"], 1);
    assert_eq!(body_progress["batch_count"], 1);
    let body_done = lines
        .iter()
        .find(|line| line["phase"] == "pull_mailbox_bodies_done")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(body_done["new_message_count"], 1);
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_body_progress_reports_fetch_batches() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-body-progress-batches");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let messages = (1..=26)
        .map(|uid| {
            let raw = format!(
                concat!(
                    "Message-ID: <progress-batch-{}@example.com>\r\n",
                    "From: Alice <alice@example.com>\r\n",
                    "To: Me <me@example.com>\r\n",
                    "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
                    "Subject: Progress batch {}\r\n",
                    "Content-Type: text/plain; charset=utf-8\r\n",
                    "\r\n",
                    "Progress batch body {}.\r\n"
                ),
                uid, uid, uid
            )
            .into_bytes();
            (uid, raw)
        })
        .collect::<Vec<_>>();
    let server = match start_imap_server(messages, 3) {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["--log", "progress", "pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    let done_batches = lines
        .iter()
        .filter(|line| {
            line["phase"] == "pull_mailbox_bodies_progress" && line["stage"] == "fetch_done"
        })
        .collect::<Vec<_>>();
    assert_eq!(done_batches.len(), 2, "{stdout}");
    assert_eq!(done_batches[0]["processed_count"], 25);
    assert_eq!(done_batches[0]["batch_index"], 1);
    assert_eq!(done_batches[0]["batch_count"], 2);
    assert_eq!(done_batches[1]["processed_count"], 26);
    assert_eq!(done_batches[1]["batch_index"], 2);
    assert_eq!(done_batches[1]["batch_count"], 2);
    let body_done = lines
        .iter()
        .find(|line| line["phase"] == "pull_mailbox_bodies_done")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(body_done["processed_count"], 26);
    assert_eq!(body_done["new_message_count"], 26);
    assert!(server.handle.join().is_ok());
    let commands = server.commands.lock();
    assert!(commands.is_ok());
    let fetch_count = commands
        .map(|items| items.iter().filter(|cmd| cmd.as_str() == "FETCH").count())
        .unwrap_or(0);
    // Header fetch + two body batches + reconciliation UID snapshot.
    assert_eq!(fetch_count, 4);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_renders_attachment_listing_in_triage_markdown() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-attachment-md");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = concat!(
        "Message-ID: <attachment-md@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: Contract with attachment\r\n",
        "Content-Type: multipart/mixed; boundary=abc\r\n",
        "\r\n",
        "--abc\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Please review the attachment.\r\n",
        "--abc\r\n",
        "Content-Type: text/plain; name=pricing.txt\r\n",
        "Content-Disposition: attachment; filename=pricing.txt\r\n",
        "\r\n",
        "pricing\r\n",
        "--abc--\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = start_imap_server(vec![(41, raw)], 3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["new_message_count"], 1);
    let id = message_id_for_rfc822(&root, "attachment-md@example.com");
    let triage = fs::read_to_string(root.join(format!("triage/{id}.md"))).unwrap_or_default();
    assert!(triage.contains("attachment_count: 1"));
    assert!(triage.contains("Attachments:"));
    assert!(triage.contains("pricing.txt"));
    assert!(triage.contains("text/plain"));
    assert!(triage.contains("7 bytes"));
    assert!(triage.contains("not fetched"));
    assert!(triage.contains(&format!("`{id}`")));

    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join(format!("messages/{id}.json"))).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["attachments"].as_array().map(|a| a.len()), Some(1));
    let part_id = message["attachments"][0]["part_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!part_id.is_empty());

    let (status, stdout) = run(&root, &["message", "show", &id]);
    assert_eq!(status, 0, "{stdout}");
    let shown = parse_one(&stdout);
    assert_eq!(shown["code"], "message_show");
    assert_eq!(shown["body_text"], "Please review the attachment.");
    assert_eq!(shown["attachment_count"], 1);
    assert_eq!(shown["attachments"][0]["content_type"], "text/plain");

    let (status, stdout) = run(&root, &["message", "attachment", "fetch", &id]);
    assert_eq!(status, 0, "{stdout}");
    let saved_all = parse_one(&stdout);
    assert_eq!(saved_all["code"], "attachments_saved");
    assert_eq!(saved_all["count"], 1);
    assert_eq!(saved_all["items"][0]["storage"], "message_cache");

    let (status, stdout) = run(&root, &["message", "attachment", "fetch", &id, &part_id]);
    assert_eq!(status, 0, "{stdout}");
    let saved = parse_one(&stdout);
    assert_eq!(saved["code"], "attachment_saved");
    assert_eq!(saved["filename"], "pricing.txt");
    assert_eq!(saved["saved_filename"], "pricing.txt");
    assert_eq!(saved["content_type"], "text/plain");
    assert_eq!(saved["storage"], "message_cache");
    assert!(saved["file_path"]
        .as_str()
        .map(|path| root.join(path).is_file())
        .unwrap_or(false));
    assert!(server.handle.join().is_ok());
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
    assert_eq!(value["error_code"], "unknown_mailbox_id");
    assert!(value["error"]
        .as_str()
        .is_some_and(|message| message.contains("available ids:")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_archive_imports_as_triage_without_direct_archive_category() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-archive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = concat!(
        "Message-ID: <archived-17@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: Already handled\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "This mail is already archived.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = start_imap_server(vec![(17, raw)], 4);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );
    let (status, stdout) = run(&root, &["pull", "archive"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "pull_result");
    assert_eq!(value["new_message_count"], 1);
    assert_eq!(value["triage_created_count"], 1);
    assert_eq!(value["archived_message_count"], 0);
    let id = message_id_for_rfc822(&root, "archived-17@example.com");
    assert!(root.join(format!("messages/{id}.json")).is_file());
    assert!(root.join(format!("triage/{id}.md")).exists());
    let data = fs::read_to_string(root.join(format!("messages/{id}.json")));
    assert!(data.is_ok());
    let message: Result<Value, _> = serde_json::from_str(&data.unwrap_or_default());
    assert!(message.is_ok());
    let message = message.unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"].get("archive_uid").is_none());
    assert_eq!(message["remote"]["locations"][0]["mailbox_name"], "Archive");
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_updates_flags_for_existing_remote_location() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-flags");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_existing", 31, None);
    let message_path = root.join("messages/message_existing.json");
    let mut message: Value =
        serde_json::from_str(&fs::read_to_string(&message_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    message["remote"]["locations"][0]["uid_validity"] = json!(44);
    write_json(&message_path, &message);
    write_triage(&root, "message_existing", "already local");
    let raw = concat!(
        "Message-ID: <message_existing@example.com>\r\n",
        "X-Afmail-Test-Flags: \\Seen \\Flagged\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: Existing flag\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "This mail already exists locally.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = start_imap_server(vec![(31, raw)], 3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["new_message_count"], 0);
    assert_eq!(value["flags_updated_count"], 1);
    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_existing.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "triage");
    assert_eq!(
        message["remote"]["locations"][0]["flags"],
        json!(["\\Flagged", "\\Seen"])
    );
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_configured_mailbox_import_rules_apply() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (name, folder_id, folder, uid, status, count_field, direction, triage_view) in [
        (
            "sent",
            "sent",
            "Sent",
            21,
            "triage",
            "triage_created_count",
            "outbound",
            true,
        ),
        (
            "junk",
            "junk",
            "Junk",
            22,
            "spam",
            "spam_message_count",
            "inbound",
            false,
        ),
        (
            "trash",
            "trash",
            "Trash",
            23,
            "trashed",
            "trashed_message_count",
            "inbound",
            false,
        ),
        (
            "archive",
            "archive",
            "Archive",
            24,
            "triage",
            "triage_created_count",
            "inbound",
            true,
        ),
    ] {
        let root = temp_root(&format!("pull-{name}"));
        assert!(fs::create_dir_all(&root).is_ok());
        assert_eq!(run(&root, &["init"]).0, 0);
        let raw = format!(
            concat!(
                "Message-ID: <{}-{}@example.com>\r\n",
                "From: Alice <alice@example.com>\r\n",
                "To: Me <me@example.com>\r\n",
                "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
                "Subject: RFC special use\r\n",
                "Content-Type: text/plain; charset=utf-8\r\n",
                "\r\n",
                "This mail came from a configured mailbox.\r\n"
            ),
            name, uid
        )
        .into_bytes();
        let server = start_imap_server(vec![(uid, raw)], 4);
        assert!(server.is_some());
        let server = match server {
            Some(server) => server,
            None => return,
        };
        write_json(
            &root.join(".afmail/config.json"),
            &test_config(Some(server.addr.port()), None),
        );

        let (status_code, stdout) = run(&root, &["pull", folder_id]);
        assert_eq!(status_code, 0, "{stdout}");
        let value = parse_one(&stdout);
        assert_eq!(value["code"], "pull_result");
        assert_eq!(value["new_message_count"], 1);
        assert_eq!(value[count_field], 1);
        let expected_id = message_id_for_rfc822(&root, &format!("{name}-{uid}@example.com"));
        assert_eq!(
            root.join(format!("triage/{expected_id}.md")).exists(),
            triage_view
        );
        let message_path = root.join(format!("messages/{expected_id}.json"));
        assert!(message_path.is_file());
        let message: Value =
            serde_json::from_str(&fs::read_to_string(message_path).unwrap_or_default())
                .unwrap_or(Value::Null);
        assert_eq!(message["workspace"]["status"], status);
        assert_eq!(message["direction"], direction);
        assert_eq!(message["remote"]["locations"][0]["mailbox_name"], folder);
        assert_eq!(message["remote"]["locations"][0]["mailbox_id"], folder_id);
        if direction == "outbound" {
            assert_ne!(message["sent_rfc3339"], Value::Null);
            assert_eq!(message["received_rfc3339"], Value::Null);
            let (status, stdout) = run(&root, &["triage", "list"]);
            assert_eq!(status, 0, "{stdout}");
            let triage = parse_one(&stdout);
            assert_eq!(triage["count"], 1);
            assert_eq!(triage["items"][0]["message_id"], expected_id);
            assert_eq!(
                triage["path_templates"]["view_path"],
                "triage/{message_id}.md"
            );
            assert_eq!(
                triage["path_templates"]["json_path"],
                "messages/{message_id}.json"
            );
            assert!(triage["items"][0].get("view_path").is_none());
            assert!(triage["items"][0].get("json_path").is_none());
        }
        assert!(server.handle.join().is_ok());
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn pull_defaults_unprocessed_folders_to_triage() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-default-triage");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = concat!(
        "Message-ID: <archived-default@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: Old archived mail\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Filed by a human long ago.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = match start_imap_server(vec![(50, raw)], 4) {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    // Archive imports as triage unless its mailbox config says otherwise.
    let (status, stdout) = run(&root, &["pull", "archive"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["new_message_count"], 1);
    assert_eq!(value["triage_created_count"], 1);
    assert_eq!(value["archived_message_count"], 0);
    let id = message_id_for_rfc822(&root, "archived-default@example.com");
    assert!(root.join(format!("triage/{id}.md")).is_file());
    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join(format!("messages/{id}.json"))).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"].get("archive_uid").is_none());
    // Provenance is preserved in the location, not baked into the status.
    assert_eq!(message["remote"]["locations"][0]["mailbox_name"], "Archive");
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_defaults_junk_to_spam_and_trash_to_trashed() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (folder_id, folder, uid, expected_status) in [
        ("junk", "Junk", 61, "spam"),
        ("trash", "Trash", 62, "trashed"),
    ] {
        let root = temp_root(&format!("pull-default-{folder}"));
        assert!(fs::create_dir_all(&root).is_ok());
        assert_eq!(run(&root, &["init"]).0, 0);
        let raw = format!(
            concat!(
                "Message-ID: <default-{}-{}@example.com>\r\n",
                "From: Alice <alice@example.com>\r\n",
                "To: Me <me@example.com>\r\n",
                "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
                "Subject: x\r\n",
                "Content-Type: text/plain; charset=utf-8\r\n",
                "\r\n",
                "body\r\n"
            ),
            folder, uid
        )
        .into_bytes();
        let server = match start_imap_server(vec![(uid, raw)], 4) {
            Some(server) => server,
            None => return,
        };
        write_json(
            &root.join(".afmail/config.json"),
            &test_config(Some(server.addr.port()), None),
        );

        // Junk/Trash import through the configured mailbox rules.
        let (status, stdout) = run(&root, &["pull", folder_id]);
        assert_eq!(status, 0, "{stdout}");
        let value = parse_one(&stdout);
        assert_eq!(value["new_message_count"], 1);
        assert_eq!(value["triage_created_count"], 0);
        let id = message_id_for_rfc822(&root, &format!("default-{folder}-{uid}@example.com"));
        assert!(!root.join(format!("triage/{id}.md")).exists());
        let message: Value = serde_json::from_str(
            &fs::read_to_string(root.join(format!("messages/{id}.json"))).unwrap_or_default(),
        )
        .unwrap_or(Value::Null);
        assert_eq!(message["workspace"]["status"], expected_status);
        let status_dir = if expected_status == "spam" {
            "spam"
        } else {
            "trash"
        };
        assert!(root.join(format!("{status_dir}/index.md")).is_file());
        assert!(root.join(format!("{status_dir}/{id}.md")).is_file());
        assert!(server.handle.join().is_ok());
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn pull_default_expands_configured_mailbox_ids() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-all");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = concat!(
        "Message-ID: <pull-all-31@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
        "Subject: Pull all\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "This mail appears in every mocked folder.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = start_imap_server(vec![(31, raw)], 8);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "pull_result");
    assert_eq!(value["mailbox_count"], 5);
    assert_eq!(
        value["mailbox_ids"],
        json!(["inbox", "sent", "archive", "junk", "trash"])
    );
    assert_eq!(value["new_message_count"], 1);
    assert_eq!(value["triage_created_count"], 1);
    assert_eq!(value["updated_location_count"], 4);
    let id = message_id_for_rfc822(&root, "pull-all-31@example.com");
    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join(format!("messages/{id}.json"))).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "triage");
    assert_eq!(
        message["remote"]["locations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        5
    );
    assert!(server.handle.join().is_ok());
    let commands = server
        .commands
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();
    let login_count = commands
        .iter()
        .filter(|cmd| cmd.as_str() == "LOGIN")
        .count();
    assert!(
        login_count <= 3,
        "pull should reuse sessions across mailboxes: {commands:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_suggests_existing_case_without_modifying_case() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-suggest");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");
    let (case_uid, case_path, _) = create_case(
        &root,
        "case-one",
        Some("open"),
        Some("message_001"),
        Some("initial case"),
    );
    let case_before = fs::read_to_string(case_path.join("data/case.json"));
    assert!(case_before.is_ok());
    let case_before = case_before.unwrap_or_default();
    let case_messages_before =
        fs::read_to_string(case_path.join("data/messages.json")).unwrap_or_default();
    let raw = concat!(
        "Message-ID: <reply-1@example.com>\r\n",
        "In-Reply-To: <message_001@example.com>\r\n",
        "References: <message_001@example.com>\r\n",
        "From: Alice <alice@example.com>\r\n",
        "To: Me <me@example.com>\r\n",
        "Date: Thu, 21 May 2026 11:00:00 +0000\r\n",
        "Subject: Re: Contract renewal\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Follow-up from IMAP.\r\n"
    )
    .as_bytes()
    .to_vec();
    let server = start_imap_server(vec![(8, raw)], 3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["new_message_count"], 1);
    let id = message_id_for_rfc822(&root, "reply-1@example.com");
    let triage = fs::read_to_string(root.join(format!("triage/{id}.md")));
    assert!(triage.is_ok());
    let triage = triage.unwrap_or_default();
    assert!(triage.contains(&format!("suggested_case_uids:\n  - {case_uid}")));
    assert!(triage.contains("suggested_reason:"));
    assert!(triage.contains(&format!("Suggested case UIDs: `{case_uid}`")));
    let case_after = fs::read_to_string(case_path.join("data/case.json")).unwrap_or_default();
    assert_eq!(case_after, case_before);
    assert_eq!(
        fs::read_to_string(case_path.join("data/messages.json")).unwrap_or_default(),
        case_messages_before
    );
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_folders_lists_mailboxes_and_config_matches() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("remote");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let server = start_imap_server(Vec::new(), 4);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["remote", "test"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "remote_test_result");
    assert_eq!(value["capabilities"]["move"], true);
    let (status, stdout) = run(&root, &["remote", "folders"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "remote_mailboxes");
    assert!(value["mailboxes"].as_array().map(|a| a.len()).unwrap_or(0) >= 3);
    assert!(value["mailboxes"].as_array().is_some_and(|folders| folders
        .iter()
        .any(|folder| folder["mailbox_name"] == "Junk" && folder["special_use"] == "junk")));
    assert!(value["mailboxes"]
        .as_array()
        .is_some_and(|folders| folders
            .iter()
            .any(|folder| folder["mailbox_name"] == "Archive"
                && folder["special_use"] == "archive"
                && folder["special_use_source"] == "rfc6154_attribute"
                && folder["special_use_matches"]
                    .as_array()
                    .is_some_and(|matches| matches
                        .iter()
                        .any(|item| item["kind"] == "archive"
                            && item["source"] == "rfc6154_attribute"))
                && folder["selected_for"]
                    .as_array()
                    .is_some_and(|selected| selected
                        .iter()
                        .any(|item| item["kind"] == "archive" && item["source"] == "mailboxes")))));
    assert!(value["special_use_targets"]
        .as_array()
        .is_some_and(
            |targets| targets.iter().any(|target| target["kind"] == "archive"
                && target["mailbox_name"] == "Archive"
                && target["source"] == "mailboxes"
                && target["exists"] == true)
        ));
    assert!(server.handle.join().is_ok());
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
    assert_eq!(value["code"], "message_spam_marked");
    assert_eq!(value["message_ids"][0], "message_spam");
    assert_eq!(value["location_count"], 1);
    assert_eq!(value["queued"], true);
    assert!(value["push_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(!root.join("triage/message_spam.md").exists());
    assert!(root.join("messages/message_spam.json").exists());
    assert!(!root.join(".afmail/messages/message_spam.txt").exists());
    assert!(root.join(".afmail/messages/message_spam.eml").exists());
    assert!(!root.join("messages/message_junk_44_900.json").exists());
    assert!(root.join("spam/index.md").is_file());
    assert!(root.join("spam/message_spam.md").is_file());

    let spam_data = fs::read_to_string(root.join("messages/message_spam.json"));
    assert!(spam_data.is_ok());
    let spam_value: Result<Value, _> = serde_json::from_str(&spam_data.unwrap_or_default());
    assert!(spam_value.is_ok());
    let spam_value = spam_value.unwrap_or(Value::Null);
    assert_eq!(spam_value["message_id"], "message_spam");
    assert_eq!(spam_value["workspace"]["status"], "spam");
    assert_eq!(
        spam_value["workspace"]["push"]["pending"][0]["kind"],
        "message.spam"
    );
    assert!(spam_value["workspace"]["push"]["pending"][0]["push_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("push_")));
    assert!(!root.join(".afmail/messages/message_spam.notes.md").exists());
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"message_spam_marked\""));
    assert!(log.contains("\"reason\":\"phishing note\""));

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let push = parse_one(&stdout);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["kind"], "message_action");
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
fn message_unspam_and_untrash_restore_triage_and_remove_pending_push() {
    let root = temp_root("unspam-untrash");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));

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
    let spam_push_id = spam["push_id"].as_str().unwrap_or_default().to_string();
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
    assert_eq!(value["code"], "message_unspammed");
    assert_eq!(value["removed_push_count"], 1);
    assert_eq!(value["push_ids"], json!([spam_push_id]));
    assert!(root.join("triage/message_spam_undo.md").exists());
    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_spam_undo.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
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
    let trash_push_id = trash["push_id"].as_str().unwrap_or_default().to_string();
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
    assert_eq!(value["code"], "message_untrashed");
    assert_eq!(value["push_ids"], json!([trash_push_id]));
    assert!(root.join("triage/message_trash_undo.md").exists());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);

    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"message_unspammed\""));
    assert!(log.contains("\"kind\":\"message_untrashed\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_archive_files_without_case_and_queues_archive_mapping() {
    let root = temp_root("triage-archive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "invoice notice");

    let (archive_uid, archive_path, value) = create_archive_message(
        &root,
        "billing",
        Some("message_notice"),
        Some("invoice notice"),
        Some("invoice notice"),
    );
    assert_eq!(value["code"], "archive_message_created");
    assert_eq!(value["archive_uid"], archive_uid);
    assert_eq!(value["eligible_message_ids"], json!(["message_notice"]));
    assert_eq!(value["queued"], true);
    assert!(!root.join("triage/message_notice.md").exists());
    assert!(!root
        .join("cases")
        .read_dir()
        .map(|mut entries| entries.any(|_| true))
        .unwrap_or(false));

    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_notice.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "archived");
    assert_eq!(message["workspace"]["archive_uid"], archive_uid);
    assert!(message["workspace"].get("buckets").is_none());
    assert_eq!(
        message["workspace"]["remote_sync"]["archive_eligible"],
        true
    );
    assert!(archive_path
        .join("views/messages/message_notice.md")
        .exists());
    assert!(fs::read_to_string(archive_path.join("archive.md"))
        .unwrap_or_default()
        .contains("invoice notice"));

    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 1);
    assert_eq!(push["items"][0]["kind"], "message_action");
    assert_eq!(push["items"][0]["action"], "archive");
    assert_eq!(
        push["items"][0]["steps"],
        json!([{"move_to_mailbox_id": "archive"}])
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn message_unarchive_restores_direct_archive_and_removes_pending_push() {
    let root = temp_root("unarchive");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "invoice notice");

    let (archive_uid, archive_path, archived) = create_archive_message(
        &root,
        "billing",
        Some("message_notice"),
        Some("invoice notice"),
        Some("invoice notice"),
    );
    let push_id = archived["push_id"].as_str().unwrap_or_default().to_string();
    assert!(!push_id.is_empty());
    assert!(archive_path
        .join("views/messages/message_notice.md")
        .exists());

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "unarchive",
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "message_unarchived");
    assert_eq!(value["archive_uid"], archive_uid);
    assert_eq!(value["removed_push_count"], 1);
    assert_eq!(value["push_ids"], json!([push_id]));
    assert!(root.join("triage/message_notice.md").exists());
    assert!(!archive_path
        .join("views/messages/message_notice.md")
        .exists());
    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_notice.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"]["archive_uid"].is_null());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archive_message_move_restore_and_log_filters_work() {
    let root = temp_root("archive-message-restore");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_notice", 42, None);
    write_triage(&root, "message_notice", "invoice notice");

    let (billing_uid, billing_path, _value) = create_archive_message(
        &root,
        "billing",
        Some("message_notice"),
        Some("invoice notice"),
        Some("invoice notice"),
    );
    let (receipts_uid, receipts_path, _value) =
        create_archive_message(&root, "receipts", None, None, Some("receipts category"));
    assert!(billing_path
        .join("views/messages/message_notice.md")
        .exists());

    let (status, stdout) = run(&root, &["archive", "message", "show", &billing_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["message_count"], 1);

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "message",
            "move",
            &billing_uid,
            "message_notice",
            &receipts_uid,
            "--reason",
            "receipts owns it",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["archive_uid"], receipts_uid);
    assert!(!billing_path
        .join("views/messages/message_notice.md")
        .exists());
    assert!(receipts_path
        .join("views/messages/message_notice.md")
        .exists());

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "message",
            "restore",
            &billing_uid,
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "archive_entry_not_found");

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "message",
            "set-summary",
            &receipts_uid,
            "message_notice",
            "--summary",
            "receipt summary",
            "--reason",
            "better summary",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["summary"], "receipt summary");

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "message",
            "restore",
            &receipts_uid,
            "message_notice",
            "--reason",
            "needs triage",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "message_restored");
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    assert!(root.join("triage/message_notice.md").exists());
    assert!(!receipts_path
        .join("views/messages/message_notice.md")
        .exists());

    let message: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_notice.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "triage");
    assert!(message["workspace"]["archive_uid"].is_null());
    assert!(message["workspace"].get("buckets").is_none());

    let (status, stdout) = run(&root, &["log", "message", "message_notice"]);
    assert_eq!(status, 0, "{stdout}");
    let log = parse_one(&stdout);
    assert_eq!(log["code"], "log_filtered");
    assert!(log["count"].as_u64().unwrap_or(0) >= 3);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_archive_defers_remote_archive_until_all_case_refs_are_archived() {
    let root = temp_root("case-archive-refs");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["actions"]["case.add"]["steps"] = json!([]);
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_shared", 42, None);
    write_triage(&root, "message_shared", "shared case");
    let (case_one_uid, _case_one_path, _) = create_case(
        &root,
        "case-one",
        Some("open"),
        Some("message_shared"),
        Some("shared case"),
    );
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let case_two_uid = "c20260521002";
    write_case_fixture(&root, "open", case_two_uid, "case-two", &["message_shared"]);

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
    assert_eq!(value["code"], "case_archived");
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
    assert_eq!(push["items"][0]["kind"], "message_action");
    assert_eq!(push["items"][0]["action"], "archive");

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "case",
            "restore",
            &case_one_uid,
            "--group",
            "open",
            "--reason",
            "new reply",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_restored");
    let (status, stdout) = run(&root, &["push", "archive", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["failed_count"], 1);
    assert_eq!(value["failures"][0]["error_code"], "message_referenced");

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
            ]
        )
        .0,
        0
    );

    let server = start_imap_server(Vec::new(), 3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["push", "spam", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_result");
    assert_eq!(value["pushed_count"], 1, "{value}");
    assert_eq!(value["failed_count"], 0);
    let progress = read_json(&root.join(".afmail/workspace.progress.json"));
    assert_eq!(progress["schema_name"], "workspace_progress");
    assert_eq!(progress["command"], "push");
    assert_eq!(progress["status"], "succeeded");
    assert_eq!(progress["result"]["code"], "push_result");
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
    let spam_data = fs::read_to_string(root.join("messages/message_spam.json"));
    assert!(spam_data.is_ok());
    let spam_value: Result<Value, _> = serde_json::from_str(&spam_data.unwrap_or_default());
    assert!(spam_value.is_ok());
    let spam_value = spam_value.unwrap_or(Value::Null);
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
    assert_eq!(spam_value["remote"]["locations"][0]["mailbox_name"], "Junk");
    assert_eq!(spam_value["remote"]["locations"][0]["uid_validity"], 44);
    assert_eq!(spam_value["remote"]["locations"][0]["uid"], 900);
    assert!(spam_value["workspace"]["push"]
        .get("pending")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty));
    assert!(spam_value["workspace"]["push"]["last_completed_rfc3339"]
        .as_str()
        .is_some());
    assert!(!root.join(".afmail/messages/message_spam.notes.md").exists());
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"message_spam_marked\""));
    assert!(log.contains("\"reason\":\"phishing note\""));
    assert!(server.handle.join().is_ok());
    let moved = server.moved.lock();
    assert!(moved.is_ok());
    assert!(moved
        .map(|items| items
            .iter()
            .any(|(source, uid, target)| source == "INBOX" && *uid == 42 && target == "Junk"))
        .unwrap_or(false));
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
fn push_spam_can_skip_marking_remote_seen_by_config() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("spam-push-unread");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["actions"]["message.spam"]["steps"] =
        json!([{"add_flags": ["$Junk"]}, {"move_to_mailbox_id": "junk"}]);
    write_json(&root.join(".afmail/config.json"), &config);
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
            ]
        )
        .0,
        0
    );
    let server = start_imap_server(Vec::new(), 3);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    let mut config = test_config(Some(server.addr.port()), None);
    config["actions"]["message.spam"]["steps"] =
        json!([{"add_flags": ["$Junk"]}, {"move_to_mailbox_id": "junk"}]);
    write_json(&root.join(".afmail/config.json"), &config);

    let (status, stdout) = run(&root, &["push", "spam", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["pushed_count"], 1, "{value}");
    assert!(server.handle.join().is_ok());
    let stored = server.stored.lock();
    assert!(stored.is_ok());
    let stored = stored.map(|items| items.clone()).unwrap_or_default();
    assert!(!stored.iter().any(|(_, _, query)| query.contains("\\Seen")));
    assert!(stored.iter().any(|(_, _, query)| query.contains("$Junk")));
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
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_arch", 43, None);
    write_triage(&root, "message_arch", "done");
    write_message(&root, "message_trash", 44, None);
    write_triage(&root, "message_trash", "delete");

    create_archive_message(
        &root,
        "done",
        Some("message_arch"),
        Some("done"),
        Some("done"),
    );
    assert_eq!(
        run(
            &root,
            &["message", "trash", "message_trash", "--reason", "delete",],
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
            item["kind"] == "message_action"
                && item["action"] == "archive"
                && item["steps"][0]["move_to_mailbox_id"] == "archive"
        })
    }));
    assert!(push["items"].as_array().is_some_and(|items| items
        .iter()
        .any(|item| item["kind"] == "message_action"
            && item["action"] == "trash"
            && item["steps"][0]["move_to_mailbox_id"] == "trash")));

    let server = start_imap_server(Vec::new(), 4);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["push", "archive", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["pushed_count"], 1);
    let (status, stdout) = run(&root, &["push", "trash", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["pushed_count"], 1);

    assert!(server.handle.join().is_ok());
    let moved = server.moved.lock();
    assert!(moved.is_ok());
    let moved = moved.map(|items| items.clone()).unwrap_or_default();

    // Ids are immutable: archive/trash moves keep the original message ids and
    // only update the recorded remote location.
    assert!(root.join("messages/message_arch.json").exists());
    assert!(!root.join("messages/message_archive_44_900.json").exists());
    assert!(root.join("messages/message_trash.json").exists());
    assert!(!root.join("messages/message_trash_44_900.json").exists());
    let arch_data = fs::read_to_string(root.join("messages/message_arch.json"));
    assert!(arch_data
        .as_ref()
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .map(|value| value["remote"]["locations"][0]["mailbox_name"] == "Archive")
        .unwrap_or(false));
    assert!(moved
        .iter()
        .any(|(source, uid, target)| source == "INBOX" && *uid == 43 && target == "Archive"));
    assert!(moved
        .iter()
        .any(|(source, uid, target)| source == "INBOX" && *uid == 44 && target == "Trash"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_group_dirs_are_cleaned_only_when_empty() {
    let root = temp_root("case-group-cleanup");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["actions"]["case.add"]["steps"] = json!([]);
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_alpha", 45, None);
    write_message(&root, "message_beta", 46, None);
    write_triage(&root, "message_alpha", "alpha");
    write_triage(&root, "message_beta", "beta");

    let (case_alpha_uid, _case_alpha_path, _) = create_case(
        &root,
        "case-alpha",
        Some("tests"),
        Some("message_alpha"),
        Some("alpha"),
    );
    let (case_beta_uid, _case_beta_path, _) = create_case(
        &root,
        "case-beta",
        Some("tests"),
        Some("message_beta"),
        Some("beta"),
    );

    assert_eq!(
        run(
            &root,
            &["case", "archive", &case_alpha_uid, "--reason", "done"]
        )
        .0,
        0
    );
    assert!(root.join("cases/tests").exists());
    assert!(root
        .join(format!("cases/tests/{case_beta_uid}-case-beta/case.md"))
        .exists());

    assert_eq!(
        run(&root, &["case", "move", &case_beta_uid, "waiting"]).0,
        0
    );
    assert!(!root.join("cases/tests").exists());
    assert!(root
        .join(format!("cases/waiting/{case_beta_uid}-case-beta/case.md"))
        .exists());

    assert!(fs::write(root.join("cases/waiting/keep.txt"), "user file").is_ok());
    assert_eq!(
        run(
            &root,
            &["case", "archive", &case_beta_uid, "--reason", "done"]
        )
        .0,
        0
    );
    assert!(root.join("cases/waiting/keep.txt").exists());
    assert!(root
        .join(format!("archive/cases/{case_beta_uid}-case-beta/case.md"))
        .exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn removed_flag_and_seen_commands_are_cli_errors() {
    let root = temp_root("message-seen");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_seen", 51, None);
    for action in ["flag", "unflag", "seen", "unseen"] {
        let (status, stdout) = run(&root, &["message", action, "message_seen"]);
        assert_eq!(status, 2, "{stdout}");
        assert_eq!(parse_one(&stdout)["code"], "error");
    }
    let (status, stdout) = run(&root, &["push", "flag"]);
    assert_eq!(status, 2, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "error");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archive_case_notes_rename_restore_and_active_hint_work() {
    let root = temp_root("archive-case-restore");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_case", 42, None);
    write_triage(&root, "message_case", "case work");
    let (case_uid, case_path, _) = create_case(
        &root,
        "case-archive",
        None,
        Some("message_case"),
        Some("case work"),
    );

    assert!(fs::write(case_path.join("case.md"), "stale generated case").is_ok());
    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "case");
    assert_eq!(value["case_uid"], case_uid);
    assert_eq!(value["case_name"], "case-archive");
    assert_eq!(value["group"], "open");
    assert_eq!(
        root.join(value["case_path"].as_str().unwrap_or_default()),
        case_path
    );
    assert_eq!(
        root.join(value["view_path"].as_str().unwrap_or_default()),
        case_path.join("case.md")
    );
    assert!(value.get("index_path").is_none());
    assert_eq!(
        root.join(value["messages_path"].as_str().unwrap_or_default()),
        case_path.join("views/messages")
    );
    assert!(value["text"]
        .as_str()
        .unwrap_or_default()
        .contains("# case-archive"));
    assert_ne!(
        fs::read_to_string(case_path.join("case.md")).unwrap_or_default(),
        "stale generated case"
    );

    let (status, stdout) = run(
        &root,
        &["case", "archive", &case_uid, "--reason", "completed"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_archived");
    assert!(!case_path.exists());
    let archived_path = root.join(format!("archive/cases/{case_uid}-case-archive"));
    assert!(archived_path.exists());

    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "case_archived");
    assert!(value["hint"]
        .as_str()
        .is_some_and(|text| text.contains("afmail archive case")));

    let (status, stdout) = run(&root, &["archive", "case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "archive_case");
    assert_eq!(value["case_uid"], case_uid);
    assert_eq!(value["case_name"], "case-archive");
    assert!(value["text"]
        .as_str()
        .unwrap_or_default()
        .contains("# case-archive"));

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "case_archived");

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "case",
            "notes",
            "append",
            &case_uid,
            "--text",
            "archive note",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let notes = fs::read_to_string(archived_path.join("notes.md")).unwrap_or_default();
    assert!(notes.contains("archive note"));

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "case",
            "rename",
            &case_uid,
            "--name",
            "case-renamed",
            "--reason",
            "better id",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["case_uid"], case_uid);
    assert!(!archived_path.exists());
    let renamed_archive_path = root.join(format!("archive/cases/{case_uid}-case-renamed"));
    assert!(renamed_archive_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "archive",
            "case",
            "restore",
            &case_uid,
            "--group",
            "open",
            "--reason",
            "needs attention",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_restored");
    let restored_path = root.join(format!("cases/open/{case_uid}-case-renamed"));
    assert!(restored_path.join("case.md").exists());
    assert!(!renamed_archive_path.exists());

    let case_data: Value = serde_json::from_str(
        &fs::read_to_string(restored_path.join("data/case.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(case_data["case_uid"], case_uid);
    assert_eq!(case_data["case_name"], "case-renamed");
    assert!(case_data.get("buckets").is_none());
    assert!(case_data.get("primary_bucket").is_none());
    assert!(case_data["archived_rfc3339"].is_null());
    let (status, stdout) = run(&root, &["log", "case", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert!(parse_one(&stdout)["count"].as_u64().unwrap_or(0) >= 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn notes_show_and_archive_refresh_preserve_plain_markdown_bytes() {
    let root = temp_root("notes-read-only");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));

    write_message(&root, "message_case_notes", 52, None);
    write_triage(&root, "message_case_notes", "case notes");
    let (case_uid, case_path, _) = create_case(
        &root,
        "notes-case",
        None,
        Some("message_case_notes"),
        Some("case notes"),
    );
    let active_notes = "---\nlegacy: kept\n---\n\n# User Notes\n\n---\nnot yaml, just markdown\n";
    assert!(fs::write(case_path.join("notes.md"), active_notes).is_ok());

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        active_notes
    );
    assert_eq!(
        fs::read_to_string(case_path.join("notes.md")).unwrap_or_default(),
        active_notes
    );

    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 0, "{stdout}");
    let archived_path = root.join(format!("archive/cases/{case_uid}-notes-case"));
    assert_eq!(
        fs::read_to_string(archived_path.join("notes.md")).unwrap_or_default(),
        active_notes
    );

    let (status, stdout) = run(&root, &["archive", "case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        active_notes
    );
    assert_eq!(
        fs::read_to_string(archived_path.join("notes.md")).unwrap_or_default(),
        active_notes
    );

    write_message(&root, "message_archive_notes", 53, None);
    write_triage(&root, "message_archive_notes", "archive notes");
    let (archive_uid, archive_path, _) = create_archive_message(
        &root,
        "notes-archive",
        Some("message_archive_notes"),
        Some("archive summary"),
        Some("archive notes"),
    );
    let archive_notes = "---\ncategory: kept\n---\n\n# Archive Notes\n";
    assert!(fs::write(archive_path.join("notes.md"), archive_notes).is_ok());

    let (status, stdout) = run(&root, &["archive", "message", "show", &archive_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        fs::read_to_string(archive_path.join("notes.md")).unwrap_or_default(),
        archive_notes
    );

    let (status, stdout) = run(
        &root,
        &["archive", "message", "notes", "show", &archive_uid],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        archive_notes
    );
    assert_eq!(
        fs::read_to_string(archive_path.join("notes.md")).unwrap_or_default(),
        archive_notes
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_notes_show_and_append_error_until_explicit_replace() {
    let root = temp_root("notes-missing");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_missing_notes", 54, None);
    let (case_uid, case_path, _) = create_case(
        &root,
        "missing-notes",
        None,
        Some("message_missing_notes"),
        Some("seed"),
    );
    let notes_path = case_path.join("notes.md");
    assert!(fs::remove_file(&notes_path).is_ok());

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "notes_missing");
    assert!(value["hint"]
        .as_str()
        .unwrap_or_default()
        .contains("notes replace"));
    assert!(!notes_path.exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "notes",
            "append",
            &case_uid,
            "--text",
            "must not create",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["error_code"], "notes_missing");
    assert!(!notes_path.exists());

    let (status, stdout) = run(
        &root,
        &["case", "notes", "replace", &case_uid, "--text", "restored"],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        fs::read_to_string(&notes_path).unwrap_or_default(),
        "restored\n"
    );

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["text"].as_str().unwrap_or_default(),
        "restored\n"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_reply_scaffolds_prefilled_draft() {
    let root = temp_root("case-reply");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_reply", 60, None);
    let (case_uid, case_path, _) = create_case(
        &root,
        "case-reply",
        None,
        Some("message_reply"),
        Some("seed"),
    );

    let (status, stdout) = run(&root, &["case", "reply", &case_uid, "message_reply"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "draft_created");
    assert_eq!(value["draft_name"], "reply-message_reply.md");

    let draft =
        fs::read_to_string(case_path.join("drafts/reply-message_reply.md")).unwrap_or_default();
    assert!(draft.contains("reply_to_message_id: message_reply"));
    assert!(draft.contains("send_intent: reply"));
    assert!(draft.contains("alice@example.com"));
    assert!(draft.contains("Re: Contract renewal"));
    assert!(draft.contains("> Body"));

    // The scaffolded draft must validate against the case.
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "validate",
            &case_uid,
            "reply-message_reply.md",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "draft_valid");

    // A second reply must not clobber the first.
    let (status, _) = run(&root, &["case", "reply", &case_uid, "message_reply"]);
    assert_eq!(status, 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_reply_all_includes_original_recipients_minus_self() {
    let root = temp_root("case-reply-all");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert_eq!(
        run(&root, &["config", "set", "smtp.from", "me@example.com"]).0,
        0
    );
    write_message(&root, "message_ra", 70, None);
    // Give the message multiple recipients, including this account itself.
    let message_path = root.join("messages/message_ra.json");
    let mut message: Value =
        serde_json::from_str(&fs::read_to_string(&message_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    message["from"] = json!("Alice <alice@example.com>");
    message["to"] = json!(["me@example.com", "Bob <bob@example.com>"]);
    message["cc"] = json!(["carol@example.com"]);
    write_json(&message_path, &message);
    let (case_uid, case_path, _) =
        create_case(&root, "case-ra", None, Some("message_ra"), Some("seed"));

    let (status, stdout) = run(&root, &["case", "reply", &case_uid, "message_ra", "--all"]);
    assert_eq!(status, 0, "{stdout}");
    let draft =
        fs::read_to_string(case_path.join("drafts/reply-message_ra.md")).unwrap_or_default();
    // To carries the sender plus the other To recipient; Cc carries the original
    // Cc. This account's own address is excluded from both.
    assert!(draft.contains("alice@example.com"));
    assert!(draft.contains("bob@example.com"));
    assert!(draft.contains("carol@example.com"));
    assert!(!draft.contains("me@example.com"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_reply_prefers_reply_to_header() {
    let root = temp_root("case-reply-to");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_rt", 72, None);
    update_message_json(&root, "message_rt", |message| {
        message["from"] = json!("News Bot <news@example.com>");
        message["reply_to"] = json!(["Support Desk <support@example.net>"]);
    });
    let (case_uid, case_path, _) =
        create_case(&root, "case-rt", None, Some("message_rt"), Some("seed"));

    let (status, stdout) = run(&root, &["case", "reply", &case_uid, "message_rt"]);
    assert_eq!(status, 0, "{stdout}");
    let draft =
        fs::read_to_string(case_path.join("drafts/reply-message_rt.md")).unwrap_or_default();
    let frontmatter = draft.split("\n---\n").next().unwrap_or_default();
    assert!(frontmatter.contains("Support Desk <support@example.net>"));
    assert!(!frontmatter.contains("News Bot <news@example.com>"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_new_and_draft_new_compose_from_scratch() {
    let root = temp_root("compose-new");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (case_uid, case_path, value) = create_case(
        &root,
        "new-thread",
        Some("open"),
        None,
        Some("new outbound case"),
    );
    assert_eq!(value["code"], "case_created");
    assert!(case_path.join("case.md").exists());
    assert!(case_path.join("data/messages.json").exists());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "plumber@example.com",
            "--subject",
            "Leak under the sink",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "draft_created");
    let draft_name = value["draft_name"].as_str().unwrap_or_default().to_string();
    assert!(!draft_name.is_empty());
    assert!(case_path.join(format!("drafts/{draft_name}")).exists());

    // The scaffolded draft validates against the case with no inbound message.
    let (status, stdout) = run(
        &root,
        &["case", "draft", "validate", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "draft_valid");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_attach_copies_external_file_and_updates_frontmatter() {
    let root = temp_root("draft-attach");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path, _) = create_case(
        &root,
        "attach-thread",
        Some("open"),
        None,
        Some("new outbound case"),
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "bob@example.com",
            "--subject",
            "With attachment",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(fs::create_dir_all(root.join("source")).is_ok());
    assert!(fs::write(root.join("source/公司资料?.txt"), "hello").is_ok());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "attach",
            &case_uid,
            &draft_name,
            "source/公司资料?.txt",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "draft_attachment_added");
    assert_eq!(value["copied"], true);
    assert_eq!(value["attachment"], "files/公司资料_.txt");
    assert!(case_path.join("files/公司资料_.txt").is_file());
    let draft =
        fs::read_to_string(case_path.join(format!("drafts/{draft_name}"))).unwrap_or_default();
    assert!(draft.contains("attachments:\n- files/公司资料_.txt"));

    let (status, stdout) = run(
        &root,
        &["case", "draft", "validate", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "attach",
            &case_uid,
            &draft_name,
            "source/公司资料?.txt",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["already_present"], true);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_remove_deletes_draft_state_and_outbound_queue() {
    let root = temp_root("draft-remove");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    let (case_uid, case_path, _) = create_case(
        &root,
        "remove-thread",
        Some("open"),
        None,
        Some("remove draft case"),
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "bob@example.com",
            "--subject",
            "Remove me",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        run(
            &root,
            &["case", "draft", "validate", &case_uid, &draft_name],
        )
        .0,
        0
    );
    assert_eq!(
        run(&root, &["case", "compose", &case_uid, &draft_name]).0,
        0
    );
    let push = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
    let eml_path = push["items"][0]["eml_path"].as_str().unwrap_or_default();
    assert!(!push_id.is_empty());
    assert!(!eml_path.is_empty());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "remove",
            &case_uid,
            &draft_name,
            "--reason",
            "mistaken draft",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let removed = parse_one(&stdout);
    assert_eq!(removed["code"], "draft_removed");
    assert_eq!(removed["queued_removed"], true);
    assert_eq!(removed["mail_sent"], false);
    assert!(!case_path.join(format!("drafts/{draft_name}")).exists());
    assert!(!root
        .join(".afmail/push")
        .join(format!("{push_id}.json"))
        .exists());
    assert!(!root.join(eml_path).exists());
    let state: Value = serde_json::from_str(
        &fs::read_to_string(case_path.join("data/drafts.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert!(state["drafts"].get(draft_name.as_str()).is_none());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_remove_refuses_started_outbound_push() {
    let root = temp_root("draft-remove-started");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    let (case_uid, case_path, _) = create_case(
        &root,
        "sent-thread",
        Some("open"),
        None,
        Some("sent draft case"),
    );
    let draft_path = case_path.join("drafts/new.md");
    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Sent maybe\"\nattachments:\n---\n\nHi Bob\n");
    assert!(fs::write(&draft_path, draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(run(&root, &["case", "compose", &case_uid, "new.md"]).0, 0);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let mut push_item: Value =
        serde_json::from_str(&fs::read_to_string(&push_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    push_item["step_states"] = json!([{
        "index": 0,
        "label": "smtp_send",
        "status": "succeeded",
        "started_rfc3339": "2026-06-09T00:00:00Z",
        "completed_rfc3339": "2026-06-09T00:00:01Z",
        "result_summary": "smtp_send succeeded"
    }]);
    write_json(&push_path, &push_item);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "remove",
            &case_uid,
            "new.md",
            "--reason",
            "mistaken draft",
        ],
    );
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["error_code"], "push_already_started");
    assert!(draft_path.exists());
    assert!(push_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn active_case_generates_localized_message_index_and_views() {
    let root = temp_root("case-messages");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["workspace"]["language_bcp47"] = json!("zh-CN");
    write_json(&root.join(".afmail/config.json"), &config);
    write_message(&root, "message_case1", 81, None);
    write_triage(&root, "message_case1", "seed");
    let (case_uid, case_path, _) = create_case(
        &root,
        "case-messages",
        None,
        Some("message_case1"),
        Some("seed"),
    );

    let index = fs::read_to_string(case_path.join("case.md")).unwrap_or_default();
    assert!(!index.starts_with("---\n"));
    assert!(!index.contains("kind: case_index"));
    assert!(index.starts_with("# case-messages\n"));
    assert!(index.contains(&format!("事项: {case_uid} · 状态: active · 消息: 1")));
    assert!(index.contains("## 1. ← 收到: Contract renewal"));
    assert!(index.contains("- 发件人: alice@example.com"));
    assert!(index.contains("- 消息: [message_case1](views/messages/message_case1.md)"));
    assert!(index.contains("- 时间: 2026-05-21 10:00"));
    assert!(!index.contains("- 状态: case"));
    assert!(!case_path.join("views/messages/index.md").exists());
    let view =
        fs::read_to_string(case_path.join("views/messages/message_case1.md")).unwrap_or_default();
    assert!(view.contains("kind: case_message"));
    assert!(view.contains(&format!("事项: case-messages（`{case_uid}`）")));
    assert!(view.contains("Body"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_index_renders_thread_blocks_with_local_times_and_actions() {
    let root = temp_root("case-thread-index");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["workspace"]["timezone_utc_offset"] = json!("+08:00");
    write_json(&root.join(".afmail/config.json"), &config);

    write_message(&root, "message_sent_reply", 91, None);
    update_message_json(&root, "message_sent_reply", |message| {
        message["direction"] = json!("outbound");
        message["subject"] = json!("Follow up");
        message["from"] = json!("Me <me@example.com>");
        message["to"] = json!(["alice@example.com"]);
        message["received_rfc3339"] = Value::Null;
        message["sent_rfc3339"] = json!("2026-05-21T02:00:00Z");
        message["in_reply_to"] = json!("<message_received@example.com>");
    });
    write_triage(&root, "message_sent_reply", "reply");
    let (case_uid, case_path, _) = create_case(
        &root,
        "thread-case",
        None,
        Some("message_sent_reply"),
        Some("seed"),
    );

    for (message_id, uid, subject, time) in [
        (
            "message_received",
            92,
            "Initial request",
            "2026-05-21T01:00:00Z",
        ),
        (
            "message_re_subject",
            93,
            "Re: Initial request",
            "2026-05-21T03:00:00Z",
        ),
        ("message_fwd", 94, "Fwd: FYI", "2026-05-21T12:00:00+08:00"),
        ("message_fw_sent", 95, "Fw: FYI", "2026-05-21T05:00:00Z"),
    ] {
        write_message(&root, message_id, uid, None);
        update_message_json(&root, message_id, |message| {
            message["subject"] = json!(subject);
            message["received_rfc3339"] = json!(time);
            if message_id == "message_fw_sent" {
                message["direction"] = json!("outbound");
                message["from"] = json!("Me <me@example.com>");
                message["to"] = json!(["alice@example.com"]);
                message["received_rfc3339"] = Value::Null;
                message["sent_rfc3339"] = json!(time);
            }
        });
        write_triage(&root, message_id, "thread");
        let (status, stdout) = run(
            &root,
            &[
                "case",
                "add",
                &case_uid,
                message_id,
                "--reason",
                "belongs to the thread",
            ],
        );
        assert_eq!(status, 0, "{stdout}");
    }

    let index = fs::read_to_string(case_path.join("case.md")).unwrap_or_default();
    let received = index.find("## 1. ← Received: Initial request");
    let sent_reply = index.find("## 2. → Sent reply: Follow up");
    let re_subject = index.find("## 3. ← Received reply: Re: Initial request");
    let fwd = index.find("## 4. ← Received forward: Fwd: FYI");
    let fw_sent = index.find("## 5. → Sent forward: Fw: FYI");
    assert!(received.is_some(), "{index}");
    assert!(sent_reply.is_some(), "{index}");
    assert!(re_subject.is_some(), "{index}");
    assert!(fwd.is_some(), "{index}");
    assert!(fw_sent.is_some(), "{index}");
    assert!(received < sent_reply && sent_reply < re_subject && re_subject < fwd && fwd < fw_sent);
    assert!(index.contains("- From: alice@example.com"));
    assert!(index.contains("- To: alice@example.com"));
    assert!(index.contains("- Time: 2026-05-21 09:00"));
    assert!(index.contains("- Time: 2026-05-21 12:00"));
    assert!(index.contains("- Time: 2026-05-21 13:00"));
    let summary = index
        .split("## Conversation")
        .next()
        .unwrap_or(index.as_str());
    assert!(!summary.contains("2026-05-21T01:00:00Z"));
    assert!(!summary.contains("2026-05-21T12:00:00+08:00"));
    assert!(!summary.contains("- Status: case"));
    assert!(index.contains("## Conversation"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn archive_message_index_renders_thread_blocks_with_summary_fallbacks() {
    let root = temp_root("archive-thread-index");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["workspace"]["timezone_utc_offset"] = json!("+08:00");
    write_json(&root.join(".afmail/config.json"), &config);

    write_message(&root, "message_archive_reply", 96, None);
    update_message_json(&root, "message_archive_reply", |message| {
        message["direction"] = json!("outbound");
        message["subject"] = json!("Original subject");
        message["from"] = json!("Me <me@example.com>");
        message["to"] = json!(["alice@example.com"]);
        message["received_rfc3339"] = Value::Null;
        message["sent_rfc3339"] = json!("2026-05-21T01:00:00Z");
        message["in_reply_to"] = json!("<external@example.com>");
    });
    write_triage(&root, "message_archive_reply", "reply archive");
    let (archive_uid, archive_path, _) = create_archive_message(
        &root,
        "thread-archive",
        Some("message_archive_reply"),
        Some("Invoice summary"),
        Some("archive reply"),
    );

    write_message(&root, "message_archive_forward", 97, None);
    update_message_json(&root, "message_archive_forward", |message| {
        message["subject"] = json!("转发: Fallback subject");
        message["received_rfc3339"] = json!("2026-05-21T15:00:00+08:00");
    });
    write_triage(&root, "message_archive_forward", "forward archive");
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "archive",
            "message_archive_forward",
            &archive_uid,
            "--summary",
            "",
            "--reason",
            "archive forward",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let index = fs::read_to_string(archive_path.join("archive.md")).unwrap_or_default();
    assert!(index.contains("## 1. ← Received forward: 转发: Fallback subject"));
    assert!(index.contains("## 2. → Sent reply: Invoice summary"));
    assert!(!index.contains("→ Sent reply: Original subject"));
    assert!(index.contains("- From: alice@example.com"));
    assert!(index.contains("- To: alice@example.com"));
    assert!(index.contains(
        "- Message: [message_archive_forward](views/messages/message_archive_forward.md)"
    ));
    assert!(index
        .contains("- Message: [message_archive_reply](views/messages/message_archive_reply.md)"));
    assert!(index.contains("- Time: 2026-05-21 15:00"));
    assert!(index.contains("- Time: 2026-05-21 09:00"));
    assert!(!index.contains(" — "));
    assert!(!index.contains("2026-05-21T15:00:00+08:00"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_refresh_rebuilds_generated_views_and_removes_obsolete_case_indexes() {
    let root = temp_root("render-refresh");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));

    write_message(&root, "message_active", 81, None);
    write_triage(&root, "message_active", "active");
    let (active_uid, active_path, _) = create_case(
        &root,
        "active-case",
        None,
        Some("message_active"),
        Some("active"),
    );

    write_message(&root, "message_done", 82, None);
    write_triage(&root, "message_done", "done");
    let (done_uid, _done_path, _) =
        create_case(&root, "done-case", None, Some("message_done"), Some("done"));
    assert_eq!(
        run(&root, &["case", "archive", &done_uid, "--reason", "done"],).0,
        0
    );
    let archived_done_path = root.join(format!("archive/cases/{done_uid}-done-case"));

    write_message(&root, "message_notice", 83, None);
    write_triage(&root, "message_notice", "notice");
    let (_billing_uid, billing_path, _) = create_archive_message(
        &root,
        "billing",
        Some("message_notice"),
        Some("invoice notice"),
        Some("notice"),
    );

    assert!(fs::write(active_path.join("views/messages/index.md"), "legacy active").is_ok());
    assert!(fs::write(
        archived_done_path.join("views/messages/index.md"),
        "legacy archived"
    )
    .is_ok());
    assert!(fs::remove_file(active_path.join("case.md")).is_ok());
    assert!(fs::remove_file(active_path.join("views/messages/message_active.md")).is_ok());
    assert!(fs::remove_file(archived_done_path.join("case.md")).is_ok());
    assert!(fs::remove_file(archived_done_path.join("views/messages/message_done.md")).is_ok());
    assert!(fs::remove_file(billing_path.join("archive.md")).is_ok());
    assert!(fs::remove_file(billing_path.join("views/messages/message_notice.md")).is_ok());
    let before_push = parse_one(&run(&root, &["push", "list"]).1)["count"].clone();

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "render_refreshed");
    assert_eq!(value["generated"]["case/case.md.j2"], 2);
    assert_eq!(value["generated"]["case/message.md.j2"], 2);
    assert_eq!(value["generated"]["archive-message/archive.md.j2"], 1);
    assert_eq!(value["generated"]["archive-message/message.md.j2"], 1);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["builtin"], 2);
    assert_eq!(
        parse_one(&run(&root, &["push", "list"]).1)["count"],
        before_push
    );

    let active_index = fs::read_to_string(active_path.join("case.md")).unwrap_or_default();
    assert!(!active_index.starts_with("---\n"));
    assert!(!active_index.contains("kind: case_index"));
    assert!(active_index.starts_with("# active-case\n"));
    assert!(active_index.contains(&format!(
        "Case: {active_uid} · Status: active · Messages: 1"
    )));
    assert!(active_index.contains("## 1. ← Received: Contract renewal"));
    assert!(active_index.contains("- From: alice@example.com"));
    assert!(active_index.contains("- Message: [message_active](views/messages/message_active.md)"));
    assert!(active_index.contains("- Time: 2026-05-21 10:00"));
    assert!(!active_index.contains("- Status: case"));
    assert!(!active_index.contains("| Message |"));
    assert!(active_path
        .join("views/messages/message_active.md")
        .exists());
    assert!(archived_done_path.join("case.md").exists());
    assert!(archived_done_path
        .join("views/messages/message_done.md")
        .exists());
    assert!(billing_path.join("archive.md").exists());
    assert!(billing_path
        .join("views/messages/message_notice.md")
        .exists());
    assert!(!active_path.join("views/messages/index.md").exists());
    assert!(!archived_done_path.join("views/messages/index.md").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_refresh_uses_user_templates_per_generated_file_type() {
    let root = temp_root("render-template");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));

    write_message(&root, "message_case", 84, None);
    write_triage(&root, "message_case", "case");
    let (case_uid, case_path, _) = create_case(
        &root,
        "template-case",
        None,
        Some("message_case"),
        Some("case"),
    );
    write_message(&root, "message_archive", 85, None);
    write_triage(&root, "message_archive", "archive");
    let (_archive_uid, archive_path, _) = create_archive_message(
        &root,
        "billing",
        Some("message_archive"),
        Some("billing summary"),
        Some("archive"),
    );

    assert!(fs::create_dir_all(root.join(".afmail/templates/case")).is_ok());
    assert!(fs::write(
        root.join(".afmail/templates/case/case.md.j2"),
        "IGNORED GENERIC {{ case_uid }}\n"
    )
    .is_ok());
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["workspace"], 0);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["builtin"], 1);
    assert!(!fs::read_to_string(case_path.join("case.md"))
        .unwrap_or_default()
        .contains("IGNORED GENERIC"));

    assert!(fs::create_dir_all(root.join(".afmail/templates/en-US/case")).is_ok());
    assert!(fs::write(
        root.join(".afmail/templates/en-US/case/case.md.j2"),
        "USER CASE {{ case_uid }} {{ message_count }}\n"
    )
    .is_ok());
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["workspace"], 1);
    assert_eq!(
        fs::read_to_string(case_path.join("case.md")).unwrap_or_default(),
        format!("USER CASE {case_uid} 1")
    );
    assert!(
        fs::read_to_string(case_path.join("views/messages/message_case.md"))
            .unwrap_or_default()
            .contains("kind: case_message")
    );
    assert!(!fs::read_to_string(archive_path.join("archive.md"))
        .unwrap_or_default()
        .contains("USER CASE"));

    assert!(fs::write(
        root.join(".afmail/templates/en-US/case/case.md.j2"),
        "{% for item in items %}"
    )
    .is_ok());
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["error_code"], "template_render_failed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_templates_exports_lists_and_force_overwrites_defaults() {
    let root = temp_root("render-templates");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(&root, &["render", "templates"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "render_templates");
    assert_eq!(value["template_dir_created"], true);
    assert_eq!(value["exported_count"], 28);
    assert_eq!(value["workspace_count"], 28);
    assert!(root
        .join(".afmail/templates/en-US/case/case.md.j2")
        .is_file());
    assert!(root
        .join(".afmail/templates/zh-CN/archive-message/message.md.j2")
        .is_file());

    assert!(fs::write(
        root.join(".afmail/templates/en-US/case/case.md.j2"),
        "CUSTOM {{ case_uid }}\n"
    )
    .is_ok());
    let (status, stdout) = run(&root, &["render", "templates"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["kept_count"], 28);
    assert_eq!(
        fs::read_to_string(root.join(".afmail/templates/en-US/case/case.md.j2"))
            .unwrap_or_default(),
        "CUSTOM {{ case_uid }}\n"
    );

    let other = temp_root("render-templates-list");
    assert!(fs::create_dir_all(&other).is_ok());
    assert_eq!(run(&other, &["init"]).0, 0);
    assert!(fs::create_dir_all(other.join(".afmail/templates/case")).is_ok());
    assert!(fs::write(other.join(".afmail/templates/case/case.md.j2"), "USER\n").is_ok());
    let (status, stdout) = run(&other, &["render", "templates"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["exported_count"], 28);
    assert_eq!(value["workspace_count"], 28);
    assert_eq!(value["builtin_count"], 0);
    assert_eq!(value["items"][0]["source"], "workspace");
    assert_eq!(value["items"][0]["language"], "en-US");

    let (status, stdout) = run(&root, &["render", "templates", "--force"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["overwritten_count"], 28);
    assert!(
        fs::read_to_string(root.join(".afmail/templates/en-US/case/case.md.j2"))
            .unwrap_or_default()
            .contains(
                "Case: {{ case_uid }} · Status: {{ status }} · Messages: {{ message_count }}"
            )
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(other);
}

#[test]
fn push_defaults_to_preview_and_rejects_dry_run_confirm() {
    let root = temp_root("push-preview");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
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
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_dry_run");
    assert_eq!(value["confirmed"], false);
    assert_eq!(value["count"], 1);

    let (status, stdout) = run(&root, &["push", "spam"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_dry_run");
    assert_eq!(value["confirmed"], false);
    assert_eq!(value["count"], 1);
    assert!(value["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("No remote changes were made")));
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 1);

    let (status, stdout) = run(&root, &["push", "spam", "--dry-run", "--confirm"]);
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["error_code"], "invalid_request");
    let (status, stdout) = run(&root, &["push", "--dry-run", "--confirm"]);
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["error_code"], "invalid_request");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_validation_hash_blocks_stale_compose_and_push() {
    let root = temp_root("draft-freshness");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    let (case_uid, case_path, _) = create_case(
        &root,
        "fresh-thread",
        Some("open"),
        None,
        Some("fresh draft case"),
    );
    let draft_path = case_path.join("drafts/new.md");
    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Hello\"\nattachments:\n---\n\nHi Bob\n");
    assert!(fs::write(&draft_path, &draft).is_ok());

    let (status, stdout) = run(&root, &["case", "compose", &case_uid, "new.md"]);
    assert_eq!(status, 1);
    assert_eq!(
        parse_one(&stdout)["error_code"],
        "draft_validation_required"
    );

    let (status, stdout) = run(&root, &["case", "draft", "validate", &case_uid, "new.md"]);
    assert_eq!(status, 0, "{stdout}");
    let validated = parse_one(&stdout);
    assert_eq!(validated["code"], "draft_valid");
    let draft_hash = validated["draft_hash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(draft_hash.starts_with("sha256:"));
    let state_path = case_path.join("data/drafts.json");
    assert!(state_path.is_file());
    let state: Value = serde_json::from_str(&fs::read_to_string(&state_path).unwrap_or_default())
        .unwrap_or(Value::Null);
    assert_eq!(state["drafts"]["new.md"]["last_validated_hash"], draft_hash);

    assert_eq!(run(&root, &["case", "move", &case_uid, "waiting"]).0, 0);
    let moved_path = root.join(format!("cases/waiting/{case_uid}-fresh-thread"));
    let moved_state_path = moved_path.join("data/drafts.json");
    assert!(moved_state_path.is_file());
    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["error_code"], "case_has_local_drafts");
    assert!(moved_path.join("data/drafts.json").is_file());
    let draft_path = moved_path.join("drafts/new.md");
    assert!(fs::write(&draft_path, draft.replace("Hi Bob", "Hi Bob\nP.S. changed")).is_ok());
    let (status, stdout) = run(&root, &["case", "compose", &case_uid, "new.md"]);
    assert_eq!(status, 1);
    assert_eq!(
        parse_one(&stdout)["error_code"],
        "draft_changed_since_validation"
    );

    let (status, stdout) = run(&root, &["case", "draft", "validate", &case_uid, "new.md"]);
    assert_eq!(status, 0, "{stdout}");
    let new_hash = parse_one(&stdout)["draft_hash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(new_hash, draft_hash);

    let (status, stdout) = run(&root, &["case", "compose", &case_uid, "new.md"]);
    assert_eq!(status, 0, "{stdout}");
    let queued = parse_one(&stdout);
    assert_eq!(queued["code"], "push_queued");
    assert_eq!(queued["draft_hash"], new_hash);
    let push_id = queued["push_id"].as_str().unwrap_or_default().to_string();
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["items"][0]["draft_hash"], new_hash);

    let (status, stdout) = run(&root, &["push", "drafts-send"]);
    assert_eq!(status, 0, "{stdout}");
    let preview = parse_one(&stdout);
    assert_eq!(preview["code"], "push_dry_run");
    assert_eq!(preview["confirmed"], false);
    assert!(preview["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("No mail was sent")));

    assert!(fs::write(&draft_path, draft.replace("Hi Bob", "Hi again")).is_ok());
    let (status, stdout) = run(&root, &["push", "drafts", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let pushed = parse_one(&stdout);
    assert_eq!(pushed["code"], "push_result");
    assert_eq!(pushed["confirmed"], true);
    assert_eq!(pushed["pushed_count"], 0);
    assert_eq!(pushed["failed_count"], 1);
    assert_eq!(
        pushed["failures"][0]["error_code"],
        "draft_changed_since_compose"
    );

    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"],).0,
        0
    );
    let (status, stdout) = run(&root, &["case", "compose", &case_uid, "new.md"]);
    assert_eq!(status, 0, "{stdout}");
    let requeried = parse_one(&stdout);
    assert_eq!(requeried["push_id"], push_id);
    let outbound_count = parse_one(&run(&root, &["push", "list"]).1)["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["kind"] == "outbound")
                .count()
        })
        .unwrap_or(0);
    assert_eq!(outbound_count, 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_list_reports_untriaged_messages() {
    let root = temp_root("triage-list");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_t1", 51, None);
    write_triage(&root, "message_t1", "note");
    write_message(&root, "message_t2", 52, None);
    write_triage(&root, "message_t2", "note");

    let (status, stdout) = run(&root, &["triage", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "triage_list");
    assert_eq!(value["count"], 2);
    assert_eq!(
        value["path_templates"]["view_path"],
        "triage/{message_id}.md"
    );
    assert_eq!(
        value["path_templates"]["json_path"],
        "messages/{message_id}.json"
    );
    let items = value["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| item["message_id"] == "message_t1"));
    assert!(items.iter().any(|item| item["message_id"] == "message_t2"));
    for old_field in [
        "from",
        "subject",
        "attachment_count",
        "mailbox_ids",
        "flags",
        "unread",
        "flagged",
        "remote_missing",
        "remote_effect_pending",
        "push",
        "suggested_case_uids",
        "related_message_ids",
        "requires_case",
        "view_path",
        "json_path",
    ] {
        assert!(items.iter().all(|item| item.get(old_field).is_none()));
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn list_commands_return_locators_and_status_includes_counts_and_progress() {
    let root = temp_root("locator-lists");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));

    write_message(&root, "message_case_list", 53, None);
    write_triage(&root, "message_case_list", "case seed");
    let (case_uid, _case_path, _) = create_case(
        &root,
        "active-locator",
        Some("waiting"),
        Some("message_case_list"),
        Some("track this work"),
    );

    let (status, stdout) = run(&root, &["case", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "case_list");
    assert_eq!(value["count"], 1);
    let item = value["items"][0].clone();
    assert_eq!(item["case_uid"], case_uid);
    assert_eq!(item["case_name"], "active-locator");
    assert_eq!(item["group"], "waiting");
    assert!(item["case_dir"]
        .as_str()
        .is_some_and(|path| path.starts_with(&case_uid)));
    assert_eq!(
        value["path_templates"]["view_path"],
        "cases/{group}/{case_dir}/case.md"
    );
    assert_eq!(
        value["path_templates"]["data_path"],
        "cases/{group}/{case_dir}/data/case.json"
    );
    assert!(item.get("view_path").is_none());
    assert!(item.get("data_path").is_none());
    assert!(item.get("path").is_none());
    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case");

    write_message(&root, "message_archive_list", 54, None);
    write_triage(&root, "message_archive_list", "archive seed");
    let (archive_uid, _archive_path, _) = create_archive_message(
        &root,
        "archive-locator",
        Some("message_archive_list"),
        Some("reference notification"),
        Some("file it"),
    );
    let (status, stdout) = run(&root, &["archive", "list", "messages"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "archive_message_list");
    assert_eq!(value["count"], 1);
    let item = value["items"][0].clone();
    assert_eq!(item["archive_uid"], archive_uid);
    assert_eq!(item["archive_name"], "archive-locator");
    assert!(item["archive_dir"]
        .as_str()
        .is_some_and(|path| path.starts_with(&archive_uid)));
    assert_eq!(
        value["path_templates"]["view_path"],
        "archive/notifications/{archive_dir}/archive.md"
    );
    assert_eq!(
        value["path_templates"]["data_path"],
        "archive/notifications/{archive_dir}/data/archive.json"
    );
    assert!(item.get("view_path").is_none());
    assert!(item.get("data_path").is_none());
    assert!(item.get("path").is_none());
    assert!(item.get("message_count").is_none());

    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(&root, &["archive", "list", "cases"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "archive_case_list");
    assert_eq!(value["count"], 1);
    let item = value["items"][0].clone();
    assert_eq!(item["case_uid"], case_uid);
    assert_eq!(item["case_name"], "active-locator");
    assert!(item["case_dir"]
        .as_str()
        .is_some_and(|path| path.starts_with(&case_uid)));
    assert_eq!(
        value["path_templates"]["view_path"],
        "archive/cases/{case_dir}/case.md"
    );
    assert_eq!(
        value["path_templates"]["data_path"],
        "archive/cases/{case_dir}/data/case.json"
    );
    assert!(item.get("view_path").is_none());
    assert!(item.get("data_path").is_none());
    assert!(item.get("path").is_none());

    let (status, stdout) = run(&root, &["archive", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "archive_list");
    assert_eq!(value["case_count"], 1);
    assert_eq!(value["message_count"], 1);
    assert_eq!(value["cases"][0]["case_uid"], case_uid);
    assert_eq!(value["messages"][0]["archive_uid"], archive_uid);
    assert_eq!(
        value["case_path_templates"]["view_path"],
        "archive/cases/{case_dir}/case.md"
    );
    assert_eq!(
        value["message_path_templates"]["view_path"],
        "archive/notifications/{archive_dir}/archive.md"
    );

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["case_count"], 0);
    assert_eq!(value["archived_case_count"], 1);
    assert_eq!(value["archive_message_category_count"], 1);
    assert_eq!(value["progress"]["status"], "idle");
    assert!(value.get("cases").is_none());
    assert!(value.get("archived_cases").is_none());
    assert!(value.get("archive_message_categories").is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn header_related_messages_require_case_for_direct_disposition() {
    let root = temp_root("related-requires-case");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_health_parent", 61, None);
    write_message(&root, "message_health_reply", 62, None);
    update_message_json(&root, "message_health_parent", |value| {
        value["subject"] = json!("不能拒绝健康");
        value["from"] = json!("support@example.com");
    });
    update_message_json(&root, "message_health_reply", |value| {
        value["direction"] = json!("outbound");
        value["subject"] = json!("Re: 不能拒绝健康");
        value["from"] = json!("me@example.com");
        value["received_rfc3339"] = Value::Null;
        value["sent_rfc3339"] = json!("2026-05-21T10:05:00Z");
        value["in_reply_to"] = json!("<message_health_parent@example.com>");
        value["references"] = json!(["<message_health_parent@example.com>"]);
    });
    write_triage(&root, "message_health_parent", "parent");
    write_triage(&root, "message_health_reply", "reply");

    let (status, stdout) = run(&root, &["triage", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let items = value["items"].as_array().cloned().unwrap_or_default();
    let parent = items
        .iter()
        .find(|item| item["message_id"] == "message_health_parent")
        .cloned()
        .unwrap_or(Value::Null);
    let reply = items
        .iter()
        .find(|item| item["message_id"] == "message_health_reply")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        value["path_templates"]["view_path"],
        "triage/{message_id}.md"
    );
    assert_eq!(
        value["path_templates"]["json_path"],
        "messages/{message_id}.json"
    );
    assert_eq!(parent["message_id"], "message_health_parent");
    assert_eq!(reply["message_id"], "message_health_reply");
    assert!(parent.get("view_path").is_none());
    assert!(parent.get("json_path").is_none());
    assert!(parent.get("requires_case").is_none());
    assert!(parent.get("related_message_ids").is_none());

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "spam",
            "message_health_parent",
            "--reason",
            "looks automated",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(
        value["error_code"],
        "message_has_related_conversation_use_case"
    );
    assert_eq!(
        value["details"]["related_message_ids"],
        json!(["message_health_reply"])
    );
    assert!(value["details"]["suggested_commands"]
        .as_array()
        .is_some_and(|commands| commands.iter().any(|command| command
            .as_str()
            .unwrap_or_default()
            .contains("afmail case add CASE_REF message_health_reply"))));

    let (done_archive_uid, _done_archive_path, _) =
        create_archive_message(&root, "done", None, None, Some("done archive"));
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "archive",
            "message_health_reply",
            &done_archive_uid,
            "--summary",
            "reply done",
            "--reason",
            "done",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(
        value["error_code"],
        "message_has_related_conversation_use_case"
    );
    assert_eq!(
        value["details"]["related_message_ids"],
        json!(["message_health_parent"])
    );
    assert!(value["details"]["suggested_commands"]
        .as_array()
        .is_some_and(|commands| commands.iter().any(|command| command
            .as_str()
            .unwrap_or_default()
            .contains("afmail case add CASE_REF message_health_parent"))));

    let (case_uid, _case_path, _value) = create_case(
        &root,
        "health-conversation",
        None,
        Some("message_health_parent"),
        Some("conversation needs case"),
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_health_reply",
            "--reason",
            "reply belongs to same case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(
        value["related_message_ids"],
        json!(["message_health_parent"])
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
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "case_archived");
    assert_eq!(
        value["case_path"],
        format!("archive/cases/{case_uid}-health-conversation")
    );
    assert!(root
        .join(format!(
            "archive/cases/{case_uid}-health-conversation/case.md"
        ))
        .exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn case_merge_preserves_other_case_notes() {
    let root = temp_root("case-merge-notes");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_a", 70, None);
    write_message(&root, "message_b", 71, None);
    let (case_a_uid, case_a_path, _) =
        create_case(&root, "case-a", None, Some("message_a"), Some("a"));
    let (case_b_uid, case_b_path, _) =
        create_case(&root, "case-b", None, Some("message_b"), Some("b"));
    let merged_notes =
        "---\nlegacy: keep-as-markdown\n---\n\n# Merged Context\n\nmust-keep-context\n";
    assert!(fs::write(case_b_path.join("notes.md"), merged_notes).is_ok());

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "merge",
            &case_a_uid,
            &case_b_uid,
            "--reason",
            "same underlying issue",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let notes = fs::read_to_string(case_a_path.join("notes.md")).unwrap_or_default();
    assert!(notes.contains(&format!("## Merged from {case_b_uid}")));
    let section = notes
        .split(&format!("## Merged from {case_b_uid}\n\n"))
        .nth(1)
        .unwrap_or_default();
    assert!(section.starts_with(merged_notes), "{notes}");
    assert!(!case_b_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn handoff_subcommands_are_removed() {
    let root = temp_root("handoff-gone");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    // External ref parsing rejects this former case subcommand at runtime.
    assert_eq!(run(&root, &["push", "handoff"]).0, 2);
    assert_eq!(run(&root, &["case", "case-x", "handoff"]).0, 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_refreshes_disposition_generated_views() {
    let root = temp_root("render-disposition-views");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam", 40, None);
    write_message(&root, "message_trash", 41, None);
    write_message(&root, "message_deleted", 42, None);
    update_message_json(&root, "message_spam", |message| {
        message["workspace"]["status"] = json!("spam");
    });
    update_message_json(&root, "message_trash", |message| {
        message["workspace"]["status"] = json!("trashed");
    });
    update_message_json(&root, "message_deleted", |message| {
        message["workspace"]["status"] = json!("deleted_remote");
    });
    assert!(fs::create_dir_all(root.join("spam")).is_ok());
    assert!(fs::create_dir_all(root.join("trash")).is_ok());
    assert!(fs::create_dir_all(root.join("deleted")).is_ok());
    assert!(fs::write(root.join("spam/message_stale.md"), "stale").is_ok());
    assert!(fs::write(root.join("trash/message_stale.md"), "stale").is_ok());
    assert!(fs::write(root.join("deleted/message_stale.md"), "stale").is_ok());

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let refreshed = parse_one(&stdout);
    assert_eq!(refreshed["spam_count"], 1);
    assert_eq!(refreshed["trash_count"], 1);
    assert_eq!(refreshed["deleted_count"], 1);
    assert_eq!(refreshed["stale_spam_removed_count"], 1);
    assert_eq!(refreshed["stale_trash_removed_count"], 1);
    assert_eq!(refreshed["stale_deleted_removed_count"], 1);
    assert!(root.join("spam/index.md").is_file());
    assert!(root.join("spam/message_spam.md").is_file());
    assert!(!root.join("spam/message_stale.md").exists());
    assert!(root.join("trash/index.md").is_file());
    assert!(root.join("trash/message_trash.md").is_file());
    assert!(!root.join("trash/message_stale.md").exists());
    assert!(root.join("deleted/index.md").is_file());
    assert!(root.join("deleted/message_deleted.md").is_file());
    assert!(!root.join("deleted/message_stale.md").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn purge_deletes_old_local_discard_records() {
    let root = temp_root("purge-dispositions");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    for (message_id, uid, status, updated) in [
        ("message_spam_old", 50, "spam", "2000-01-01T00:00:00Z"),
        ("message_spam_recent", 51, "spam", "2999-01-01T00:00:00Z"),
        ("message_trash_old", 52, "trashed", "2000-01-01T00:00:00Z"),
        (
            "message_deleted_old",
            53,
            "deleted_remote",
            "2000-01-01T00:00:00Z",
        ),
        ("message_triage_old", 54, "triage", "2000-01-01T00:00:00Z"),
    ] {
        write_message(&root, message_id, uid, None);
        update_message_json(&root, message_id, |message| {
            message["workspace"]["status"] = json!(status);
        });
        set_message_state_updated(&root, message_id, updated);
    }
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(root.join("spam/message_spam_old.md").is_file());
    assert!(root.join("trash/message_trash_old.md").is_file());
    assert!(root.join("deleted/message_deleted_old.md").is_file());

    let (status, stdout) = run(&root, &["purge", "spam"]);
    assert_eq!(status, 0, "{stdout}");
    let purged = parse_one(&stdout);
    assert_eq!(purged["code"], "purged");
    assert_eq!(purged["target"], "spam");
    assert_eq!(purged["older_than_days"], 30);
    assert_eq!(purged["purged_message_ids"], json!(["message_spam_old"]));
    assert_eq!(purged["purged_spam_count"], 1);
    assert_eq!(purged["purged_trash_count"], 0);
    assert_eq!(purged["purged_deleted_count"], 0);
    assert_eq!(
        purged["skipped_recent_message_ids"],
        json!(["message_spam_recent"])
    );
    assert!(!root.join("messages/message_spam_old.json").exists());
    assert!(!root.join(".afmail/messages/message_spam_old.eml").exists());
    assert!(!root
        .join(".afmail/messages/message_spam_old.state.json")
        .exists());
    assert!(!root
        .join(".afmail/messages/message_spam_old.remote.json")
        .exists());
    assert!(!root.join("spam/message_spam_old.md").exists());
    assert!(root.join("messages/message_spam_recent.json").is_file());
    assert!(root.join("messages/message_trash_old.json").is_file());
    assert!(root.join("messages/message_deleted_old.json").is_file());
    assert!(root.join("trash/message_trash_old.md").is_file());
    assert!(root.join("deleted/message_deleted_old.md").is_file());

    let (status, stdout) = run(&root, &["purge"]);
    assert_eq!(status, 0, "{stdout}");
    let purged = parse_one(&stdout);
    assert_eq!(purged["target"], "discards");
    assert_eq!(purged["targets"], json!(["spam", "trash", "deleted"]));
    assert_eq!(
        purged["purged_message_ids"],
        json!(["message_deleted_old", "message_trash_old"])
    );
    assert_eq!(purged["purged_spam_count"], 0);
    assert_eq!(purged["purged_trash_count"], 1);
    assert_eq!(purged["purged_deleted_count"], 1);
    assert_eq!(
        purged["skipped_recent_message_ids"],
        json!(["message_spam_recent"])
    );
    assert!(!root.join("messages/message_trash_old.json").exists());
    assert!(!root.join("messages/message_deleted_old.json").exists());
    assert!(!root.join("trash/message_trash_old.md").exists());
    assert!(!root.join("deleted/message_deleted_old.md").exists());
    assert!(root.join("messages/message_spam_recent.json").is_file());
    assert!(root.join("messages/message_triage_old.json").is_file());
    assert!(root.join("spam/index.md").is_file());
    assert!(root.join("trash/index.md").is_file());
    assert!(root.join("deleted/index.md").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_marks_unreferenced_missing_remote_message_deleted_remote() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("sync-delete");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_missing", 42, None);
    update_message_json(&root, "message_missing", |message| {
        message["workspace"]["status"] = json!("spam");
    });
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(root.join("spam/message_missing.md").exists());
    let server = start_imap_server(Vec::new(), 2);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "pull_result");
    assert_eq!(value["new_message_count"], 0);
    assert_eq!(value["triage_created_count"], 0);
    assert_eq!(value["checked_location_count"], 1);
    assert_eq!(value["missing_location_count"], 1);
    assert_eq!(value["deleted_remote_message_count"], 1);
    assert_eq!(
        value["deleted_remote_message_ids"],
        json!(["message_missing"])
    );
    assert_eq!(value["tombstoned_message_count"], 0);
    assert_eq!(value["spam_count"], 0);
    assert_eq!(value["deleted_count"], 1);
    assert!(server.handle.join().is_ok());

    // Files remain in local storage with deleted_remote state until purge.
    assert!(root.join("messages/message_missing.json").exists());
    let data = fs::read_to_string(root.join("messages/message_missing.json")).unwrap_or_default();
    let msg: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
    assert_eq!(msg["workspace"]["status"], "deleted_remote");
    assert_eq!(
        msg["remote"]["locations"][0]["missing_rfc3339"]
            .as_str()
            .map(|v| v.is_empty()),
        Some(false)
    );
    let show = parse_one(&run(&root, &["message", "show", "message_missing"]).1);
    assert_eq!(show["remote_missing"], true);
    assert!(show["remote_missing_since_rfc3339"].as_str().is_some());
    assert_eq!(show["view_path"], "deleted/message_missing.md");
    assert!(!root.join("spam/message_missing.md").exists());
    assert!(root.join("deleted/message_missing.md").exists());
    assert!(!root.join(".afmail/deleted").exists());

    let (status, stdout) = run(&root, &["purge", "deleted", "--older-than-days", "0"]);
    assert_eq!(status, 0, "{stdout}");
    let purged = parse_one(&stdout);
    assert_eq!(purged["target"], "deleted");
    assert_eq!(purged["purged_message_ids"], json!(["message_missing"]));
    assert_eq!(purged["purged_deleted_count"], 1);
    assert!(!root.join("messages/message_missing.json").exists());
    assert!(!root.join("deleted/message_missing.md").exists());
    assert!(!root.join(".afmail/messages/message_missing.eml").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_tombstones_referenced_missing_remote_message_in_place() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("sync-tombstone");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_missing", 42, None);
    write_triage(&root, "message_missing", "still needs review");
    create_case(
        &root,
        "case-ref",
        None,
        Some("message_missing"),
        Some("keep referenced message"),
    );
    let server = start_imap_server(Vec::new(), 2);
    assert!(server.is_some());
    let server = match server {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "pull_result");
    assert_eq!(value["new_message_count"], 0);
    assert_eq!(value["triage_created_count"], 0);
    assert_eq!(value["deleted_remote_message_count"], 0);
    assert_eq!(value["tombstoned_message_count"], 1);
    assert_eq!(value["tombstoned_message_ids"][0], "message_missing");
    assert!(server.handle.join().is_ok());

    assert!(root.join("messages/message_missing.json").exists());
    assert!(!root.join(".afmail/deleted").exists());
    let data = fs::read_to_string(root.join("messages/message_missing.json"));
    assert!(data.is_ok());
    let message: Result<Value, _> = serde_json::from_str(&data.unwrap_or_default());
    assert!(message.is_ok());
    let message = message.unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "case");
    assert_eq!(
        message["remote"]["locations"][0]["missing_rfc3339"]
            .as_str()
            .map(|value| value.is_empty()),
        Some(false)
    );
    assert!(!root.join("triage/message_missing.md").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_spam_rejects_when_message_is_referenced_by_case() {
    let root = temp_root("spam-reference");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_spam", 42, None);
    write_triage(&root, "message_spam", "phishing note");
    let case_uid = "c20260521001";
    let _case_path = write_case_fixture(&root, "open", case_uid, "case-ref", &["message_spam"]);

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
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "error");
    assert_eq!(value["error_code"], "message_referenced");
    assert!(value["error"]
        .as_str()
        .is_some_and(|error| error.contains(&format!(
            "cases/open/{case_uid}-case-ref/data/messages.json"
        ))));
    assert!(root.join("triage/message_spam.md").exists());
    assert!(root.join("messages/message_spam.json").exists());
    assert!(!root.join(".afmail/messages/message_spam.txt").exists());
    assert!(root.join(".afmail/messages/message_spam.eml").exists());
    let blocked: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_spam.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(blocked["remote"]["locations"][0]["mailbox_name"], "INBOX");
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
    let _case_path = write_case_fixture(&root, "open", case_uid, "case-ref", &["message_spam"]);

    let (status, stdout) = run(&root, &["push", "spam", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_result");
    assert_eq!(value["pushed_count"], 0);
    assert_eq!(value["failed_count"], 1);
    assert_eq!(value["failures"][0]["error_code"], "message_referenced");
    assert!(value["failures"][0]["error"]
        .as_str()
        .is_some_and(|error| error.contains(&format!(
            "cases/open/{case_uid}-case-ref/data/messages.json"
        ))));
    assert!(root.join("messages/message_spam.json").exists());
    let blocked: Value = serde_json::from_str(
        &fs::read_to_string(root.join("messages/message_spam.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(blocked["remote"]["locations"][0]["mailbox_name"], "INBOX");
    assert!(blocked["workspace"]["push"]["pending"][0]["last_error"]
        .as_str()
        .is_some_and(|error| error.contains("message_referenced")));
    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["count"], 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_config_is_rejected() {
    let root = temp_root("legacy-config");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(
        &root.join(".afmail/config.json"),
        &json!({"schema_name": "config", "schema_version": 1, "imap_host": "imap.example.com"}),
    );
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "config_invalid");

    write_json(
        &root.join(".afmail/config.json"),
        &json!({
            "schema_name": "config",
        "schema_version": 1,
            "imap": {"host": null, "port": 993, "tls": true, "username": null, "password_secret": null},
            "pull": {"folders": ["INBOX"]},
            "case": {"default_group": "open"},
            "special_use": {},
            "smtp": {"host": null, "port": 587, "starttls": true, "tls_wrapper": false, "username": null, "password_secret": null, "from": null},
            "push": {"spam_mark_seen": true}
        }),
    );
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "config_invalid");

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
    assert_eq!(value["error_code"], "config_invalid");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_drafts_send_does_not_require_imap_move() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("no-move");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");
    let (case_uid, case_path, _) = create_case(
        &root,
        "case-one",
        Some("open"),
        Some("message_001"),
        Some("reply test case"),
    );
    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nreply_to_message_id: message_001\nto:\n  - alice@example.com\nsubject: \"Re: Contract\"\nattachments:\n---\n\nHi\n");
    assert!(fs::write(case_path.join("drafts/reply.md"), draft).is_ok());
    let imap = start_imap_server_with_move(Vec::new(), 3, false);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    let smtp = start_smtp_server_with_accept_timeout(Duration::from_secs(5));
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(imap.addr.port()), Some(smtp.addr.port())),
    );
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "reply.md"]).0,
        0
    );
    assert_eq!(run(&root, &["case", "compose", &case_uid, "reply.md"]).0, 0);
    let (status, stdout) = run(&root, &["push", "drafts-send", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["failed_count"], 0, "{value}");
    assert_eq!(value["pushed_count"], 1);
    assert!(smtp
        .data_rx
        .recv_timeout(Duration::from_secs(5))
        .map(|body| body.contains("Hi"))
        .unwrap_or(false));
    assert!(smtp.handle.join().is_ok());
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    assert!(appended
        .map(|items| items
            .iter()
            .any(|(folder, draft, body)| folder == "Sent" && !*draft && body.contains("Hi")))
        .unwrap_or(false));
    let stored = imap.stored.lock();
    assert!(stored.is_ok());
    assert!(stored
        .map(|items| items
            .iter()
            .any(|(_, _, query)| query.contains("\\Answered")))
        .unwrap_or(false));
    let push = parse_one(&run(&root, &["push", "list"]).1);
    assert_eq!(push["count"], 0);
    assert!(imap.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_drafts_send_new_message_skips_reply_to_step() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("new-send-no-reply-step");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, case_path, _) = create_case(
        &root,
        "new-thread",
        Some("open"),
        None,
        Some("new outbound case"),
    );
    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"New outbound\"\nattachments:\n---\n\nHi Bob\n");
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());

    let imap = start_imap_server(Vec::new(), 4);
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
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(imap.addr.port()), Some(smtp.addr.port())),
    );
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(run(&root, &["case", "compose", &case_uid, "new.md"]).0, 0);

    let (status, stdout) = run(&root, &["push", "drafts-send", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    let dry_run = parse_one(&stdout);
    assert_eq!(
        dry_run["items"][0]["actions"],
        json!(["smtp_send", "append_to_mailbox_id_sent"])
    );

    let (status, stdout) = run(&root, &["push", "drafts-send", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["failed_count"], 0, "{value}");
    assert_eq!(value["pushed_count"], 1);
    assert!(smtp
        .data_rx
        .recv_timeout(Duration::from_secs(5))
        .map(|body| body.contains("Hi Bob"))
        .unwrap_or(false));
    assert!(smtp.handle.join().is_ok());
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    assert!(appended
        .map(|items| items
            .iter()
            .any(|(folder, draft, body)| folder == "Sent" && !*draft && body.contains("Hi Bob")))
        .unwrap_or(false));
    let stored = imap.stored.lock();
    assert!(stored.is_ok());
    assert!(stored.map(|items| items.is_empty()).unwrap_or(false));
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    assert!(imap.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_drafts_send_resumes_after_completed_smtp_step() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("send-resume-after-smtp");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    let (case_uid, case_path, _) = create_case(
        &root,
        "resume-thread",
        Some("open"),
        None,
        Some("resume outbound case"),
    );
    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Resume outbound\"\nattachments:\n---\n\nResume me\n");
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(run(&root, &["case", "compose", &case_uid, "new.md"]).0, 0);
    let push = parse_one(&run(&root, &["push", "list"]).1);
    let push_id = push["items"][0]["push_id"].as_str().unwrap_or_default();
    let push_path = root.join(".afmail/push").join(format!("{push_id}.json"));
    let mut push_item: Value =
        serde_json::from_str(&fs::read_to_string(&push_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    push_item["step_states"] = json!([{
        "index": 0,
        "label": "smtp_send",
        "status": "succeeded",
        "started_rfc3339": "2026-06-09T00:00:00Z",
        "completed_rfc3339": "2026-06-09T00:00:01Z",
        "result_summary": "smtp_send succeeded"
    }]);
    write_json(&push_path, &push_item);

    let imap = start_imap_server(Vec::new(), 3);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    // No SMTP host is configured. If step_states is ignored, this push fails
    // before it can append to Sent.
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(imap.addr.port()), None),
    );

    let (status, stdout) = run(&root, &["push", "drafts-send", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["failed_count"], 0, "{value}");
    assert_eq!(value["pushed_count"], 1);
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    assert!(appended
        .map(|items| items
            .iter()
            .any(|(folder, draft, body)| folder == "Sent" && !*draft && body.contains("Resume me")))
        .unwrap_or(false));
    assert!(imap.handle.join().is_ok());
    assert_eq!(parse_one(&run(&root, &["push", "list"]).1)["count"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn push_failure_records_step_level_state() {
    let root = temp_root("push-step-state");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    let (case_uid, case_path, _) = create_case(
        &root,
        "step-state",
        Some("open"),
        None,
        Some("step state case"),
    );
    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nto:\n  - bob@example.com\ncc: []\nsubject: \"Step state\"\nattachments:\n---\n\nHello\n");
    assert!(fs::write(case_path.join("drafts/new.md"), draft).is_ok());
    assert_eq!(
        run(&root, &["case", "draft", "validate", &case_uid, "new.md"]).0,
        0
    );
    assert_eq!(run(&root, &["case", "compose", &case_uid, "new.md"]).0, 0);

    let (status, stdout) = run(&root, &["push", "drafts-send", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["pushed_count"], 0);
    assert_eq!(value["failed_count"], 1);

    let push = parse_one(&run(&root, &["push", "list"]).1);
    let item = &push["items"][0];
    assert_eq!(item["attempt_count"], 1);
    assert_eq!(item["step_states"][0]["label"], "smtp_send");
    assert_eq!(item["step_states"][0]["status"], "failed");
    assert!(item["step_states"][0]["error_code"].as_str().is_some());
}

#[test]
fn special_use_push_list_and_push_lock_work() {
    let root = temp_root("special-use-push");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");

    let (_archive_uid, _archive_path, value) = create_archive_message(
        &root,
        "done",
        Some("message_001"),
        Some("archive later"),
        Some("archive later"),
    );
    assert_eq!(value["code"], "archive_message_created");
    assert_eq!(value["queued"], true);
    assert_eq!(value["location_count"], 1);
    let push_id = value["push_id"].as_str().unwrap_or_default().to_string();
    assert!(!push_id.is_empty());

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_list");
    assert_eq!(value["count"], 1);
    assert_eq!(value["items"][0]["kind"], "message_action");
    assert_eq!(value["items"][0]["action"], "archive");
    assert_eq!(
        value["items"][0]["steps"],
        json!([{"move_to_mailbox_id": "archive"}])
    );

    let workspace_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".afmail/workspace.lock"));
    assert!(workspace_lock.is_ok());
    let Ok(workspace_lock) = workspace_lock else {
        return;
    };
    assert!(FileExt::try_lock(&workspace_lock).is_ok());
    let (status, stdout) = run(&root, &["push", "archive", "--confirm"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "workspace_locked");
    assert_eq!(value["retryable"], true);
    assert!(FileExt::unlock(&workspace_lock).is_ok());

    let (status, stdout) = run(&root, &["push", "remove", &push_id]);
    assert_eq!(status, 2);
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "error");
    assert!(error["error"]
        .as_str()
        .is_some_and(|text| text.contains("unrecognized subcommand 'remove'")));

    let (status, stdout) = run(&root, &["push", "list"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["count"], 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn status_reports_progress_while_workspace_is_exclusively_locked() {
    let root = temp_root("status-progress-locked");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_json(
        &root.join(".afmail/workspace.progress.json"),
        &json!({
            "schema_name": "workspace_progress",
            "schema_version": 1,
            "command": "pull",
            "status": "running",
            "phase": "pull_mailbox_bodies_progress",
            "message": "fetching new message bodies",
            "started_rfc3339": "2026-06-11T00:00:00Z",
            "updated_rfc3339": "2026-06-11T00:00:01Z",
            "elapsed_ms": 1000,
            "fields": {
                "stage": "fetch_done",
                "mailbox_id": "trash",
                "mailbox_name": "Trash",
                "index": 2,
                "mailbox_count": 4,
                "uid_count": 199,
                "processed_count": 125,
                "batch_index": 5,
                "batch_count": 8
            },
            "result": null,
            "error": null
        }),
    );

    let workspace_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".afmail/workspace.lock"));
    assert!(workspace_lock.is_ok());
    let Ok(workspace_lock) = workspace_lock else {
        return;
    };
    assert!(FileExt::try_lock(&workspace_lock).is_ok());

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "status");
    assert_eq!(value["workspace_locked"], true);
    assert_eq!(value["progress"]["status"], "running");
    assert_eq!(value["progress"]["command"], "pull");
    assert_eq!(value["progress"]["phase"], "pull_mailbox_bodies_progress");
    assert_eq!(
        value["progress"]["summary"],
        "pull: Trash bodies 125/199, batch 5/8"
    );
    assert_eq!(value["progress"]["processed_count"], 125);
    assert_eq!(value["progress"]["total_count"], 199);
    assert_eq!(value["progress"]["batch_index"], 5);
    assert_eq!(value["progress"]["batch_count"], 8);
    assert_eq!(value["progress"]["mailbox_id"], "trash");
    assert_eq!(value["progress"]["mailbox_index"], 2);
    assert_eq!(value["progress"]["fields"]["stage"], "fetch_done");
    assert!(value["hint"]
        .as_str()
        .is_some_and(|text| text.contains("counts are omitted")));
    let object = value.as_object().cloned().unwrap_or_default();
    assert!(!object.contains_key("message_count"), "{stdout}");
    assert!(!object.contains_key("push_count"), "{stdout}");

    assert!(FileExt::unlock(&workspace_lock).is_ok());
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "status");
    assert!(value.get("workspace_locked").is_none());
    assert_eq!(value["message_count"], 0);
    assert_eq!(
        value["progress"]["summary"],
        "pull: Trash bodies 125/199, batch 5/8"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn workspace_lock_blocks_writers_but_allows_shared_readers() {
    let root = temp_root("workspace-lock");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_001", 1, None);
    let (case_uid, _case_path, _) = create_case(&root, "read-lock", None, None, Some("read lock"));

    let workspace_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".afmail/workspace.lock"));
    assert!(workspace_lock.is_ok());
    let Ok(workspace_lock) = workspace_lock else {
        return;
    };
    assert!(FileExt::try_lock_shared(&workspace_lock).is_ok());

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "status");

    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case");

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_notes");

    let (status, stdout) = run(&root, &["case", "move", &case_uid, "waiting"]);
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "workspace_locked");
    assert_eq!(value["retryable"], true);

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "trash",
            "message_001",
            "--reason",
            "locked writer check",
        ],
    );
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["error_code"], "workspace_locked");
    assert_eq!(value["retryable"], true);

    assert!(FileExt::unlock(&workspace_lock).is_ok());
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "trash",
            "message_001",
            "--reason",
            "after lock release",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_archive_already_in_archive_marks_archived_without_push() {
    let root = temp_root("archive-noop");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_archived", 7, None);
    let message_path = root.join("messages/message_archived.json");
    let mut message: Value =
        serde_json::from_str(&fs::read_to_string(&message_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    message["remote"]["locations"][0]["mailbox_name"] = json!("Archive");
    message["remote"]["locations"][0]["mailbox_id"] = json!("archive");
    write_json(&message_path, &message);
    write_triage(&root, "message_archived", "already handled");

    let (archive_uid, _archive_path, value) = create_archive_message(
        &root,
        "done",
        Some("message_archived"),
        Some("already archived"),
        Some("already archived"),
    );
    assert_eq!(value["code"], "archive_message_created");
    assert_eq!(value["queued"], false);
    assert_eq!(value["location_count"], 1);
    assert_eq!(value["queued_location_count"], 0);
    assert!(!root.join("triage/message_archived.md").exists());
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
    let message: Value =
        serde_json::from_str(&fs::read_to_string(&message_path).unwrap_or_default())
            .unwrap_or(Value::Null);
    assert_eq!(message["workspace"]["status"], "archived");
    assert_eq!(message["workspace"]["archive_uid"], archive_uid);
    assert!(message["workspace"].get("buckets").is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_assign_move_draft_and_send_contract_work() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("assign");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");

    let (case_uid, case_path, value) = create_case(
        &root,
        "acme-contract",
        Some("open"),
        Some("message_001"),
        Some("new contract case"),
    );
    assert_eq!(value["code"], "case_created");
    assert!(case_path.join("case.md").is_file());
    assert!(!root.join("triage/message_001.md").exists());

    let message_data = fs::read_to_string(root.join("messages/message_001.json"));
    assert!(message_data.is_ok());
    let message_value: Result<Value, _> = serde_json::from_str(&message_data.unwrap_or_default());
    assert!(message_value.is_ok());
    let message_value = message_value.unwrap_or(Value::Null);
    assert_eq!(message_value["workspace"]["status"], "case");
    let case_messages: Value = serde_json::from_str(
        &fs::read_to_string(case_path.join("data/messages.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(case_messages["message_ids"], json!(["message_001"]));

    let (status, _) = run(&root, &["case", "move", &case_uid, "waiting"]);
    assert_eq!(status, 0);
    let waiting_path = root.join(format!("cases/waiting/{case_uid}-acme-contract"));
    assert!(waiting_path.join("case.md").is_file());

    let draft = format!("---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: reply\nreply_to_message_id: message_001\nto:\n  - alice@example.com\ncc: []\nsubject: \"Re: Contract renewal\"\nattachments:\n---\n\nHi Alice\n");
    assert!(fs::write(waiting_path.join("drafts/reply.md"), draft).is_ok());
    let (status, stdout) = run(&root, &["case", "draft", "validate", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "draft_valid");

    let imap = start_imap_server(Vec::new(), 20);
    assert!(imap.is_some());
    let imap = match imap {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(imap.addr.port()), None),
    );
    let (status, stdout) = run(&root, &["case", "compose", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_queued");
    let first_message_id = value["message_id"].as_str().unwrap_or_default().to_string();
    assert!(root.join(".afmail/push").read_dir().is_ok());

    let (status, stdout) = run(&root, &["push", "drafts", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_dry_run");
    assert_eq!(value["count"], 1);

    let (status, stdout) = run(&root, &["push", "drafts", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_result");
    assert_eq!(value["pushed_count"], 1);
    let staged = fs::read_to_string(root.join(format!("messages/{first_message_id}.json")));
    assert!(staged
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|value| value["workspace"]["status"] == "staged_draft")
        .unwrap_or(false));

    let smtp = start_smtp_server();
    assert!(smtp.is_some());
    let smtp = match smtp {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(imap.addr.port()), Some(smtp.addr.port())),
    );
    let (status, stdout) = run(&root, &["case", "compose", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let message_id = value["message_id"].as_str().unwrap_or_default().to_string();
    assert!(!message_id.is_empty());

    let (status, stdout) = run(&root, &["push", "drafts-send", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["items"][0]["actions"][0], "smtp_send");

    let (status, stdout) = run(&root, &["push", "drafts-send", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "push_result");
    assert_eq!(value["pushed_count"], 1, "{value}");
    assert!(root
        .join(format!(".afmail/messages/{message_id}.eml"))
        .is_file());
    let outbound = fs::read_to_string(root.join(format!(".afmail/messages/{message_id}.eml")));
    assert!(outbound
        .as_ref()
        .map(|text| text.contains("In-Reply-To: <message_001@example.com>"))
        .unwrap_or(false));
    assert!(outbound
        .as_ref()
        .map(|text| text.contains("Hi Alice"))
        .unwrap_or(false));
    let server_data = smtp
        .data_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    assert!(server_data.contains("Subject: Re: Contract renewal"));
    assert!(server_data.contains("Hi Alice"));
    assert!(smtp.handle.join().is_ok());
    assert!(imap.handle.join().is_ok());
    let appended = imap.appended.lock();
    assert!(appended.is_ok());
    let appended = appended.map(|items| items.clone()).unwrap_or_default();
    assert_eq!(appended.len(), 2);
    assert!(appended
        .iter()
        .any(|(folder, draft, body)| folder == "Drafts" && *draft && body.contains("Hi Alice")));
    assert!(appended
        .iter()
        .any(|(folder, draft, body)| folder == "Sent" && !*draft && body.contains("Hi Alice")));
    let moved = imap.moved.lock();
    assert!(moved.is_ok());
    assert!(moved.map(|items| items.is_empty()).unwrap_or(false));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn triage_assign_attachment_fetch_work() {
    let root = temp_root("assign-existing");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert!(fs::create_dir_all(root.join("source")).is_ok());
    assert!(fs::write(root.join("source/pricing.txt"), "pricing").is_ok());

    write_message(&root, "message_001", 1, Some("source/pricing.txt"));
    write_triage(&root, "message_001", "first note");
    let (case_uid, case_path, _) = create_case(
        &root,
        "case-one",
        Some("open"),
        Some("message_001"),
        Some("first case message"),
    );

    write_message(&root, "message_missing", 9, None);
    write_triage(&root, "message_missing", "missing case");
    let (_missing_uid, _missing_path, value) = create_case(
        &root,
        "missing-case",
        None,
        Some("message_missing"),
        Some("new missing case"),
    );
    assert_eq!(value["code"], "case_created");
    assert_eq!(value["group"], "open");
    assert!(!root.join("triage/message_missing.md").is_file());

    write_message(&root, "message_002", 2, None);
    write_triage(&root, "message_002", "second note");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_002",
            "--group",
            "open",
            "--reason",
            "same case",
        ],
    );
    assert_eq!(status, 2);
    assert_eq!(parse_one(&stdout)["code"], "error");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "add",
            &case_uid,
            "message_002",
            "--reason",
            "same case",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["created_case"], false);
    let messages = fs::read_to_string(case_path.join("data/messages.json"));
    assert!(messages.is_ok());
    let messages: Result<Value, _> = serde_json::from_str(&messages.unwrap_or_default());
    assert!(messages.is_ok());
    let messages = messages.unwrap_or(Value::Null);
    assert_eq!(messages["message_ids"].as_array().map(|a| a.len()), Some(2));

    let (status, stdout) = run(
        &root,
        &["message", "attachment", "fetch", "message_001", "2"],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "attachment_saved");
    assert_eq!(value["storage"], "message_cache");
    assert!(root
        .join(".afmail/messages/message_001.files/pricing.txt")
        .is_file());
    assert!(!case_path.join("files/pricing.txt").is_file());
    let message_view =
        fs::read_to_string(case_path.join("views/messages/message_001.md")).unwrap_or_default();
    assert!(message_view.contains("fetched: `.afmail/messages/message_001.files/pricing.txt`"));

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
