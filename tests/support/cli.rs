use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static COMMAND_TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn temp_root(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("afmail-it-{name}-{}-{stamp}", std::process::id()))
}

pub fn run(cwd: &Path, args: &[&str]) -> (i32, String) {
    let _command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let output = Command::new(bin()).current_dir(cwd).args(args).output();
    assert!(output.is_ok());
    let output = match output {
        Ok(v) => v,
        Err(_) => return (99, String::new()),
    };
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    assert!(
        stdout.is_empty() || stderr.is_empty(),
        "default split routing produced both streams; use --output-to stdout \
         when the test needs one ordered event stream\nstdout: {stdout}\nstderr: {stderr}"
    );
    (
        output.status.code().unwrap_or(99),
        if stdout.is_empty() { stderr } else { stdout },
    )
}

#[allow(dead_code)]
pub fn run_allow_stderr(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let _command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let output = Command::new(bin()).current_dir(cwd).args(args).output();
    assert!(output.is_ok());
    let output = match output {
        Ok(v) => v,
        Err(_) => return (99, String::new(), String::new()),
    };
    (
        output.status.code().unwrap_or(99),
        String::from_utf8(output.stdout).unwrap_or_default(),
        String::from_utf8(output.stderr).unwrap_or_default(),
    )
}

pub fn parse_one(stream: &str) -> Value {
    let parsed = serde_json::from_str(stream.trim());
    assert!(parsed.is_ok(), "protocol stream was not JSON: {stream}");
    match parsed {
        Ok(v) => protocol_payload(v),
        Err(_) => Value::Null,
    }
}

fn protocol_payload(value: Value) -> Value {
    assert!(
        agent_first_data::validate_protocol_event(&value, false).is_ok(),
        "stdout was not a valid AFDATA protocol event: {value}"
    );
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    value.get(kind).cloned().unwrap_or(Value::Null)
}

#[allow(dead_code)]
pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_default()).unwrap_or(Value::Null)
}

#[allow(dead_code)]
pub fn write_json(path: &Path, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    assert!(fs::write(path, text).is_ok(), "write {}", path.display());
}

#[allow(dead_code)]
pub fn write_message(root: &Path, message_id: &str, uid: u64, source_path: Option<&str>) {
    let mut attachment = json!({
        "part_id": "2",
        "filename": "pricing.txt",
        "content_type": "text/plain",
        "size_bytes": 7
    });
    if let Some(source) = source_path {
        attachment["source_path"] = json!(source);
    }
    let msg = json!({
        "schema_name": "message",
        "schema_version": 2,
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
        &root.join(format!(".afmail/messages/{message_id}.json")),
        &json!({
            "schema_name": "message_metadata",
            "schema_version": 2,
            "message_id": message_id,
            "remote": msg["remote"].clone()
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

#[allow(dead_code)]
pub fn write_triage(root: &Path, message_id: &str, notes: &str) {
    let text = format!(
        "---\nkind: triage_view\nmessage_id: {message_id}\n---\n\n# Contract renewal\n\n### {message_id} - 2026-05-21T10:00:00Z - Alice <alice@example.com>\n\n```text\nBody for {message_id}\n```\n"
    );
    let _ = notes;
    assert!(fs::write(root.join(format!("triage/{message_id}.md")), text).is_ok());
}

#[allow(dead_code)]
pub fn update_message_json(root: &Path, message_id: &str, update: impl FnOnce(&mut Value)) {
    let path = root.join(format!("messages/{message_id}.json"));
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    update(&mut value);
    if value.get("remote").is_some() {
        let metadata_path = root.join(format!(".afmail/messages/{message_id}.json"));
        let mut metadata = read_json(&metadata_path);
        if !metadata.is_object() {
            metadata = json!({
                "schema_name": "message_metadata",
                "schema_version": 2,
                "message_id": message_id,
            });
        }
        metadata["remote"] = value["remote"].clone();
        write_json(&metadata_path, &metadata);
    }
    write_json(&path, &value);
    rewrite_message_eml_from_json(root, message_id, &value);
}

fn rewrite_message_eml_from_json(root: &Path, message_id: &str, value: &Value) {
    let eml_path = root.join(format!(".afmail/messages/{message_id}.eml"));
    if !eml_path.is_file() {
        return;
    }
    let existing = fs::read_to_string(&eml_path).unwrap_or_default();
    let body = existing
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("Body\r\n");
    let header_string = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(ToString::to_string)
    };
    let header_list = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|text| !text.is_empty())
    };
    let message_id_header =
        header_string("rfc822_message_id").unwrap_or_else(|| format!("<{message_id}@example.com>"));
    let from = header_string("from").unwrap_or_else(|| "alice@example.com".to_string());
    let to = header_list("to").unwrap_or_else(|| "me@example.com".to_string());
    let date = value
        .get("received_rfc3339")
        .and_then(Value::as_str)
        .or_else(|| value.get("sent_rfc3339").and_then(Value::as_str))
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|date| date.to_rfc2822())
        .unwrap_or_else(|| "Thu, 21 May 2026 10:00:00 +0000".to_string());
    let subject = header_string("subject").unwrap_or_default();
    let mut headers = vec![
        format!("Message-ID: {message_id_header}"),
        format!("From: {from}"),
        format!("To: {to}"),
    ];
    if let Some(cc) = header_list("cc") {
        headers.push(format!("Cc: {cc}"));
    }
    if let Some(reply_to) = header_list("reply_to") {
        headers.push(format!("Reply-To: {reply_to}"));
    }
    headers.push(format!("Date: {date}"));
    headers.push(format!("Subject: {subject}"));
    if let Some(in_reply_to) = header_string("in_reply_to") {
        headers.push(format!("In-Reply-To: {in_reply_to}"));
    }
    if let Some(references) = header_list("references").or_else(|| header_string("references")) {
        headers.push(format!("References: {references}"));
    }
    headers.push("MIME-Version: 1.0".to_string());
    headers.push("Content-Type: multipart/mixed; boundary=\"afmail-test\"".to_string());
    assert!(fs::write(&eml_path, format!("{}\r\n\r\n{body}", headers.join("\r\n"))).is_ok());
}

fn bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_afmail") {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(env!("CARGO_BIN_EXE_afmail")),
    }
}
