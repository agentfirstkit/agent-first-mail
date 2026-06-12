use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "support/env.rs"]
mod test_env;

const GREENMAIL_SMTP_PORT: &str = "3025/tcp";
const GREENMAIL_IMAP_PORT: &str = "3143/tcp";
const MAIL_DOMAIN: &str = "localhost";
const ME_LOGIN: &str = "me";
const ALICE_LOGIN: &str = "alice";
const PASSWORD: &str = "secret";

#[test]
#[ignore]
fn docker_greenmail_pull_reply_send_e2e() {
    if test_env::env_value("AFMAIL_E2E").as_deref() != Some("1") {
        return;
    }

    let suffix = unique_suffix();
    let container_name = format!("afmail-e2e-greenmail-{suffix}");
    let image = test_env::env_value("AFMAIL_E2E_GREENMAIL_IMAGE")
        .unwrap_or_else(|| "greenmail/standalone:2.1.8".to_string());
    let _docker_guard = DockerE2eGuard {
        containers: vec![container_name.clone()],
    };
    let greenmail_opts = format!(
        "GREENMAIL_OPTS=-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled -Dgreenmail.users={ME_LOGIN}:{PASSWORD}@{MAIL_DOMAIN},{ALICE_LOGIN}:{PASSWORD}@{MAIL_DOMAIN}"
    );

    docker_success(
        &[
            "run",
            "-d",
            "--rm",
            "--name",
            &container_name,
            "-e",
            &greenmail_opts,
            "-p",
            "127.0.0.1::3025",
            "-p",
            "127.0.0.1::3143",
            &image,
        ],
        "start GreenMail container",
    );
    let smtp_port = docker_mapped_port(&container_name, GREENMAIL_SMTP_PORT);
    let imap_port = docker_mapped_port(&container_name, GREENMAIL_IMAP_PORT);
    assert!(
        wait_until(Duration::from_secs(45), || imap_login_works(
            imap_port, ME_LOGIN, PASSWORD
        )),
        "GreenMail IMAP did not become ready"
    );

    let root = TempRoot::new("greenmail-e2e");
    assert!(fs::create_dir_all(root.path()).is_ok());
    assert_eq!(run(root.path(), &["init"]).0, 0);
    write_json(
        &root.path().join(".afmail/config.json"),
        &test_config(imap_port, smtp_port),
    );

    seed_inbound_message(smtp_port, &suffix);
    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "INBOX",
            "Hello from real GreenMail"
        )),
        "seeded inbound message was not visible over IMAP"
    );

    let (status, stdout) = run(root.path(), &["remote", "test"]);
    assert_eq!(status, 0, "{stdout}");
    let remote = parse_one(&stdout);
    assert_eq!(remote["code"], "remote_test_result");
    assert_eq!(remote["capabilities"]["move"], true);

    ensure_remote_folder(root.path(), "Drafts");
    ensure_remote_folder(root.path(), "Sent");
    ensure_remote_folder(root.path(), "Archive");
    ensure_remote_folder(root.path(), "Junk");
    ensure_remote_folder(root.path(), "Trash");

    let (status, stdout) = run(root.path(), &["pull"]);
    assert_eq!(status, 0, "{stdout}");
    let pull = parse_one(&stdout);
    assert_eq!(pull["code"], "pull_result");
    assert_eq!(pull["new_message_count"], 1);
    assert_eq!(pull["triage_created_count"], 1);

    let inbound_message_id = single_message_id(root.path(), "inbound");
    let (status, stdout) = run(
        root.path(),
        &[
            "case",
            "create",
            "--name",
            "greenmail-e2e",
            "--message",
            &inbound_message_id,
            "--group",
            "open",
            "--reason",
            "real inbound message needs a reply",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let case_created = parse_one(&stdout);
    let case_uid = case_created["case_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let case_path = case_created["case_path"].as_str().unwrap_or_default();
    assert!(!case_uid.is_empty(), "{case_created}");
    assert!(!case_path.is_empty(), "{case_created}");

    let draft_path = root.path().join(case_path).join("drafts/reply.md");
    let draft = format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: reply\nreply_to_message_id: {inbound_message_id}\nto:\n  - alice@localhost\ncc: []\nsubject: \"Re: Docker GreenMail inbound\"\nattachments:\n---\n\nHi Alice, this reply went through the real SMTP server.\n"
    );
    assert!(fs::write(&draft_path, draft).is_ok());
    let (status, stdout) = run(
        root.path(),
        &["case", "draft", "validate", &case_uid, "reply.md"],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(root.path(), &["case", "compose", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    let queued = parse_one(&stdout);
    assert_eq!(queued["code"], "push_queued");
    let outbound_message_id = queued["message_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!outbound_message_id.is_empty());

    let (status, stdout) = run(root.path(), &["push", "drafts-send", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let pushed = parse_one(&stdout);
    assert_eq!(pushed["code"], "push_result");
    assert_eq!(pushed["pushed_count"], 1);
    assert_eq!(pushed["failed_count"], 0);
    assert_eq!(push_json_count(root.path()), 0);

    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "Sent",
            "this reply went through the real SMTP server"
        )),
        "sent copy was not appended to the sender Sent folder"
    );
    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ALICE_LOGIN,
            PASSWORD,
            "INBOX",
            "this reply went through the real SMTP server"
        )),
        "recipient mailbox did not receive the SMTP message"
    );

    let (status, stdout) = run(root.path(), &["pull", "sent"]);
    assert_eq!(status, 0, "{stdout}");
    let sent_pull = parse_one(&stdout);
    assert_eq!(sent_pull["new_message_count"], 0);
    assert_eq!(sent_pull["updated_location_count"], 1);

    let message_json = fs::read_to_string(
        root.path()
            .join(format!("messages/{outbound_message_id}.json")),
    )
    .unwrap_or_default();
    assert!(
        message_json.contains("\"mailbox_name\": \"Sent\""),
        "{message_json}"
    );
    // The reply carried RFC 5322 threading headers through real SMTP + IMAP.
    assert!(
        message_json.contains("\"in_reply_to\"") && message_json.contains("\"references\""),
        "outbound reply lost its threading headers: {message_json}"
    );

    seed_spam_message(smtp_port, &suffix);
    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "INBOX",
            "Spam from real GreenMail"
        )),
        "seeded spam message was not visible over IMAP"
    );

    let (status, stdout) = run(root.path(), &["pull"]);
    assert_eq!(status, 0, "{stdout}");
    let spam_pull = parse_one(&stdout);
    assert_eq!(spam_pull["new_message_count"], 1);
    assert_eq!(spam_pull["triage_created_count"], 1);

    let old_spam_message_id = message_id_by_subject(root.path(), "Docker GreenMail spam");
    let (status, stdout) = run(
        root.path(),
        &[
            "message",
            &old_spam_message_id,
            "spam",
            "--reason",
            "real spam message",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let queued_spam = parse_one(&stdout);
    assert_eq!(queued_spam["code"], "message_spam_marked");
    assert_eq!(queued_spam["message_ids"][0], old_spam_message_id);
    assert_eq!(queued_spam["queued"], true);
    assert_eq!(push_json_count(root.path()), 1);
    assert!(root
        .path()
        .join(format!("messages/{old_spam_message_id}.json"))
        .is_file());

    let (status, stdout) = run(root.path(), &["push", "spam", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let pushed_spam = parse_one(&stdout);
    assert_eq!(pushed_spam["code"], "push_result");
    assert_eq!(pushed_spam["pushed_count"], 1);
    assert_eq!(pushed_spam["failed_count"], 0);
    assert_eq!(push_json_count(root.path()), 0);

    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "Junk",
            "Spam from real GreenMail"
        )),
        "spam message was not moved into Junk"
    );
    assert!(
        wait_until(Duration::from_secs(15), || mailbox_message_seen(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "Junk",
            "Spam from real GreenMail"
        )),
        "spam message in Junk was not marked seen"
    );
    assert!(
        !mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "INBOX",
            "Spam from real GreenMail"
        ),
        "spam message should not remain in INBOX"
    );

    let new_spam_message_id = message_id_by_subject(root.path(), "Docker GreenMail spam");
    assert_eq!(new_spam_message_id, old_spam_message_id);
    assert!(root
        .path()
        .join(format!("messages/{old_spam_message_id}.json"))
        .exists());
    let spam_json = fs::read_to_string(
        root.path()
            .join(format!("messages/{new_spam_message_id}.json")),
    )
    .unwrap_or_default();
    assert!(spam_json.contains("\"status\": \"spam\""), "{spam_json}");
    assert!(
        spam_json.contains("\"mailbox_name\": \"Junk\""),
        "{spam_json}"
    );
}

fn test_config(imap_port: u16, smtp_port: u16) -> Value {
    json!({
        "schema_name": "config",
            "schema_version": 1,
        "imap": {
            "host": "127.0.0.1",
            "port": imap_port,
            "tls": false,
            "username": ME_LOGIN,
            "password_secret": PASSWORD
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
            "case.add": {"steps": [{"add_flags": ["\\Seen"]}]},
            "draft.save": {"steps": [{"append_to_mailbox_id": "drafts"}]},
            "draft.send": {"steps": [
                {"smtp_send": {}},
                {"append_to_mailbox_id": "sent"},
                {"add_flags": ["\\Answered"], "on": "reply_to_message"}
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
            "host": "127.0.0.1",
            "port": smtp_port,
            "starttls": false,
            "tls_wrapper": false,
            "username": null,
            "password_secret": null,
            "from": "Me <me@localhost>"
        },
        "workspace": {"language_bcp47": null, "timezone_utc_offset": "UTC"}
    })
}

fn seed_inbound_message(smtp_port: u16, suffix: &str) {
    let raw = e2e_message(
        &format!("seed-{suffix}@localhost"),
        "Docker GreenMail inbound",
        "Hello from real GreenMail.",
    );
    smtp_send(smtp_port, "alice@localhost", "me@localhost", raw.as_bytes());
}

fn seed_spam_message(smtp_port: u16, suffix: &str) {
    let raw = e2e_message(
        &format!("spam-{suffix}@localhost"),
        "Docker GreenMail spam",
        "Spam from real GreenMail.",
    );
    smtp_send(smtp_port, "alice@localhost", "me@localhost", raw.as_bytes());
}

fn e2e_message(message_id: &str, subject: &str, body: &str) -> String {
    format!(
        "Message-ID: <{message_id}>\r\nFrom: Alice <alice@localhost>\r\nTo: Me <me@localhost>\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
}

fn ensure_remote_folder(root: &Path, folder: &str) {
    let (status, stdout) = run(root, &["remote", "folders"]);
    assert_eq!(status, 0, "{stdout}");
    let folders = parse_one(&stdout);
    let already_exists = folders["mailboxes"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .any(|item| item["mailbox_name"].as_str() == Some(folder))
        })
        .unwrap_or(false);
    assert!(
        already_exists,
        "remote folder {folder} should exist in GreenMail setup"
    );
}

fn smtp_send(port: u16, mail_from: &str, rcpt_to: &str, raw: &[u8]) {
    let stream = TcpStream::connect(("127.0.0.1", port));
    assert!(stream.is_ok(), "connect SMTP on port {port} failed");
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let reader = stream.try_clone();
    assert!(reader.is_ok(), "clone SMTP stream failed");
    let mut reader = match reader {
        Ok(reader) => BufReader::new(reader),
        Err(_) => return,
    };
    assert_response(&mut reader, "220");
    smtp_cmd(&mut stream, &mut reader, "EHLO afmail-e2e\r\n", "250");
    smtp_cmd(
        &mut stream,
        &mut reader,
        &format!("MAIL FROM:<{mail_from}>\r\n"),
        "250",
    );
    smtp_cmd(
        &mut stream,
        &mut reader,
        &format!("RCPT TO:<{rcpt_to}>\r\n"),
        "250",
    );
    smtp_cmd(&mut stream, &mut reader, "DATA\r\n", "354");
    assert!(stream.write_all(raw).is_ok());
    if !raw.ends_with(b"\r\n") {
        assert!(stream.write_all(b"\r\n").is_ok());
    }
    assert!(stream.write_all(b".\r\n").is_ok());
    assert!(stream.flush().is_ok());
    assert_response(&mut reader, "250");
    smtp_cmd(&mut stream, &mut reader, "QUIT\r\n", "221");
}

fn smtp_cmd(
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    command: &str,
    expected_code: &str,
) {
    assert!(stream.write_all(command.as_bytes()).is_ok());
    assert!(stream.flush().is_ok());
    assert_response(reader, expected_code);
}

fn assert_response(reader: &mut BufReader<TcpStream>, expected_code: &str) {
    let response = read_smtp_response(reader);
    assert!(
        response.starts_with(expected_code),
        "SMTP response mismatch, expected {expected_code}: {response}"
    );
}

fn read_smtp_response(reader: &mut BufReader<TcpStream>) -> String {
    let mut response = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line);
        assert!(read.is_ok(), "read SMTP response failed");
        let read = read.unwrap_or(0);
        assert!(read > 0, "SMTP server closed connection");
        response.push_str(&line);
        let bytes = line.as_bytes();
        if bytes.len() >= 4 && bytes[3] != b'-' {
            break;
        }
    }
    response
}

fn imap_login_works(port: u16, username: &str, password: &str) -> bool {
    let Some(mut session) = imap_login(port, username, password) else {
        return false;
    };
    let ok = session.list(None, Some("*")).is_ok();
    let _ = session.logout();
    ok
}

fn mailbox_contains(
    port: u16,
    username: &str,
    password: &str,
    mailbox: &str,
    needle: &str,
) -> bool {
    let Some(mut session) = imap_login(port, username, password) else {
        return false;
    };
    let selected = session.examine(mailbox);
    if selected.is_err() {
        let _ = session.logout();
        return false;
    }
    let mailbox_status = selected.unwrap_or_default();
    if mailbox_status.exists == 0 {
        let _ = session.logout();
        return false;
    }
    let fetches = session.fetch("1:*", "BODY.PEEK[]");
    let Ok(fetches) = fetches else {
        let _ = session.logout();
        return false;
    };
    let mut all = String::new();
    for fetch in fetches.iter() {
        if let Some(body) = fetch.body() {
            all.push_str(&String::from_utf8_lossy(body));
        }
    }
    let _ = session.logout();
    all.contains(needle)
}

fn mailbox_message_seen(
    port: u16,
    username: &str,
    password: &str,
    mailbox: &str,
    needle: &str,
) -> bool {
    let Some(mut session) = imap_login(port, username, password) else {
        return false;
    };
    let selected = session.examine(mailbox);
    if selected.is_err() {
        let _ = session.logout();
        return false;
    }
    let mailbox_status = selected.unwrap_or_default();
    if mailbox_status.exists == 0 {
        let _ = session.logout();
        return false;
    }
    let fetches = session.fetch("1:*", "(FLAGS BODY.PEEK[])");
    let Ok(fetches) = fetches else {
        let _ = session.logout();
        return false;
    };
    let mut found = false;
    for fetch in fetches.iter() {
        let Some(body) = fetch.body() else {
            continue;
        };
        if String::from_utf8_lossy(body).contains(needle)
            && fetch
                .flags()
                .iter()
                .any(|flag| matches!(flag, imap::types::Flag::Seen))
        {
            found = true;
            break;
        }
    }
    let _ = session.logout();
    found
}

fn imap_login(port: u16, username: &str, password: &str) -> Option<imap::Session<TcpStream>> {
    let stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut client = imap::Client::new(stream);
    client.read_greeting().ok()?;
    client.login(username, password).ok()
}

fn wait_until<F>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

fn docker_success(args: &[&str], context: &str) {
    let output_result = Command::new("docker").args(args).output();
    assert!(
        output_result.is_ok(),
        "{context} failed: {:?}",
        output_result.as_ref().err()
    );
    let output = match output_result {
        Ok(output) => output,
        Err(_) => return,
    };
    assert!(
        output.status.success(),
        "{context} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn docker_output(args: &[&str], context: &str) -> String {
    let output_result = Command::new("docker").args(args).output();
    assert!(
        output_result.is_ok(),
        "{context} failed: {:?}",
        output_result.as_ref().err()
    );
    let output = match output_result {
        Ok(output) => output,
        Err(_) => return String::new(),
    };
    assert!(
        output.status.success(),
        "{context} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap_or_default()
}

fn docker_mapped_port(container: &str, private_port: &str) -> u16 {
    let output = docker_output(&["port", container, private_port], "inspect mapped port");
    let port_text = output.trim();
    let port = port_text
        .rsplit(':')
        .next()
        .and_then(|value| value.parse().ok());
    assert!(
        port.is_some(),
        "could not parse docker port output: {port_text}"
    );
    port.unwrap_or(0)
}

struct DockerE2eGuard {
    containers: Vec<String>,
}

impl Drop for DockerE2eGuard {
    fn drop(&mut self) {
        for name in &self.containers {
            let _ = Command::new("docker").args(["rm", "-f", name]).status();
        }
    }
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("afmail-{name}-{}", unique_suffix()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_afmail") {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(env!("CARGO_BIN_EXE_afmail")),
    }
}

fn run(cwd: &Path, args: &[&str]) -> (i32, String) {
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

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        assert!(fs::create_dir_all(parent).is_ok());
    }
    let data = serde_json::to_string_pretty(value).unwrap_or_default();
    assert!(fs::write(path, data).is_ok());
}

fn single_message_id(root: &Path, direction: &str) -> String {
    let mut ids = fs::read_dir(root.join(".afmail/messages"))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        return None;
                    }
                    let text = fs::read_to_string(path).ok()?;
                    let value = serde_json::from_str::<Value>(&text).ok()?;
                    if value["direction"].as_str() == Some(direction) {
                        value["message_id"].as_str().map(ToString::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ids.sort();
    assert_eq!(ids.len(), 1, "expected one {direction} message: {ids:?}");
    ids.into_iter().next().unwrap_or_default()
}

fn message_id_by_subject(root: &Path, subject: &str) -> String {
    let mut ids = fs::read_dir(root.join(".afmail/messages"))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        return None;
                    }
                    let text = fs::read_to_string(path).ok()?;
                    let value = serde_json::from_str::<Value>(&text).ok()?;
                    if value["subject"].as_str() == Some(subject) {
                        value["message_id"].as_str().map(ToString::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ids.sort();
    assert_eq!(
        ids.len(),
        1,
        "expected one message with subject {subject}: {ids:?}"
    );
    ids.into_iter().next().unwrap_or_default()
}

fn push_json_count(root: &Path) -> usize {
    root.join(".afmail/push")
        .read_dir()
        .map(|entries| {
            entries
                .filter(|entry| {
                    entry
                        .as_ref()
                        .map(|entry| {
                            entry.path().extension().and_then(|s| s.to_str()) == Some("json")
                        })
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn unique_suffix() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{stamp}", std::process::id())
}
