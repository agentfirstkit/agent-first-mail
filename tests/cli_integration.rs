#[cfg(feature = "ui")]
use agent_first_ui::{
    AgentFirstUiMessage, Layout, MessagePayload, find_capability, find_unit, validate_space,
    validate_space_bindings,
};
use fs4::FileExt;
use serde_json::{Value, json};
#[cfg(feature = "ui")]
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "ui")]
use std::process::Stdio;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "ui")]
#[allow(dead_code)]
#[path = "support/fixtures.rs"]
mod test_fixtures;

static NETWORK_TEST_LOCK: Mutex<()> = Mutex::new(());
static COMMAND_TEST_LOCK: Mutex<()> = Mutex::new(());

type ImapCommands = Arc<Mutex<Vec<String>>>;
type AppendedMessages = Arc<Mutex<Vec<(String, bool, String)>>>;
type CreatedFolders = Arc<Mutex<Vec<String>>>;
type MovedMessages = Arc<Mutex<Vec<(String, u64, String)>>>;
#[cfg(feature = "ui")]
type MailboxMessages = BTreeMap<String, Vec<(u64, Vec<u8>)>>;

enum TestImapMessages {
    Shared(Vec<(u64, Vec<u8>)>),
    #[cfg(feature = "ui")]
    ByMailbox(BTreeMap<String, Vec<(u64, Vec<u8>)>>),
}

#[test]
fn obsolete_flat_state_layout_is_rejected_and_reported_by_doctor() {
    let root = temp_root("obsolete-state-layout");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert!(fs::write(root.join(".afmail/pull-cursors.json"), "{}").is_ok());

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "workspace_layout_outdated");
    assert_eq!(error["legacy_paths"], json!([".afmail/pull-cursors.json"]));

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 1, "{stdout}");
    let doctor = parse_one(&stdout);
    assert!(doctor["issues"].as_array().is_some_and(|issues| {
        issues.iter().any(|issue| {
            issue["code"] == "workspace_layout_outdated"
                && issue["path"] == ".afmail/pull-cursors.json"
        })
    }));
}

impl TestImapMessages {
    fn for_mailbox(&self, _mailbox: &str) -> &[(u64, Vec<u8>)] {
        match self {
            Self::Shared(messages) => messages.as_slice(),
            #[cfg(feature = "ui")]
            Self::ByMailbox(messages) => messages
                .get(_mailbox)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        }
    }
}

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
    command_output(output)
}

fn run_with_env(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> (i32, String) {
    let _command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut command = Command::new(bin());
    command.current_dir(cwd).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command_output(command.output())
}

fn run_allow_stderr(cwd: &Path, args: &[&str]) -> (i32, String, String) {
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

fn current_push_plan_id(cwd: &Path) -> String {
    let (status, stdout) = run(cwd, &["push"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    value["plan_id"].as_str().unwrap_or_default().to_string()
}

fn run_confirmed_push(cwd: &Path) -> (i32, String) {
    let plan_id = current_push_plan_id(cwd);
    run(cwd, &["push", "--confirm", plan_id.as_str()])
}

fn command_output(output: std::io::Result<std::process::Output>) -> (i32, String) {
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

fn parse_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_one)
        .collect()
}

#[cfg(feature = "ui")]
fn parse_raw_json_lines(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn workspace_tree_snapshot(root: &Path) -> Vec<(String, bool, Vec<u8>, u128)> {
    fn visit(root: &Path, path: &Path, out: &mut Vec<(String, bool, Vec<u8>, u128)>) {
        let metadata = fs::symlink_metadata(path);
        assert!(metadata.is_ok(), "snapshot metadata: {}", path.display());
        let Ok(metadata) = metadata else {
            return;
        };
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let is_dir = metadata.is_dir();
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap_or_default()
        } else {
            Vec::new()
        };
        out.push((relative, is_dir, bytes, modified_ns));
        if is_dir {
            let entries = fs::read_dir(path);
            assert!(entries.is_ok(), "snapshot directory: {}", path.display());
            if let Ok(entries) = entries {
                let mut paths = entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .collect::<Vec<_>>();
                paths.sort();
                for child in paths {
                    visit(root, &child, out);
                }
            }
        }
    }

    let mut snapshot = Vec::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn workspace_runtime_lock_path(root: &Path) -> PathBuf {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let digest = blake3::hash(canonical.as_os_str().as_encoded_bytes()).to_hex();
    std::env::temp_dir()
        .join("afmail-workspace-locks")
        .join(format!("{digest}.lock"))
}

fn workspace_snapshot_differences(
    before: &[(String, bool, Vec<u8>, u128)],
    after: &[(String, bool, Vec<u8>, u128)],
) -> Vec<String> {
    let before_by_path = before
        .iter()
        .map(|entry| (entry.0.as_str(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    let after_by_path = after
        .iter()
        .map(|entry| (entry.0.as_str(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    before_by_path
        .keys()
        .chain(after_by_path.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(
            |path| match (before_by_path.get(path), after_by_path.get(path)) {
                (Some(before_entry), Some(after_entry)) if before_entry == after_entry => None,
                (Some(before_entry), Some(after_entry)) => Some(format!(
                    "{path}: type {} -> {}, bytes_equal={}, mtime {} -> {}",
                    before_entry.1,
                    after_entry.1,
                    before_entry.2 == after_entry.2,
                    before_entry.3,
                    after_entry.3
                )),
                (Some(_), None) => Some(format!("{path}: removed")),
                (None, Some(_)) => Some(format!("{path}: added")),
                (None, None) => None,
            },
        )
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

fn draft_with_compose(raw: String) -> String {
    let Some((frontmatter, body)) = raw.split_once("\n---\n") else {
        return raw;
    };
    format!(
        "{frontmatter}\n---\n# Draft\n\n<!-- afmail:compose:start -->\n{}\n<!-- afmail:compose:end -->\n",
        body.trim_matches(['\r', '\n'])
    )
}

#[cfg(feature = "ui")]
fn mail_batch_messages_by_mailbox(
    batch: &test_fixtures::MailBatch,
) -> Result<MailboxMessages, String> {
    let mut by_mailbox: MailboxMessages = BTreeMap::new();
    let mut next_uid_by_mailbox: BTreeMap<String, u64> = BTreeMap::new();
    for message in &batch.messages {
        let mailbox_name = batch.mailbox_name(&message.mailbox_id).ok_or_else(|| {
            format!(
                "fixture {} references unknown mailbox_id {}",
                message.fixture_id, message.mailbox_id
            )
        })?;
        let next_uid = next_uid_by_mailbox
            .entry(mailbox_name.to_string())
            .and_modify(|uid| *uid += 1)
            .or_insert(1);
        by_mailbox
            .entry(mailbox_name.to_string())
            .or_default()
            .push((*next_uid, batch.raw_message(message)?));
    }
    Ok(by_mailbox)
}

#[cfg(feature = "ui")]
fn fixture_message_id(root: &Path, batch: &test_fixtures::MailBatch, fixture_id: &str) -> String {
    let message = batch.message(fixture_id);
    assert!(message.is_some(), "fixture_id {fixture_id} not found");
    let Some(message) = message else {
        return String::new();
    };
    message_id_for_rfc822(root, &message.rfc822_message_id)
}

#[test]
#[cfg(feature = "ui")]
fn ui_snapshot_projects_reader_drafts_editor_and_delegation_references() {
    let batch = test_fixtures::load_mail_batch();
    assert!(batch.is_ok(), "{batch:?}");
    let Ok(batch) = batch else {
        return;
    };
    let messages_by_mailbox = mail_batch_messages_by_mailbox(&batch);
    assert!(messages_by_mailbox.is_ok(), "{messages_by_mailbox:?}");
    let Ok(messages_by_mailbox) = messages_by_mailbox else {
        return;
    };
    let root = temp_root("ui-product-snapshot");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let server = start_imap_mailbox_server(messages_by_mailbox, 20);
    assert!(server.is_some());
    let Some(server) = server else {
        return;
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );
    let (status, stdout) = run(&root, &["pull"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(server.handle.join().is_ok());

    let message_id = fixture_message_id(&root, &batch, "refund_initial");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "create",
            "--name",
            "ui-draft-review",
            "--message",
            &message_id,
            "--group",
            "support",
            "--reason",
            "exercise AFUI reader and draft projection",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let case_created = parse_one(&stdout);
    let case_ref = case_created["case_uid"].as_str().unwrap_or_default();
    assert!(!case_ref.is_empty(), "{case_created}");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            case_ref,
            "--to",
            "customer@example.com",
            "--cc",
            "team@example.com",
            "--subject",
            "Refund follow-up",
            "--body",
            "Hello,\n\nWe are reviewing your refund.",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_created = parse_one(&stdout);
    let draft_name = draft_created["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!draft_name.is_empty(), "{draft_created}");
    let attachment_path = root.join("refund.txt");
    assert!(fs::write(&attachment_path, "refund evidence").is_ok());
    let attachment_arg = attachment_path.to_string_lossy().to_string();
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "attach",
            case_ref,
            &draft_name,
            &attachment_arg,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(&root, &["case", "draft", "validate", case_ref, &draft_name]);
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "create",
            "--name",
            "archived-ui-draft",
            "--group",
            "support",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let archived_case_created = parse_one(&stdout);
    let archived_case_ref = archived_case_created["case_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &archived_case_ref,
            "--to",
            "archive@example.com",
            "--subject",
            "Retained remote draft",
            "--body",
            "Keep this draft available for reference.",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let archived_draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "upload",
            &archived_case_ref,
            &archived_draft_name,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "archive",
            &archived_case_ref,
            "--reason",
            "retain queued draft while closing the case",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "case_has_local_drafts");

    let (status, stdout) = run(&root, &["ui", "snapshot"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["agent_first_ui"], "0.2");
    assert_eq!(value["type"], "snapshot");
    let message = serde_json::from_value::<AgentFirstUiMessage>(value);
    assert!(message.is_ok(), "{message:?}");
    let Ok(message) = message else {
        return;
    };
    let MessagePayload::Snapshot { space, .. } = message.payload else {
        return;
    };
    assert!(validate_space(&space).is_ok());
    assert!(validate_space_bindings(&space).is_ok());
    let afmail = &space.as_value()["afmail"];
    assert_eq!(afmail["surface"]["genre"], "triage");
    assert!(space.as_value().get("host_web").is_none());
    assert!(afmail.get("status").is_none());
    assert!(afmail.get("ui").is_none());
    assert!(afmail.get("logs").is_none());

    let units = afmail["units"].as_array();
    assert!(units.is_some(), "{afmail}");
    let Some(units) = units else {
        return;
    };
    let unit_ids = units
        .iter()
        .filter_map(|unit| unit["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        unit_ids,
        ["mail_queue", "draft_list", "draft_editor", "mailbox_glance"]
    );
    for retired in [
        "inbox_triage",
        "active_cases",
        "push_queue",
        "notifications",
        "archived_cases",
        "activity_log",
        "mail_terminal",
    ] {
        assert!(find_unit(&space, "afmail", retired).is_none(), "{retired}");
    }

    let capabilities = afmail["capabilities"].as_array();
    assert!(capabilities.is_some(), "{afmail}");
    let Some(capabilities) = capabilities else {
        return;
    };
    let capability_ids = capabilities
        .iter()
        .filter_map(|capability| capability["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(capability_ids, ["draft_change"]);

    let mail_items = afmail["mail"]["items"].as_array();
    assert!(mail_items.is_some(), "{afmail}");
    let Some(mail_items) = mail_items else {
        return;
    };
    assert_eq!(mail_items.len(), 30);
    let projected_message = mail_items
        .iter()
        .find(|item| item["message"]["message_id"] == message_id);
    assert!(projected_message.is_some(), "{mail_items:?}");
    let Some(projected_message) = projected_message else {
        return;
    };
    assert!(projected_message["view"]["body_text"].is_string());
    assert!(projected_message["view"]["workspace_status"].is_string());

    let drafts = afmail["drafts"]["items"].as_array();
    assert!(drafts.is_some(), "{afmail}");
    let Some(drafts) = drafts else {
        return;
    };
    assert_eq!(drafts.len(), 2, "{drafts:?}");
    let draft = drafts.iter().find(|draft| draft["case_ref"] == case_ref);
    assert!(draft.is_some(), "{drafts:?}");
    let Some(draft) = draft else {
        return;
    };
    assert_eq!(draft["case_ref"], case_ref);
    assert_eq!(draft["draft_name"], draft_name);
    assert_eq!(draft["reference"], format!("{case_ref}/{draft_name}"));
    assert_eq!(draft["to"], json!(["customer@example.com"]));
    assert_eq!(draft["cc"], json!(["team@example.com"]));
    assert_eq!(draft["subject"], "Refund follow-up");
    assert!(
        draft["body"]
            .as_str()
            .unwrap_or_default()
            .contains("reviewing your refund")
    );
    assert_eq!(draft["attachments"][0]["exists"], true);
    assert_eq!(draft["validation"]["state"], "valid");
    assert_eq!(draft["validation"]["recorded"], true);
    assert_eq!(draft["availability"]["editable"], true);
    assert_eq!(draft["availability"]["usable"], true);
    let archived_draft = drafts
        .iter()
        .find(|draft| draft["case_ref"] == archived_case_ref);
    assert!(archived_draft.is_some(), "{drafts:?}");
    let Some(archived_draft) = archived_draft else {
        return;
    };
    assert_eq!(archived_draft["draft_name"], archived_draft_name);
    assert_eq!(archived_draft["case_archived"], false);
    assert_eq!(archived_draft["status"], "queued_upload");
    assert_eq!(archived_draft["queued_action"], "save_draft");
    assert_eq!(archived_draft["remote_changed"], false);
    assert_eq!(archived_draft["availability"]["editable"], false);
    assert_eq!(
        space.as_value()["session_operator"]["selection"]["draft"],
        drafts[0]
    );
    assert_eq!(drafts[0]["case_ref"], case_ref);

    let queue = find_unit(&space, "afmail", "mail_queue");
    assert!(queue.is_some());
    let Some(queue) = queue else {
        return;
    };
    assert_eq!(queue.kind, "list");
    assert_eq!(
        queue.field("badge_key"),
        Some(&json!("view.workspace_status"))
    );
    assert_eq!(
        queue
            .field("fields")
            .and_then(|fields| fields.pointer("/1/visible_when/empty")),
        Some(&json!(false))
    );
    assert_eq!(
        queue
            .field("fields")
            .and_then(|fields| fields.pointer("/3/visible_when/gt")),
        Some(&json!(0))
    );
    assert!(queue.field("fields").is_some_and(|fields| {
        fields.as_array().is_some_and(|items| {
            items.iter().all(|field| {
                !matches!(
                    field.get("key").and_then(Value::as_str),
                    Some("view.workspace_status" | "view.direction")
                )
            })
        })
    }));
    assert_eq!(
        queue.field("reference_binding"),
        Some(&json!("record:message.message_id"))
    );
    assert_eq!(queue.field("reference_kind"), Some(&json!("message")));
    assert!(queue.field("reference_label").is_none());
    assert_eq!(queue.field("body_key"), Some(&json!("view.body_text")));
    assert_eq!(
        queue.field("no_body_text"),
        Some(&json!("This message has no text body."))
    );
    assert_eq!(
        queue
            .field("interactions")
            .and_then(|value| value["select"][0]["path"].as_str()),
        Some("selection.message")
    );

    assert!(find_unit(&space, "afmail", "message_header").is_none());
    assert!(find_unit(&space, "afmail", "mail_reader").is_none());

    let draft_list = find_unit(&space, "afmail", "draft_list");
    assert!(draft_list.is_some());
    let Some(draft_list) = draft_list else {
        return;
    };
    assert_eq!(draft_list.field("reference_kind"), Some(&json!("draft")));
    assert_eq!(draft_list.field("body_key"), Some(&json!("body")));
    assert_eq!(
        draft_list
            .field("fields")
            .and_then(|fields| fields.pointer("/3/visible_when/empty")),
        Some(&json!(false))
    );
    assert_eq!(
        draft_list
            .field("interactions")
            .and_then(|value| value["select"][0]["path"].as_str()),
        Some("selection.draft")
    );

    let editor = find_unit(&space, "afmail", "draft_editor");
    assert!(editor.is_some());
    let Some(editor) = editor else {
        return;
    };
    assert_eq!(editor.kind, "form");
    assert_eq!(editor.field("reference_kind"), Some(&json!("draft")));
    assert_eq!(
        editor.field("submit_capability"),
        Some(&json!("draft_change"))
    );
    assert!(editor.field("capabilities").is_none());
    assert_eq!(
        editor
            .field("fields")
            .and_then(Value::as_array)
            .and_then(|fields| fields.first()),
        Some(&json!({
            "name": "to",
            "label": "To",
            "type": "array",
            "format": "email_list",
            "value_binding": "actor:selection.draft.to",
            "required": true
        }))
    );
    assert_eq!(
        editor
            .field("fields")
            .and_then(Value::as_array)
            .and_then(|fields| fields.get(3)),
        Some(&json!({
            "name": "body",
            "label": "Body",
            "type": "string",
            "format": "markdown",
            "value_binding": "actor:selection.draft.body",
            "required": true
        }))
    );

    let change = find_capability(&space, "afmail", "draft_change");
    assert!(change.is_some());
    let Some(change) = change else {
        return;
    };
    assert_eq!(change.risk.as_str(), "local_mutation");
    assert_eq!(
        change
            .input_schema
            .as_ref()
            .and_then(|schema| schema["properties"]["to"]["type"].as_str()),
        Some("array")
    );
    assert_eq!(
        change
            .input_schema
            .as_ref()
            .and_then(|schema| schema["properties"]["to"]["format"].as_str()),
        Some("email_list")
    );
    assert_eq!(
        change
            .input_schema
            .as_ref()
            .and_then(|schema| schema["properties"]["body"]["format"].as_str()),
        Some("markdown")
    );

    let layouts = serde_json::from_value::<Vec<Layout>>(afmail["layouts"].clone());
    assert!(layouts.is_ok(), "{layouts:?}");
    let Ok(layouts) = layouts else {
        return;
    };
    assert_eq!(layouts[0].members, ["mail_queue"]);
    assert_eq!(layouts[1].members, ["draft_list", "draft_editor"]);
    assert_eq!(layouts[2].members, ["mailbox_glance"]);
    assert_eq!(layouts[3].members, ["mail", "drafts", "glance"]);

    let pack = read_json(&Path::new(env!("CARGO_MANIFEST_DIR")).join("afui/afmail.pack.json"));
    assert_eq!(pack["genre"], "triage");
    assert_eq!(pack["slots"]["mail_queue"], "queue");
    assert_eq!(pack["zoom"]["mail_queue"], "full");
    assert_eq!(pack["slots"]["draft_list"], "drafts");
    assert_eq!(pack["slots"]["draft_editor"], "editor");
    assert_eq!(pack["slots"]["mailbox_glance"], "glance");
    assert_eq!(
        pack["order"],
        json!(["draft_list", "draft_editor", "mail_queue", "mailbox_glance"])
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "ui")]
fn ui_open_reports_afui_fallback_when_host_binary_is_missing() {
    let root = temp_root("ui-missing-afui");
    assert!(fs::create_dir_all(&root).is_ok());
    let (status, stdout) = run(&root, &["init"]);
    assert_eq!(status, 0, "{stdout}");
    let missing = root.join("missing-afui").to_string_lossy().to_string();

    let (status, stdout) = run(&root, &["ui", "--afui-bin", missing.as_str(), "--no-open"]);
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "afui_not_found");
    assert!(
        value["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("afmail ui snapshot > mail.afui.json")
    );
    assert!(
        value["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("afui web serve mail.afui.json")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "ui")]
fn ui_owner_bridge_persists_draft_and_returns_fresh_snapshot() {
    let root = temp_root("ui-owner-bridge");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let (case_uid, _case_path, _) = create_case(
        &root,
        "ui-bridge-case",
        Some("support"),
        None,
        Some("exercise AFUI owner bridge"),
    );
    let (status, stdout) = run(
        &root,
        &[
            "case",
            "draft",
            "new",
            &case_uid,
            "--to",
            "old@example.com",
            "--cc",
            "old-cc@example.com",
            "--subject",
            "Old subject",
            "--body",
            "Old body",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let draft_name = parse_one(&stdout)["draft_name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!draft_name.is_empty());

    let command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let child = Command::new(bin())
        .current_dir(&root)
        .args(["ui", "bridge"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    assert!(child.is_ok());
    let Ok(mut child) = child else {
        return;
    };
    let stdin = child.stdin.take();
    assert!(stdin.is_some());
    let Some(mut stdin) = stdin else {
        let _ = child.kill();
        return;
    };
    let request = json!({
        "request_id": "request_ui_bridge_1",
        "author": "session_operator",
        "owner": "afmail",
        "capability": "draft_change",
        "risk": "local_mutation",
        "input": {
            "case_ref": case_uid.clone(),
            "draft_name": draft_name.clone(),
            "to": ["new@example.com"],
            "cc": [],
            "subject": "Updated through AFUI",
            "body": "Updated body.\n\nFormatting stays intact."
        }
    });
    assert!(writeln!(stdin, "{request}").is_ok());
    assert!(writeln!(stdin, "{}", json!({"type": "snapshot"})).is_ok());
    drop(stdin);
    let output = child.wait_with_output();
    assert!(output.is_ok());
    let Ok(output) = output else {
        return;
    };
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let responses = parse_raw_json_lines(&stdout);
    assert_eq!(responses.len(), 2, "{stdout}");
    assert_eq!(responses[0]["status"], "accepted");
    assert_eq!(responses[0]["capability"], "draft_change");
    assert_eq!(responses[1]["type"], "snapshot");
    assert_eq!(
        responses[1]["snapshot"]["space"]["afmail"]["drafts"]["items"][0]["subject"],
        "Updated through AFUI"
    );
    assert_eq!(
        responses[1]["snapshot"]["space"]["afmail"]["drafts"]["items"][0]["cc"],
        json!([])
    );

    drop(command_guard);
    let draft = parse_one(&run(&root, &["case", "draft", "show", &case_uid, &draft_name]).1);
    assert_eq!(draft["subject"], "Updated through AFUI");
    assert_eq!(draft["to"], json!(["new@example.com"]));
    assert_eq!(draft["cc"], json!([]));
    assert!(
        draft["body"]
            .as_str()
            .unwrap_or_default()
            .contains("Formatting stays intact")
    );
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
            "allow_unbounded_body_fetch": true,
            "username": imap_port.map(|_| "user"),
            "password_secret": imap_port.map(|_| "pass")
        },
        "mailboxes": {
            "inbox": {"resolve": [{"mailbox_name": "INBOX"}]},
            "sent": {"resolve": [{"special_use": "\\Sent"}, {"mailbox_name": "Sent"}, {"mailbox_name": "Sent Mail"}, {"mailbox_name": "Sent Messages"}]},
            "archive": {"resolve": [{"special_use": "\\Archive"}, {"mailbox_name": "Archive"}, {"mailbox_name": "Archives"}]},
            "junk": {"resolve": [{"special_use": "\\Junk"}, {"mailbox_name": "Junk"}, {"mailbox_name": "Spam"}, {"mailbox_name": "Junk Email"}, {"mailbox_name": "Junk E-mail"}]},
            "trash": {"resolve": [{"special_use": "\\Trash"}, {"mailbox_name": "Trash"}, {"mailbox_name": "Deleted Items"}, {"mailbox_name": "Deleted Messages"}, {"mailbox_name": "Bin"}]},
            "drafts": {"resolve": [{"special_use": "\\Drafts"}, {"mailbox_name": "Drafts"}, {"mailbox_name": "Draft"}]}
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
                {"append_to_mailbox_id": "drafts"},
                {"smtp_send": {}},
                {"append_to_mailbox_id": "sent"},
                {"add_flags": ["\\Seen", "\\Answered"], "on": "reply_to_message"}
            ]},
            "message.spam": {"steps": [
                {"add_flags": ["\\Seen", "$Junk"]},
                {"move_to_mailbox_id": "junk"}
            ]},
            "message.trash": {"steps": [{"move_to_mailbox_id": "trash"}]},
            "message.unspam": {"steps": [{"move_to_mailbox_id": "inbox"}]},
            "message.untrash": {"steps": [{"move_to_mailbox_id": "inbox"}]},
            "message.notification": {"by_source_mailbox_id": {
                "inbox": {"steps": [{"move_to_mailbox_id": "archive"}]},
                "sent": {"steps": []},
                "archive": {"steps": []},
                "junk": {"steps": []},
                "trash": {"steps": []},
                "drafts": {"steps": []}
            }}
        },
        "identities": [
            {"identity": "me", "name": "Me", "email": "me@example.com", "default": true}
        ],
        "case": {},
        "audit": {"reason_mode": "required"},
        "smtp": {
            "host": smtp_port.map(|_| "127.0.0.1"),
            "port": smtp_port.unwrap_or(587),
            "starttls": false,
            "tls_wrapper": false,
            "username": null,
            "password_secret": null
        },
        "workspace": {"language_bcp47": null, "timezone_utc_offset": "UTC"}
    })
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
    if let Some(status) = value
        .get("workspace")
        .and_then(|workspace| workspace.get("status"))
        .and_then(Value::as_str)
    {
        set_local_disposition(root, message_id, status, "2026-05-21T10:00:00Z");
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

fn local_disposition_spec(status: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match status {
        "spam" => Some(("spam", "spam.json", "spam")),
        "trashed" => Some(("trash", "trash.json", "trash")),
        "deleted_remote" => Some(("deleted", "deleted.json", "deleted_remote")),
        _ => None,
    }
}

fn set_local_disposition(root: &Path, message_id: &str, status: &str, added_rfc3339: &str) {
    let Some((dir, file, schema_name)) = local_disposition_spec(status) else {
        clear_local_dispositions(root, message_id);
        return;
    };
    clear_local_dispositions(root, message_id);
    let data_dir = root.join(dir).join("data");
    assert!(fs::create_dir_all(&data_dir).is_ok());
    let path = data_dir.join(file);
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut value: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
        json!({
            "schema_name": schema_name,
            "schema_version": 1,
            "collection_uid": dir,
            "collection_name": dir,
            "status": status,
            "items": []
        })
    });
    let items = value["items"].as_array_mut();
    if let Some(items) = items {
        items.retain(|item| item["message_id"] != message_id);
        items.push(json!({
            "message_id": message_id,
            "added_rfc3339": added_rfc3339
        }));
        value["message_count"] = json!(items.len());
    }
    value["updated_rfc3339"] = json!(added_rfc3339);
    write_json(&path, &value);
}

fn clear_local_dispositions(root: &Path, message_id: &str) {
    for (dir, file, _) in [
        ("spam", "spam.json", "spam"),
        ("trash", "trash.json", "trash"),
        ("deleted", "deleted.json", "deleted_remote"),
    ] {
        let path = root.join(dir).join("data").join(file);
        let text = fs::read_to_string(&path).unwrap_or_default();
        let mut value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if let Some(items) = value["items"].as_array_mut() {
            let before = items.len();
            items.retain(|item| item["message_id"] != message_id);
            if items.len() != before {
                value["message_count"] = json!(items.len());
                write_json(&path, &value);
            }
        }
    }
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
        if let Some(stored) = value["rfc822_message_id"].as_str()
            && trim(stored) == needle
        {
            return path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
        }
    }
    // Returns empty when not found; the caller's path assertion then fails clearly.
    String::new()
}

fn write_triage(root: &Path, message_id: &str, notes: &str) {
    let text = format!(
        "---\nkind: triage_view\nmessage_id: {message_id}\nmessage_ids:\n  - {message_id}\nmessage_count: 1\nattachment_count: 1\n---\n\n# Contract renewal\n\n<!-- afmail:conversation:start -->\n\n### {message_id} - 2026-05-21T10:00:00Z - Alice <alice@example.com>\n\n```text\nBody for {message_id}\n```\n\n<!-- afmail:conversation:end -->\n"
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
    args.push("--group".to_string());
    args.push(group.unwrap_or("support").to_string());
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

fn create_notification(
    root: &Path,
    name: &str,
    message_id: Option<&str>,
    summary: Option<&str>,
    reason: Option<&str>,
) -> (String, PathBuf, Value) {
    let mut args = vec![
        "notification".to_string(),
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
    let notification_uid = value["notification_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!notification_uid.is_empty(), "{value}");
    let path = value["path"].as_str().unwrap_or_default();
    assert!(!path.is_empty(), "{value}");
    (notification_uid, root.join(path), value)
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
            "schema_name": "case",
            "schema_version": 1,
            "collection_uid": case_uid,
            "collection_name": case_name,
            "tags": [],
            "created_rfc3339": "2026-05-21T10:00:00Z",
            "updated_rfc3339": "2026-05-21T10:00:00Z",
            "message_count": message_ids.len(),
            "thread_count": 0,
            "attachment_count": 0,
            "last_message_rfc3339": "2026-05-21T10:00:00Z",
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
    handle: thread::JoinHandle<()>,
}

#[derive(Clone, Copy)]
struct ImapServerCapabilities {
    move_supported: bool,
    uidplus_supported: bool,
}

fn start_imap_server(messages: Vec<(u64, Vec<u8>)>, accept_count: usize) -> Option<ImapTestServer> {
    start_imap_server_with_move(messages, accept_count, true)
}

fn start_imap_server_with_move(
    messages: Vec<(u64, Vec<u8>)>,
    accept_count: usize,
    move_supported: bool,
) -> Option<ImapTestServer> {
    start_imap_server_with_capabilities(
        messages,
        accept_count,
        ImapServerCapabilities {
            move_supported,
            uidplus_supported: false,
        },
    )
}

fn start_imap_server_with_capabilities(
    messages: Vec<(u64, Vec<u8>)>,
    accept_count: usize,
    capabilities: ImapServerCapabilities,
) -> Option<ImapTestServer> {
    start_imap_server_with_messages(
        TestImapMessages::Shared(messages),
        accept_count,
        capabilities,
    )
}

#[cfg(feature = "ui")]
fn start_imap_mailbox_server(
    messages: BTreeMap<String, Vec<(u64, Vec<u8>)>>,
    accept_count: usize,
) -> Option<ImapTestServer> {
    start_imap_server_with_messages(
        TestImapMessages::ByMailbox(messages),
        accept_count,
        ImapServerCapabilities {
            move_supported: true,
            uidplus_supported: false,
        },
    )
}

fn start_imap_server_with_messages(
    messages: TestImapMessages,
    accept_count: usize,
    capabilities: ImapServerCapabilities,
) -> Option<ImapTestServer> {
    let listener = bind_loopback_listener()?;
    let addr = listener.local_addr().ok()?;
    let commands = Arc::new(Mutex::new(Vec::new()));
    let appended = Arc::new(Mutex::new(Vec::new()));
    let created = Arc::new(Mutex::new(Vec::new()));
    let moved = Arc::new(Mutex::new(Vec::new()));
    let server_commands = Arc::clone(&commands);
    let server_appended = Arc::clone(&appended);
    let server_created = Arc::clone(&created);
    let server_moved = Arc::clone(&moved);
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
                        capabilities,
                    );
                    served += 1;
                    last = Instant::now();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let idle_limit = if served == 0 {
                        Duration::from_secs(5)
                    } else {
                        Duration::from_millis(800)
                    };
                    if last.elapsed() > idle_limit {
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
        handle,
    })
}

#[allow(clippy::too_many_arguments)]
fn serve_imap_connection(
    mut writer: TcpStream,
    messages: &TestImapMessages,
    commands: &ImapCommands,
    appended: &AppendedMessages,
    created: &CreatedFolders,
    moved: &MovedMessages,
    capabilities: ImapServerCapabilities,
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
                let mut caps = vec!["IMAP4rev1"];
                if capabilities.move_supported {
                    caps.push("MOVE");
                }
                if capabilities.uidplus_supported {
                    caps.push("UIDPLUS");
                }
                let caps = caps.join(" ");
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
                if let Some(folder) = folder_arg_from_line(&line, "CREATE")
                    && let Ok(mut locked) = created.lock()
                {
                    locked.push(folder);
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
                let exists = messages.for_mailbox(&selected_mailbox).len();
                let response = format!(
                    "* FLAGS ()\r\n* {exists} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY 44] UIDs valid\r\n* OK [UIDNEXT 100] Next UID\r\n{tag} OK [READ-WRITE] Select completed\r\n"
                );
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "FETCH" if is_uid_fetch => {
                if write_imap_fetch(
                    &mut writer,
                    &tag,
                    messages.for_mailbox(&selected_mailbox),
                    &line,
                )
                .is_err()
                {
                    return;
                }
            }
            "SEARCH" => {
                let text = String::from_utf8_lossy(&line);
                let upper = text.to_ascii_uppercase();
                let uids =
                    if upper.contains(" UID SEARCH ALL") || upper.contains(" UID SEARCH SINCE ") {
                        messages
                            .for_mailbox(&selected_mailbox)
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
                let response = format!("{tag} OK STORE completed\r\n");
                if write_imap(&mut writer, response.as_bytes()).is_err() {
                    return;
                }
            }
            "COPY" => {
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
    if query.contains("BODY.PEEK[]<")
        && messages.iter().any(|(_, raw)| {
            String::from_utf8_lossy(raw).contains("X-Afmail-Test-Reject-Partial: true")
        })
    {
        writer.write_all(b"* BYE partial body fetch unsupported\r\n")?;
        writer.flush()?;
        return Ok(());
    }
    let includes_body = query.contains("BODY.PEEK[");
    let header_only = query.contains("BODY.PEEK[HEADER]");
    let header_fields_only = query.contains("BODY.PEEK[HEADER.FIELDS");
    let matching = messages
        .iter()
        .filter(|(uid, _)| uid_fetch_query_matches_uid(&query, *uid))
        .collect::<Vec<_>>();
    for (idx, (uid, raw)) in matching.iter().enumerate() {
        let seq = idx + 1;
        let body = if header_only || header_fields_only {
            raw_message_header(raw)
        } else {
            raw.as_slice()
        };
        let len = body.len();
        let raw_len = test_declared_size_from_raw(raw).unwrap_or(raw.len());
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
        if includes_body {
            write!(writer, " {body_key} {{{len}}}\r\n")?;
            writer.write_all(body)?;
        }
        writer.write_all(b")\r\n")?;
    }
    write!(writer, "{tag} OK FETCH completed\r\n")?;
    writer.flush()
}

fn uid_fetch_query_matches_uid(query: &str, uid: u64) -> bool {
    let Some((_, after_fetch)) = query.split_once(" UID FETCH ") else {
        return true;
    };
    let Some(sequence) = after_fetch.split_whitespace().next() else {
        return true;
    };
    sequence.split(',').any(|part| {
        if let Some((start, end)) = part.split_once(':') {
            let start = start.parse::<u64>().unwrap_or(1);
            if end == "*" {
                uid >= start
            } else {
                end.parse::<u64>()
                    .map(|end| uid >= start && uid <= end)
                    .unwrap_or(false)
            }
        } else {
            part.parse::<u64>()
                .map(|value| value == uid)
                .unwrap_or(false)
        }
    })
}

fn test_declared_size_from_raw(raw: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(raw);
    text.lines().find_map(|line| {
        line.strip_prefix("X-Afmail-Test-Declared-Size:")
            .and_then(|value| value.trim().parse().ok())
    })
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
    start_smtp_server_connections(1)
}

fn start_smtp_server_connections(connection_count: usize) -> Option<SmtpTestServer> {
    let listener = bind_loopback_listener()?;
    let addr = listener.local_addr().ok()?;
    let (data_tx, data_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        for _ in 0..connection_count {
            let accepted = listener.accept();
            let Ok((stream, _)) = accepted else {
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
    assert_eq!(value["code"], "cli_error");
    let (status, stdout) = run(&root, &["message", "--help"]);
    assert_eq!(status, 0);
    assert!(!stdout.contains("ignore"));
    assert!(stdout.contains("list"));
    let (status, stdout) = run(&root, &["message", "spam", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("<MESSAGE_ID>..."));
    let (status, stdout) = run(&root, &["message", "list", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("--sender"));
    assert!(stdout.contains("--subject-contains"));
    assert!(stdout.contains("--since"));
    assert!(stdout.contains("--in <SCOPE>"));
    let (status, stdout) = run(&root, &["case", "add", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("<MESSAGE_ID>..."));
    let (status, stdout) = run(&root, &["notification", "add", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("<MESSAGE_ID>..."));
    let (status, stdout) = run(&root, &["case", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("show"));
    assert!(stdout.contains("add"));
    assert!(stdout.contains("set-summary"));
    assert!(stdout.contains("draft"));
    assert!(stdout.contains("archive"));
    assert!(stdout.contains("unarchive"));
    assert!(stdout.contains("trash"));
    assert!(stdout.contains("draft"));
    assert!(stdout.contains("notes"));
    assert!(stdout.contains("merge"));
    assert!(!stdout.contains("compose"));
    let (status, stdout) = run(&root, &["case", "draft", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("reply"));
    assert!(stdout.contains("change"));
    assert!(stdout.contains("list"));
    assert!(stdout.contains("upload"));
    assert!(stdout.contains("send"));
    let (status, stdout) = run(&root, &["case", "add", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("<CASE_REF> <MESSAGE_ID>"));
    assert!(stdout.contains("--reason"));
    let (status, stdout) = run(&root, &["notification", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("create"));
    assert!(stdout.contains("show"));
    assert!(stdout.contains("remove"));
    assert!(stdout.contains("move"));
    assert!(stdout.contains("rename"));
    assert!(stdout.contains("set-summary"));
    assert!(stdout.contains("notes"));
    let (status, stdout) = run(&root, &["case", "unarchive", "--help"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("--group"));
    assert!(stdout.contains("--reason"));
    let (status, stdout) = run(&root, &["archive", "case", "show", "c20260603001"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    let (status, stdout) = run(&root, &["message", "ignore", "message_001"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    let (status, stdout) = run(&root, &["message", "message_001", "show"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    let (status, stdout) = run(&root, &["case", "c20260603001", "add", "message_001"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    let (status, stdout) = run(&root, &["--help", "--output", "markdown"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("# Agent-First Mail"));
    assert!(stdout.contains("afmail case show c20260603001"));
    assert!(stdout.contains("afmail case show REF"));
    assert!(stdout.contains("afmail notification show n20260603001"));
    assert!(!stdout.contains("afmail message ignore"));
    assert!(!stdout.contains("--mode"));
    let (status, stdout) = run(&root, &["--mode", "pipe"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    assert!(!stdout.to_ascii_lowercase().contains("bucket"));

    let (status, stdout) = run(&root, &["init"]);
    assert_eq!(status, 0);
    let value = parse_one(&stdout);
    let first_workspace_uid = value["workspace_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(first_workspace_uid.len(), "workspace_".len() + 32);
    assert!(first_workspace_uid.starts_with("workspace_"));
    let first_identity_bytes = fs::read(root.join(".afmail/workspace.json")).unwrap_or_default();
    assert!(value.get("agent_skill_created").is_none());
    assert_eq!(value["gitignore_created"], true);
    assert_eq!(value["gitignore_updated"], false);
    assert_eq!(value["gitignore_path"], ".gitignore");
    assert_eq!(value["do_not_edit_created"], true);
    assert_eq!(value["do_not_edit_path"], ".afmail/DO_NOT_EDIT.txt");
    assert!(root.join(".afmail/transactions").is_dir());
    assert!(root.join(".afmail/pull").is_dir());
    assert!(root.join(".afmail/cache").is_dir());
    assert!(!root.join(".afmail/config.example.json").exists());
    assert!(!root.join("spam").exists());
    assert!(!root.join("spam/data/spam.json").exists());
    assert!(!root.join("trash").exists());
    assert!(!root.join("trash/data/trash.json").exists());
    assert!(!root.join("deleted").exists());
    assert!(!root.join("deleted/data/deleted.json").exists());
    assert!(!root.join("buckets").exists());
    assert!(root.join("templates").is_dir());
    assert!(!root.join(".codex").exists());
    assert!(!root.join("AGENTS.md").exists());
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    assert!(gitignore.contains("# BEGIN afmail managed"));
    assert!(!gitignore.contains(".afmail/messages/"));
    assert!(gitignore.contains(".afmail/push/"));
    assert!(!gitignore.contains(".codex/skills/agent-first-mail/"));
    assert!(!gitignore.contains(".agents/skills/agent-first-mail/"));
    assert!(!gitignore.contains(".claude/skills/agent-first-mail/"));
    assert!(gitignore.contains(".afmail/cache/"));
    assert!(!gitignore.contains(".afmail/cache/progress.json"));
    assert!(gitignore.contains("messages/*.json"));
    assert!(gitignore.contains("triage/*.md"));
    assert!(gitignore.contains("spam/*.md"));
    assert!(gitignore.contains("trash/*.md"));
    assert!(gitignore.contains("deleted/*.md"));
    let (status, stdout) = run(
        &root,
        &[
            "skill",
            "install",
            "--agent",
            "claude-code",
            "--scope",
            "workspace",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert!(
        value["targets"][0]["skill_path"]
            .as_str()
            .unwrap_or_default()
            .ends_with(".claude/skills/agent-first-mail/SKILL.md")
    );
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    assert!(!gitignore.contains(".codex/skills/agent-first-mail/"));
    assert!(gitignore.contains(".claude/skills/agent-first-mail/"));
    assert!(gitignore.contains("messages/*.json"));
    assert_eq!(gitignore.matches("# BEGIN afmail managed").count(), 1);
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["workspace_uid"], first_workspace_uid);
    assert!(value["workspace_path"].as_str().is_some(), "{stdout}");
    assert_eq!(value["progress"]["status"], "idle");
    assert_eq!(value["readiness"]["ready_for_pull"], false);
    assert_eq!(value["readiness"]["ready_for_send"], false);
    assert_eq!(value["readiness"]["placeholder_identity"], true);
    let do_not_edit = root.join(".afmail/DO_NOT_EDIT.txt");
    assert!(do_not_edit.is_file());
    assert!(fs::write(&do_not_edit, "custom warning").is_ok());
    assert!(fs::write(root.join("AGENTS.md"), "custom agents file").is_ok());
    assert!(fs::write(root.join(".gitignore"), "*.tmp\n").is_ok());
    let (status, stdout) = run(&root, &["init"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["workspace_uid"], first_workspace_uid);
    assert_eq!(
        fs::read(root.join(".afmail/workspace.json")).unwrap_or_default(),
        first_identity_bytes
    );
    assert!(value.get("agent_skill_created").is_none());
    assert_eq!(value["gitignore_created"], false);
    assert_eq!(value["gitignore_updated"], true);
    assert_eq!(value["do_not_edit_created"], false);
    let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap_or_default();
    assert!(agents.contains("custom agents file"));
    assert_eq!(agents, "custom agents file");
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    assert!(gitignore.contains("*.tmp"));
    assert!(gitignore.contains("# BEGIN afmail managed"));
    assert!(gitignore.contains("messages/*.json"));
    assert!(gitignore.contains("notifications/*/views/**/*.md"));
    assert_ne!(
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
            "support",
        ],
    );
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    let config = fs::read_to_string(root.join(".afmail/config.json")).unwrap_or_default();
    let config_json: Value = serde_json::from_str(&config).unwrap_or(Value::Null);
    assert_eq!(config_json, json!({}));
    assert!(config_json.get("code").is_none());
    assert!(config_json.get("special_use").is_none());
    assert!(config_json.get("imap_mailboxes").is_none());
    assert!(config_json.get("pull").is_none());
    assert!(config_json.get("push").is_none());
    assert!(!config.contains("\"folders\": {"));
    assert!(!config.contains("imap_host"));
    let (status, stdout) = run(&root, &["config", "pull", "add", "Sent"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
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
    assert!(!config.contains("\"actions\""));
    // `config show` dumps the whole config; it must redact inline secrets the
    // same way `config get`/`set` do, or an agent surfaces the live password.
    let (status, stdout) = run(&root, &["config", "show"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(
        !stdout.contains("super-secret"),
        "config show leaked the inline secret: {stdout}"
    );
    assert_eq!(
        parse_one(&stdout)["config"]["imap"]["password_secret"],
        "***"
    );
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
        &[
            "config",
            "set",
            "mailboxes.archive.resolve",
            r#"[{"mailbox_name":"Archive"}]"#,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(
        parse_one(&stdout)["value"],
        json!([{"mailbox_name": "Archive"}])
    );
    let (status, stdout) = run(&root, &["config", "get", "audit.reason_mode"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "required");
    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "optional"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "optional");
    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "required"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "required");
    let config = fs::read_to_string(root.join(".afmail/config.json")).unwrap_or_default();
    assert!(
        !config.contains("\"reason_mode\""),
        "default audit mode should not be persisted: {config}"
    );

    let (status, stdout) = run(&root, &["--output", "plain", "status"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("kind=result"));
    assert!(stdout.contains("triage_count=0"));

    let (status, stdout) = run(&root, &["--log", "startup,request,progress", "status"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    assert_eq!(lines.len(), 4, "{stdout}");
    assert_eq!(lines[0]["event"], "startup");
    assert_eq!(lines[0]["command"], "status");
    assert_eq!(lines[1]["event"], "request");
    assert_eq!(lines[2]["event"], "progress");
    assert_eq!(lines[2]["success"], true);
    for line in &lines[0..2] {
        assert_eq!(line["level"], "info");
        assert!(line["event"].as_str().is_some(), "{stdout}");
        assert!(line["timestamp_epoch_ms"].as_u64().is_some(), "{stdout}");
        assert!(line["message"].as_str().is_some(), "{stdout}");
    }
    assert_eq!(lines[2]["level"], "info");
    assert!(lines[2]["message"].as_str().is_some(), "{stdout}");
    assert!(lines[3].get("outcome").is_none());

    let (status, stdout) = run(&root, &["--verbose", "status"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    assert!(
        lines
            .last()
            .is_some_and(|value| value.get("outcome").is_none())
    );

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

    let (status, stdout) = run(
        &root,
        &[
            "--log",
            "startup",
            "case",
            "draft",
            "new",
            "case_private",
            "--to",
            "alice.private@example.com",
            "--cc=finance.private@example.com",
            "--subject",
            "Private payroll thread",
            "--body=Please review the private numbers.",
        ],
    );
    assert_ne!(status, 2, "{stdout}");
    assert!(!stdout.contains("alice.private@example.com"), "{stdout}");
    assert!(!stdout.contains("finance.private@example.com"), "{stdout}");
    assert!(!stdout.contains("Private payroll thread"), "{stdout}");
    assert!(
        !stdout.contains("Please review the private numbers."),
        "{stdout}"
    );
    let argv = parse_lines(&stdout)[0]["argv"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(argv.iter().any(|arg| arg == "--to"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "***"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "--cc=***"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "--body=***"), "{stdout}");
    let shape = &parse_lines(&stdout)[0]["argv_shape"];
    assert_eq!(shape["command_tokens"], json!(["case", "draft", "new"]));
    assert_eq!(
        shape["flag_names"],
        json!(["--body", "--cc", "--log", "--subject", "--to"])
    );
    assert_eq!(shape["positional_count"], 1);
    assert_eq!(shape["redacted_value_count"], 6);
    assert_eq!(shape["executable"], "afmail");
    assert!(
        !shape.to_string().contains("alice.private@example.com"),
        "{stdout}"
    );

    let (status, stdout) = run(
        &root,
        &[
            "--log",
            "startup",
            "notification",
            "create",
            "--name",
            "Private vendor notice",
            "--message",
            "message_missing",
            "--summary",
            "Private subscription summary",
            "--reason",
            "Private filing reason",
        ],
    );
    assert_ne!(status, 2, "{stdout}");
    assert!(!stdout.contains("Private vendor notice"), "{stdout}");
    assert!(!stdout.contains("Private subscription summary"), "{stdout}");
    assert!(!stdout.contains("Private filing reason"), "{stdout}");
    let argv = parse_lines(&stdout)[0]["argv"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(argv.iter().any(|arg| arg == "--name"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "--summary"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "--reason"), "{stdout}");
    assert!(argv.iter().filter(|arg| *arg == "***").count() >= 3);

    let (status, stdout) = run(
        &root,
        &[
            "--log",
            "startup",
            "case",
            "notes",
            "append",
            "c20260711001-private-customer",
            "--text",
            "Private note body with customer context",
        ],
    );
    assert_ne!(status, 2, "{stdout}");
    assert!(
        !stdout.contains("c20260711001-private-customer"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("Private note body with customer context"),
        "{stdout}"
    );
    let argv = parse_lines(&stdout)[0]["argv"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(argv.iter().any(|arg| arg == "case"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "notes"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "append"), "{stdout}");
    assert!(argv.iter().any(|arg| arg == "--text"), "{stdout}");
    assert!(argv.iter().filter(|arg| *arg == "***").count() >= 2);
    let shape = &parse_lines(&stdout)[0]["argv_shape"];
    assert_eq!(shape["command_tokens"], json!(["case", "notes", "append"]));
    assert_eq!(shape["flag_names"], json!(["--log", "--text"]));
    assert_eq!(shape["positional_count"], 1);
    assert_eq!(shape["redacted_value_count"], 3);
    assert!(
        !shape.to_string().contains("c20260711001-private-customer"),
        "{stdout}"
    );

    let (status, stdout) = run(&root, &["--log", "bogus", "status"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");

    let (status, stdout) = run(&root, &["--log", "redirect", "status"]);
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");
    let error = value["message"].as_str().unwrap_or_default();
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
    assert_eq!(value["code"], "cli_error");
    let (status, stdout) = run(&root, &["--output", "yaml", "nope"]);
    assert_eq!(status, 2);
    assert!(stdout.starts_with("---\n"), "{stdout}");
    assert!(stdout.contains("kind: \"error\""), "{stdout}");
    assert!(stdout.contains("code: \"cli_error\""), "{stdout}");
    assert!(stdout.contains("duration:"), "{stdout}");
    assert!(!stdout.contains("error_code:"), "{stdout}");
    let (status, stdout) = run(&root, &["--output=plain", "nope"]);
    assert_eq!(status, 2);
    assert!(stdout.contains("kind=error"), "{stdout}");
    assert!(stdout.contains("error.code=cli_error"), "{stdout}");
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
        &root.join(".afmail/cache/progress.json"),
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
    assert_eq!(value["progress"]["status"], "stale");
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

    write_json(
        &root.join(".afmail/cache/progress.json"),
        &json!({
            "schema_name": "workspace_progress",
            "schema_version": 1,
            "command": "pull",
            "status": "running",
            "phase": "pull_render_triage_done",
            "started_rfc3339": "2026-06-11T00:00:00Z",
            "updated_rfc3339": "2026-06-11T00:00:02Z",
            "elapsed_ms": 2000,
            "fields": {
                "render_stage": "triage",
                "elapsed_ms": 1500,
                "triage_count": 3,
                "triage_written_count": 2,
                "triage_unchanged_count": 1
            },
            "result": null,
            "error": null
        }),
    );
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["progress"]["status"], "stale");
    assert_eq!(value["progress"]["render_stage"], "triage");
    assert_eq!(value["progress"]["elapsed_ms"], 1500);
    assert_eq!(value["progress"]["triage_count"], 3);
    assert_eq!(value["progress"]["triage_written_count"], 2);
    assert_eq!(value["progress"]["triage_unchanged_count"], 1);
    assert!(
        value["progress"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("rendered triage views")),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn doctor_reports_and_repairs_afmail_state_only() {
    let root = temp_root("doctor");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_doctor", 1, None);
    write_json(
        &root.join(".afmail/messages/message_doctor.state.json"),
        &json!({
            "schema_name": "message_state",
            "schema_version": 1,
            "message_id": "message_doctor",
            "status": "triage",
            "updated_rfc3339": "2026-05-21T10:00:00Z"
        }),
    );

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 3, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "partial");
    assert_eq!(value["error_count"], 0);
    assert_eq!(value["warning_count"], 1);
    assert_eq!(value["checks"]["git_checked"], false);
    assert!(value.get("ok").is_none());
    assert!(
        value["issues"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|issue| issue["code"] == "message_state_deprecated")
    );

    let (status, yaml) = run(&root, &["--output", "yaml", "doctor"]);
    assert_eq!(status, 3, "{yaml}");
    assert!(yaml.contains("kind: \"error\""), "{yaml}");
    let (status, plain) = run(&root, &["--output", "plain", "doctor"]);
    assert_eq!(status, 3, "{plain}");
    assert!(plain.contains("kind=error"), "{plain}");

    let (status, stdout) = run(&root, &["doctor", "repair"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "confirm_required");

    let (status, stdout) = run(&root, &["doctor", "repair", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let repaired = parse_one(&stdout);
    assert_eq!(repaired["deprecated_state_removed_count"], 1);
    assert!(
        !root
            .join(".afmail/messages/message_doctor.state.json")
            .is_file()
    );
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
            "kind": "message_notification",
            "created_rfc3339": "2026-06-09T00:00:00Z",
            "paths": ["messages/message_1.json"]
        }),
    );

    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "optional"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "transaction_incomplete");

    let (status, stdout) = run(&root, &["doctor"]);
    assert_eq!(status, 1, "{stdout}");
    let doctor = parse_one(&stdout);
    assert_eq!(doctor["code"], "command_failed");
    assert!(
        doctor["issues"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|issue| issue["code"] == "transaction_incomplete")
    );

    let (status, yaml) = run(&root, &["--output", "yaml", "doctor"]);
    assert_eq!(status, 1, "{yaml}");
    assert!(yaml.contains("kind: \"error\""), "{yaml}");
    let (status, plain) = run(&root, &["--output", "plain", "doctor"]);
    assert_eq!(status, 1, "{plain}");
    assert!(plain.contains("kind=error"), "{plain}");
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
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "reason_required");
    assert_eq!(
        value["items"][0]["error"]["hint"],
        "Repeat the command with `--reason TEXT`."
    );
    assert_eq!(value["items"][0]["error"]["required_option"], "--reason");
    assert_eq!(
        value["items"][0]["error"]["suggested_args"],
        json!(["--reason", "TEXT"])
    );
    assert!(
        !stdout.contains("audit.reason_mode"),
        "reason_required should not suggest lowering audit policy: {stdout}"
    );
    assert!(root.join("triage/message_optional.md").exists());
    assert!(
        !root
            .join(".afmail/messages/message_optional.notes.md")
            .exists()
    );

    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "optional"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "optional");
    let (status, stdout) = run(&root, &["message", "trash", "message_optional"]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    assert!(
        !root
            .join(".afmail/messages/message_optional.notes.md")
            .exists()
    );

    let (status, stdout) = run(&root, &["config", "set", "audit.reason_mode", "required"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["value"], "required");
    write_message(&root, "message_case", 2, None);
    write_triage(&root, "message_case", "case");
    let (case_uid, case_path, _) =
        create_case(&root, "case-reason", None, None, Some("setup case"));
    let (status, stdout) = run(&root, &["case", "add", &case_uid, "message_case"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");
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
    let _ = parse_one(&stdout);
    let notes = fs::read_to_string(case_path.join("notes.md")).unwrap_or_default();
    assert_eq!(notes, "# Notes\n\n");
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"case_message_added\""));
    assert!(log.contains("\"reason\":\"belongs to the case\""));

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "set-summary",
            &case_uid,
            "message_case",
            "--summary",
            "updated case summary",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "set-summary",
            &case_uid,
            "message_case",
            "--summary",
            "updated case summary",
            "--reason",
            "better case summary",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);
    let case_json = fs::read_to_string(case_path.join("data/case.json")).unwrap_or_default();
    assert!(case_json.contains("\"summary\": \"updated case summary\""));

    let (status, stdout) = run(&root, &["case", "move", &case_uid, "--group", "waiting"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");
    assert!(case_path.join("case.md").exists());

    write_message(&root, "message_notification_reason", 3, None);
    write_triage(&root, "message_notification_reason", "archive");
    let (_notification_uid, archive_path, _value) = create_notification(
        &root,
        "receipts",
        Some("message_notification_reason"),
        Some("receipt notification"),
        Some("receipt notification"),
    );
    assert!(
        !root
            .join(".afmail/messages/message_notification_reason.notes.md")
            .exists()
    );
    assert!(
        archive_path
            .join("views/messages/message_notification_reason.md")
            .exists()
    );
    let log = fs::read_to_string(root.join(".afmail/logs/events.jsonl")).unwrap_or_default();
    assert!(log.contains("\"kind\":\"notification_created\""));
    assert!(log.contains("\"reason\":\"receipt notification\""));

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Audit Contact",
            "--email",
            "audit@example.com",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "create",
            "--name",
            "Audit Contact",
            "--email",
            "audit@example.com",
            "--reason",
            "seed contact",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let contact_uid = parse_one(&stdout)["contact_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, stdout) = run(&root, &["contact", "move", &contact_uid, "--group", "team"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "rename",
            &contact_uid,
            "--name",
            "Audit Contact 2",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

    let (status, stdout) = run(
        &root,
        &[
            "contact",
            "email",
            "add",
            &contact_uid,
            "audit2@example.com",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

    let (status, stdout) = run(
        &root,
        &["contact", "phone", "add", &contact_uid, "+1-555-0100"],
    );
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

    let (status, stdout) = run(&root, &["contact", "tag", &contact_uid, "ops"]);
    assert_eq!(status, 1, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "reason_required");

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
    assert_eq!(value["new_message_count"], 1);
    let progress = read_json(&root.join(".afmail/cache/progress.json"));
    assert_eq!(progress["schema_name"], "workspace_progress");
    assert_eq!(progress["command"], "pull");
    assert_eq!(progress["status"], "succeeded");
    assert_eq!(progress["result"]["new_message_count"], 1);
    let id = message_id_for_rfc822(&root, "imap-7@example.com");
    assert_eq!(value["new_message_ids"], json!([id.clone()]));
    assert_eq!(value["triage_created_message_ids"], json!([id.clone()]));
    assert!(root.join(format!(".afmail/messages/{id}.eml")).is_file());
    assert!(!root.join(format!(".afmail/messages/{id}.txt")).exists());
    assert!(root.join(format!("messages/{id}.json")).is_file());
    assert!(root.join(format!("triage/{id}.md")).is_file());
    let message_json = fs::read_to_string(root.join(format!("messages/{id}.json")));
    let message_value: Value =
        serde_json::from_str(&message_json.unwrap_or_default()).unwrap_or(Value::Null);
    assert!(
        message_value["body_text"]
            .as_str()
            .is_some_and(|text| text.contains("Hello from IMAP."))
    );
    let render_cache = read_json(&root.join(".afmail/cache/render-cache.json"));
    assert_eq!(render_cache["schema_name"], "render_cache");
    assert!(
        render_cache["entries"][format!("triage/{id}.md")]["input_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("blake3:"))
    );
    let cursor = read_json(&root.join(".afmail/pull/cursors.json"));
    assert_eq!(cursor["schema_name"], "pull_cursors");
    assert_eq!(cursor["mailboxes"]["inbox"]["mailbox_name"], "INBOX");
    assert_eq!(cursor["mailboxes"]["inbox"]["uid_validity"], 44);
    assert_eq!(cursor["mailboxes"]["inbox"]["highest_uid"], 7);
    assert_eq!(
        cursor["mailboxes"]["inbox"]["pull_count_since_full_reconcile"],
        0
    );

    let (status, stdout) = run(&root, &["pull", "inbox"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["new_message_count"], 0);
    assert_eq!(value["triage_created_count"], 0);
    assert_eq!(value["triage_written_count"], 0);
    assert_eq!(value["triage_unchanged_count"], 1);
    assert_eq!(value["checked_location_count"], 0);
    let cursor = read_json(&root.join(".afmail/pull/cursors.json"));
    assert_eq!(cursor["mailboxes"]["inbox"]["highest_uid"], 7);
    assert_eq!(
        cursor["mailboxes"]["inbox"]["pull_count_since_full_reconcile"],
        1
    );
    assert!(server.handle.join().is_ok());
    let commands = server.commands.lock();
    assert!(commands.is_ok());
    let commands = commands.map(|items| items.clone()).unwrap_or_default();
    assert!(commands.iter().any(|cmd| cmd == "LOGIN"));
    assert!(commands.iter().any(|cmd| cmd == "EXAMINE"));
    assert!(commands.iter().any(|cmd| cmd == "FETCH"));
    assert_eq!(
        commands
            .iter()
            .filter(|cmd| cmd.as_str() == "FETCH")
            .count(),
        5
    );
    assert!(!commands.iter().any(|cmd| cmd == "STORE"));
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
    let phases = lines
        .iter()
        .filter(|line| line["event"] == "progress")
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
        "pull_render_triage_start",
        "pull_render_triage_progress",
        "pull_render_triage_done",
        "pull_render_dispositions_start",
        "pull_render_dispositions_done",
        "pull_render_cases_start",
        "pull_render_cases_done",
        "pull_render_notifications_start",
        "pull_render_notifications_done",
        "pull_render_cache_write_start",
        "pull_render_cache_write_done",
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
    let render_triage_done = lines
        .iter()
        .find(|line| line["phase"] == "pull_render_triage_done")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(render_triage_done["render_stage"], "triage");
    assert_eq!(render_triage_done["triage_count"], 1);
    assert!(render_triage_done["elapsed_ms"].as_u64().is_some());
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
    // Header fetch + two metadata batches + one bounded partial body fetch per
    // message + reconciliation UID snapshot.
    assert_eq!(fetch_count, 30);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_applies_one_global_new_message_budget() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-global-budget");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert_eq!(
        run(
            &root,
            &["pull", "--since-days", "30", "--all-history", "inbox"]
        )
        .0,
        2
    );
    let messages = (1..=3)
        .map(|uid| {
            (
                uid,
                format!(
                    "Message-ID: <budget-{uid}@example.com>\r\nFrom: a@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Budget {uid}\r\n\r\nBody {uid}\r\n"
                )
                .into_bytes(),
            )
        })
        .collect::<Vec<_>>();
    let server = match start_imap_server(messages.clone(), 3) {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );
    let (status, stdout) = run(
        &root,
        &[
            "pull",
            "--all-history",
            "--max-new-messages",
            "2",
            "--max-total-bytes",
            "1000000",
            "inbox",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["new_message_count"], 2, "{result}");
    assert_eq!(result["budget_limited"], true);
    assert_eq!(result["budget_skipped_count"], 1);
    assert_eq!(result["remaining_message_budget"], 0);
    assert_eq!(result["all_history"], true);
    assert!(result["since_days"].is_null());
    assert_eq!(
        fs::read_dir(root.join("messages"))
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0),
        2
    );
    assert!(server.handle.join().is_ok());
    let continuation_server = match start_imap_server(messages, 3) {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(continuation_server.addr.port()), None),
    );
    let (status, stdout) = run(
        &root,
        &[
            "pull",
            "--all-history",
            "--max-new-messages",
            "2",
            "--max-total-bytes",
            "1000000",
            "inbox",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["new_message_count"], 1);
    assert_eq!(
        fs::read_dir(root.join("messages"))
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0),
        3
    );
    assert!(continuation_server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_dry_run_estimates_without_workspace_changes() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-dry-run-pure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = b"Message-ID: <dry-run@example.com>\r\nFrom: a@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Dry run\r\n\r\nBody\r\n".to_vec();
    let server = match start_imap_server(vec![(1, raw)], 3) {
        Some(server) => server,
        None => return,
    };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(server.addr.port()), None),
    );
    let before = workspace_tree_snapshot(&root);
    let (status, stdout) = run(
        &root,
        &[
            "pull",
            "--dry-run",
            "--all-history",
            "--max-new-messages",
            "10",
            "inbox",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["remote_read"], true);
    assert_eq!(result["local_changed"], false);
    assert_eq!(result["candidate_count"], 1);
    let after = workspace_tree_snapshot(&root);
    assert!(
        workspace_snapshot_differences(&before, &after).is_empty(),
        "dry-run changed workspace"
    );
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_rejects_unbounded_server_before_local_changes_by_default() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-partial-unsupported");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let raw = b"Message-ID: <partial-unsupported@example.com>\r\nX-Afmail-Test-Reject-Partial: true\r\nFrom: a@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Unsupported\r\n\r\nBody\r\n".to_vec();
    let server = match start_imap_server(vec![(1, raw)], 3) {
        Some(server) => server,
        None => return,
    };
    let mut config = test_config(Some(server.addr.port()), None);
    config["imap"]["allow_unbounded_body_fetch"] = json!(false);
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run(&root, &["pull", "--all-history", "inbox"]);
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "imap_partial_fetch_unsupported");
    assert_eq!(error["message_writes_started"], false);
    assert_eq!(
        fs::read_dir(root.join("messages"))
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0),
        0
    );
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_skips_declared_oversized_message_and_continues() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-oversized-skip");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let oversized = b"Message-ID: <oversized@example.com>\r\nX-Afmail-Test-Declared-Size: 104857601\r\nFrom: a@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Oversized\r\n\r\nSmall fixture body\r\n".to_vec();
    let normal = b"Message-ID: <normal-after-oversized@example.com>\r\nFrom: b@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Normal\r\n\r\nNormal body\r\n".to_vec();
    let server = match start_imap_server(vec![(1, oversized), (2, normal)], 3) {
        Some(server) => server,
        None => return,
    };
    let mut config = test_config(Some(server.addr.port()), None);
    config["imap"]["allow_unbounded_body_fetch"] = json!(false);
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run(
        &root,
        &[
            "pull",
            "--all-history",
            "--max-total-bytes",
            "209715200",
            "inbox",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["new_message_count"], 1, "{result}");
    assert_eq!(result["skipped_message_count"], 1);
    assert_eq!(
        result["skipped_messages"][0]["reason"],
        "declared_size_exceeded"
    );
    assert_eq!(result["skipped_messages"][0]["uid"], 1);
    assert!(
        root.join("messages/message_normal-after-oversized@example.com.json")
            .exists()
            || fs::read_dir(root.join("messages"))
                .map(|entries| entries.filter_map(Result::ok).count() == 1)
                .unwrap_or(false)
    );
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pull_skips_actual_oversized_message_when_server_lies() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("pull-actual-oversized-skip");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let oversized_body = "x".repeat(400);
    let oversized = format!(
        "Message-ID: <actual-oversized@example.com>\r\nX-Afmail-Test-Declared-Size: 64\r\nFrom: a@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Lying size\r\n\r\n{oversized_body}\r\n"
    )
    .into_bytes();
    let normal = b"Message-ID: <normal-after-lie@example.com>\r\nX-Afmail-Test-Declared-Size: 180\r\nFrom: b@example.com\r\nTo: me@example.com\r\nDate: Thu, 21 May 2026 10:00:00 +0000\r\nSubject: Normal\r\n\r\nNormal body\r\n".to_vec();
    let server = match start_imap_server(vec![(1, oversized), (2, normal)], 3) {
        Some(server) => server,
        None => return,
    };
    let mut config = test_config(Some(server.addr.port()), None);
    config["imap"]["allow_unbounded_body_fetch"] = json!(false);
    write_json(&root.join(".afmail/config.json"), &config);
    let (status, stdout) = run_with_env(
        &root,
        &["pull", "--all-history", "inbox"],
        &[("AFMAIL_TEST_MAX_MESSAGE_BYTES", "256")],
    );
    assert_eq!(status, 0, "{stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["new_message_count"], 1, "{result}");
    assert_eq!(result["skipped_message_count"], 1);
    assert_eq!(
        result["skipped_messages"][0]["reason"],
        "actual_size_exceeded"
    );
    assert_eq!(result["skipped_messages"][0]["uid"], 1);
    assert_eq!(result["skipped_messages"][0]["max_message_bytes"], 256);
    assert!(server.handle.join().is_ok());
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
    assert_eq!(shown["body_text"], "Please review the attachment.");
    assert_eq!(shown["attachment_count"], 1);
    assert_eq!(shown["attachments"][0]["content_type"], "text/plain");

    let (status, stdout) = run(&root, &["message", "attachment", "fetch", &id]);
    assert_eq!(status, 0, "{stdout}");
    let saved_all = parse_one(&stdout);
    assert_eq!(saved_all["count"], 1);
    assert_eq!(saved_all["items"][0]["storage"], "message_cache");

    let (status, stdout) = run(&root, &["message", "attachment", "fetch", &id, &part_id]);
    assert_eq!(status, 0, "{stdout}");
    let saved = parse_one(&stdout);
    assert_eq!(saved["filename"], "pricing.txt");
    assert_eq!(saved["saved_filename"], "pricing.txt");
    assert_eq!(saved["content_type"], "text/plain");
    assert_eq!(saved["storage"], "message_cache");
    assert!(
        saved["file_path"]
            .as_str()
            .map(|path| root.join(path).is_file())
            .unwrap_or(false)
    );
    assert!(server.handle.join().is_ok());
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
    assert!(message["workspace"].get("notification_uid").is_none());
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
    update_message_json(&root, "message_existing", |message| {
        message["remote"]["locations"][0]["uid_validity"] = json!(44);
    });
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
    assert!(message["workspace"].get("notification_uid").is_none());
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
        Some("support"),
        Some("message_001"),
        Some("initial case"),
    );
    let case_before = fs::read_to_string(case_path.join("data/case.json"));
    assert!(case_before.is_ok());
    let case_before = case_before.unwrap_or_default();
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
    assert_eq!(value["capabilities"]["move"], true);
    let (status, stdout) = run(&root, &["remote", "folders"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert!(value["mailboxes"].as_array().map(|a| a.len()).unwrap_or(0) >= 3);
    assert!(value["mailboxes"].as_array().is_some_and(|folders| {
        folders
            .iter()
            .any(|folder| folder["mailbox_name"] == "Junk" && folder["special_use"] == "junk")
    }));
    assert!(
        value["mailboxes"]
            .as_array()
            .is_some_and(|folders| folders.iter().any(|folder| folder["mailbox_name"]
                == "Archive"
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
                        .any(|item| item["kind"] == "archive" && item["source"] == "mailboxes"))))
    );
    assert!(
        value["special_use_targets"]
            .as_array()
            .is_some_and(
                |targets| targets.iter().any(|target| target["kind"] == "archive"
                    && target["mailbox_name"] == "Archive"
                    && target["source"] == "mailboxes"
                    && target["exists"] == true)
            )
    );
    assert!(server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn imap_connect_failure_is_retryable_and_emits_retry_log() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("imap-retryable-connect");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let listener = bind_loopback_listener();
    assert!(listener.is_some());
    let port = listener.map(|listener| listener.local_addr().map(|addr| addr.port()).ok());
    let Some(Some(port)) = port else { return };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(port), None),
    );

    let (status, stdout) = run(&root, &["--log", "retry", "remote", "test"]);
    assert_eq!(status, 1, "{stdout}");
    let lines = parse_lines(&stdout);
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0]["event"], "retry");
    assert_eq!(lines[0]["phase"], "retryable_error");
    assert_eq!(lines[0]["error_code"], "imap_connect_failed");
    assert_eq!(lines[0]["error"]["redacted"], true);
    assert!(lines[0]["error"]["original_hash"].as_str().is_some());
    assert_eq!(lines[1]["code"], "imap_connect_failed");
    assert_eq!(lines[1]["retryable"], true);
    assert_eq!(lines[1]["protocol"], "imap");
    assert_eq!(lines[1]["phase"], "connect");
    assert_eq!(lines[1]["host"], "127.0.0.1");
    assert_eq!(lines[1]["port"], port);
    assert_eq!(lines[1]["tls"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn smtp_remote_test_failure_is_retryable_and_emits_retry_log() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("smtp-retryable-connect");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let imap_server = start_imap_server(Vec::new(), 5);
    assert!(imap_server.is_some());
    let Some(imap_server) = imap_server else {
        return;
    };
    let listener = bind_loopback_listener();
    assert!(listener.is_some());
    let port = listener.map(|listener| listener.local_addr().map(|addr| addr.port()).ok());
    let Some(Some(smtp_port)) = port else { return };
    write_json(
        &root.join(".afmail/config.json"),
        &test_config(Some(imap_server.addr.port()), Some(smtp_port)),
    );

    let (status, stdout) = run(&root, &["--log", "retry", "remote", "test"]);
    assert_eq!(status, 1, "{stdout}");
    let lines = parse_lines(&stdout);
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0]["event"], "retry");
    assert_eq!(lines[0]["phase"], "retryable_error");
    assert_eq!(lines[0]["error_code"], "smtp_test_failed");
    assert_eq!(lines[0]["error"]["redacted"], true);
    assert!(lines[0]["error"]["original_hash"].as_str().is_some());
    assert_eq!(lines[1]["code"], "smtp_test_failed");
    assert_eq!(lines[1]["retryable"], true);
    assert_eq!(lines[1]["protocol"], "smtp");
    assert_eq!(lines[1]["phase"], "connect");
    assert_eq!(lines[1]["host"], "127.0.0.1");
    assert_eq!(lines[1]["port"], smtp_port);
    assert_eq!(lines[1]["starttls"], false);
    assert_eq!(lines[1]["tls_wrapper"], false);
    assert!(imap_server.handle.join().is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn message_batch_partial_and_failed_outcomes_preserve_all_item_results() {
    let root = temp_root("message-batch-outcomes");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_batch_ok", 44, None);
    write_triage(&root, "message_batch_ok", "batch outcome note");

    let (status, stdout) = run(
        &root,
        &[
            "--log",
            "progress",
            "message",
            "spam",
            "message_batch_ok",
            "message_batch_missing",
            "--reason",
            "verify partial outcome",
        ],
    );
    assert_eq!(status, 3, "{stdout}");
    let lines = parse_lines(&stdout);
    let finish = lines.first().unwrap_or(&Value::Null);
    assert_eq!(finish["event"], "progress");
    assert_eq!(finish["phase"], "finish");
    assert_eq!(finish["level"], "warn");
    assert_eq!(finish["status"], "partial");
    assert_eq!(finish["success"], false);
    assert_eq!(finish["exit_code"], 3);
    let value = lines.last().unwrap_or(&Value::Null);
    assert_eq!(value["code"], "partial");
    assert!(value["items"][0].get("code").is_none());
    assert!(value["items"][1]["error"]["code"].as_str().is_some());
    assert_eq!(value["items"][1]["message_id"], "message_batch_missing");
    assert!(value["items"][1]["error"]["message"].as_str().is_some());
    assert!(value["items"][1]["error"]["retryable"].as_bool().is_some());

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "spam",
            "message_missing_one",
            "message_missing_two",
            "--reason",
            "verify failed outcome",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "store_error");
    assert_eq!(value["items"].as_array().map(Vec::len), Some(2));

    let log_path = root.join(".afmail/logs/events.jsonl");
    let log_file = fs::OpenOptions::new().append(true).open(&log_path);
    assert!(log_file.is_ok());
    if let Ok(mut log_file) = log_file {
        assert!(log_file.write_all(b"{truncated\n").is_ok());
    }
    let (status, stdout) = run(&root, &["log", "list", "--limit", "20"]);
    assert_eq!(status, 3, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "partial");
    assert_eq!(value["warning_count"], 1);
    assert_eq!(value["warnings"].as_array().map(Vec::len), Some(1));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn stdout_file_rejects_non_regular_target() {
    if !Path::new("/dev/full").exists() {
        return;
    }
    let root = temp_root("stdout-file-non-regular");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    // The stream-redirect layer requires a regular file, so a character
    // device like /dev/full is rejected before any command runs — the error
    // is emitted on the normal stdout with a dedicated cli_error exit code.
    let (status, stdout, stderr) =
        run_allow_stderr(&root, &["--stdout-file", "/dev/full", "status"]);
    assert_eq!(status, 2, "stdout={stdout} stderr={stderr}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error", "{value}");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|message| message.contains("regular file")),
        "{value}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_refresh_progress_logs_render_substages() {
    let root = temp_root("render-refresh-progress");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_render_progress", 86, None);

    let (status, stdout) = run(&root, &["--log", "progress", "render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let lines = parse_lines(&stdout);
    let phases = lines
        .iter()
        .filter(|line| line["event"] == "progress")
        .filter_map(|line| line["phase"].as_str())
        .collect::<Vec<_>>();
    for phase in [
        "render_start",
        "render_materialize_start",
        "render_materialize_done",
        "render_triage_start",
        "render_triage_progress",
        "render_triage_done",
        "render_dispositions_start",
        "render_dispositions_done",
        "render_cases_start",
        "render_cases_done",
        "render_notifications_start",
        "render_notifications_done",
        "render_cache_write_start",
        "render_cache_write_done",
        "render_done",
        "finish",
    ] {
        assert!(phases.contains(&phase), "{stdout}");
    }
    let triage_done = lines
        .iter()
        .find(|line| line["phase"] == "render_triage_done")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(triage_done["render_stage"], "triage");
    assert_eq!(triage_done["triage_count"], 1);
    assert!(triage_done["elapsed_ms"].as_u64().is_some());

    let progress = read_json(&root.join(".afmail/cache/progress.json"));
    assert_eq!(progress["command"], "render");
    assert_eq!(progress["status"], "succeeded");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_refresh_reports_progress_write_failure_as_partial() {
    let root = temp_root("render-progress-write-failure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_render_progress_write_failure", 87, None);
    let _ = fs::remove_file(root.join(".afmail/cache/progress.json"));
    assert!(fs::create_dir(root.join(".afmail/cache/progress.json")).is_ok());

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 3, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "partial");
    assert!(
        value["progress_warning_count"].as_u64().unwrap_or(0) > 0,
        "{stdout}"
    );
    assert_eq!(
        value["progress_warnings"][0]["code"],
        "progress_write_failed"
    );
    assert!(
        value["progress_warnings"][0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with(".afmail/cache/progress.json")),
        "{stdout}"
    );
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["progress"]["status"], "unavailable");
    assert_eq!(value["progress"]["error"]["code"], "progress_unavailable");
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
    write_message(&root, "message_notification", 85, None);
    write_triage(&root, "message_notification", "archive");
    let (_notification_uid, archive_path, _) = create_notification(
        &root,
        "billing",
        Some("message_notification"),
        Some("billing summary"),
        Some("archive"),
    );

    assert!(fs::create_dir_all(root.join(".afmail/templates/en-US/case")).is_ok());
    assert!(
        fs::write(
            root.join(".afmail/templates/en-US/case/case.md.j2"),
            "IGNORED LEGACY {{ case.collection_uid }}\n"
        )
        .is_ok()
    );
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["workspace"], 0);
    assert_eq!(value["template_sources"]["case/case.md.j2"]["builtin"], 1);
    assert!(
        !fs::read_to_string(case_path.join("case.md"))
            .unwrap_or_default()
            .contains("IGNORED LEGACY")
    );

    assert!(fs::create_dir_all(root.join("templates/en-US/case")).is_ok());
    assert!(
        fs::write(
            root.join("templates/en-US/case/case.md.j2"),
            "USER CASE {{ case.collection_uid }} {{ view.message_count }}\n"
        )
        .is_ok()
    );
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
    assert!(
        !fs::read_to_string(archive_path.join("notification.md"))
            .unwrap_or_default()
            .contains("USER CASE")
    );

    assert!(
        fs::write(
            root.join("templates/en-US/case/case.md.j2"),
            "{% for item in items %}"
        )
        .is_ok()
    );
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 1);
    assert_eq!(parse_one(&stdout)["code"], "template_render_failed");
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
    assert_eq!(value["template_dir"], "templates");
    assert_eq!(value["template_dir_created"], false);
    assert_eq!(value["exported_count"], 126);
    assert_eq!(value["workspace_count"], 126);
    assert!(root.join("templates/en-US/case/case.md.j2").is_file());
    assert!(
        root.join("templates/zh-CN/notification/message.md.j2")
            .is_file()
    );

    assert!(
        fs::write(
            root.join("templates/en-US/case/case.md.j2"),
            "CUSTOM {{ case.collection_uid }}\n"
        )
        .is_ok()
    );
    let (status, stdout) = run(&root, &["render", "templates"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["kept_count"], 126);
    assert_eq!(
        fs::read_to_string(root.join("templates/en-US/case/case.md.j2")).unwrap_or_default(),
        "CUSTOM {{ case.collection_uid }}\n"
    );

    let other = temp_root("render-templates-list");
    assert!(fs::create_dir_all(&other).is_ok());
    assert_eq!(run(&other, &["init"]).0, 0);
    assert!(fs::create_dir_all(other.join(".afmail/templates/case")).is_ok());
    assert!(fs::write(other.join(".afmail/templates/case/case.md.j2"), "USER\n").is_ok());
    let (status, stdout) = run(&other, &["render", "templates"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["exported_count"], 126);
    assert_eq!(value["workspace_count"], 126);
    assert_eq!(value["builtin_count"], 0);
    assert_eq!(value["items"][0]["source"], "workspace");
    assert_eq!(value["items"][0]["language"], "en-US");

    let (status, stdout) = run(&root, &["render", "templates", "--force"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["overwritten_count"], 126);
    assert!(
        fs::read_to_string(root.join("templates/en-US/case/case.md.j2"))
            .unwrap_or_default()
            .contains("Case: {{ case.collection_uid }} · Messages: {{ view.message_count }}")
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(other);
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
    assert_eq!(value["count"], 2);
    assert_eq!(value["returned_count"], 2);
    assert_eq!(value["limited"], false);
    assert_eq!(value["details"], false);
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

    let (status, stdout) = run(&root, &["triage", "list", "--limit", "1"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 2);
    assert_eq!(value["returned_count"], 1);
    assert_eq!(value["limited"], true);
    assert_eq!(value["limit"], 1);
    assert_eq!(value["items"][0]["message_id"], "message_t2");

    let (status, stdout) = run(&root, &["triage", "list", "--limit", "1", "--details"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 2);
    assert_eq!(value["returned_count"], 1);
    assert_eq!(value["details"], true);
    assert_eq!(value["items"][0]["message_id"], "message_t2");
    assert_eq!(value["items"][0]["workspace_status"], "triage");
    assert_eq!(value["items"][0]["related_message_ids"], json!([]));
    assert_eq!(value["items"][0]["case_refs"], json!([]));
    assert_eq!(value["items"][0]["view_path"], "triage/message_t2.md");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shared_mailbox_reads_leave_workspace_bytes_and_mtimes_unchanged() {
    let root = temp_root("shared-reads-pure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));

    write_message(&root, "message_read_triage", 61, None);
    write_triage(&root, "message_read_triage", "still in triage");

    write_message(&root, "message_read_case", 62, None);
    write_triage(&root, "message_read_case", "case read");
    let (case_uid, _, _) = create_case(
        &root,
        "read-only-case",
        Some("support"),
        Some("message_read_case"),
        Some("case read"),
    );

    write_message(&root, "message_read_notification", 63, None);
    write_triage(&root, "message_read_notification", "notification read");
    let (notification_uid, _, _) = create_notification(
        &root,
        "read-only-notification",
        Some("message_read_notification"),
        Some("notification read"),
        Some("notification read"),
    );

    let _ = fs::remove_file(workspace_runtime_lock_path(&root));
    assert!(!root.join(".afmail/workspace.lock").exists());
    let before = workspace_tree_snapshot(&root);
    for args in [
        vec!["status"],
        vec!["message", "show", "message_read_triage"],
        vec!["message", "list"],
        vec!["triage", "list", "--details"],
        vec!["case", "show", case_uid.as_str()],
        vec!["notification", "show", notification_uid.as_str()],
        vec!["push", "list"],
        vec!["push"],
    ] {
        let command_before = workspace_tree_snapshot(&root);
        let (status, stdout) = run(&root, &args);
        assert_eq!(status, 0, "args={args:?}: {stdout}");
        let command_after = workspace_tree_snapshot(&root);
        let differences = workspace_snapshot_differences(&command_before, &command_after);
        assert!(
            differences.is_empty(),
            "shared read {args:?} changed workspace:\n{}",
            differences.join("\n")
        );
    }
    let after = workspace_tree_snapshot(&root);
    assert_eq!(after, before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_shared_readers_leave_workspace_unchanged() {
    let root = temp_root("concurrent-shared-reads-pure");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_message(&root, "message_concurrent_read", 65, None);
    write_triage(&root, "message_concurrent_read", "concurrent read");
    let _ = fs::remove_file(workspace_runtime_lock_path(&root));

    let before = workspace_tree_snapshot(&root);
    let _command_guard = COMMAND_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let outputs = thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..12 {
            let root = &root;
            handles.push(scope.spawn(move || {
                let args: &[&str] = if index % 3 == 0 {
                    &["push", "list"]
                } else if index % 3 == 1 {
                    &["message", "show", "message_concurrent_read"]
                } else {
                    &["status"]
                };
                Command::new(bin()).current_dir(root).args(args).output()
            }));
        }
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect::<Vec<_>>()
    });
    assert_eq!(outputs.len(), 12, "a shared reader thread panicked");
    for output in outputs {
        let (status, stdout) = command_output(output);
        assert_eq!(status, 0, "{stdout}");
    }
    let after = workspace_tree_snapshot(&root);
    let differences = workspace_snapshot_differences(&before, &after);
    assert!(
        differences.is_empty(),
        "concurrent shared reads changed workspace:\n{}",
        differences.join("\n")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn list_commands_return_locators_and_status_includes_counts_and_progress() {
    let root = temp_root("locator-lists");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["actions"]["message.notification"]["by_source_mailbox_id"]["inbox"]["steps"] = json!([]);
    write_json(&root.join(".afmail/config.json"), &config);

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
    assert_eq!(value["count"], 1);
    assert_eq!(value["active_count"], 1);
    assert_eq!(value["archiving_count"], 0);
    let item = value["items"][0].clone();
    assert_eq!(item["case_uid"], case_uid);
    assert_eq!(item["case_name"], "active-locator");
    assert_eq!(item["group"], "waiting");
    assert!(
        item["case_dir"]
            .as_str()
            .is_some_and(|path| path.starts_with(&case_uid))
    );
    assert_eq!(
        value["path_templates"]["active_view_path"],
        "cases/{group}/{case_dir}/case.md"
    );
    assert_eq!(
        value["path_templates"]["active_data_path"],
        "cases/{group}/{case_dir}/data/case.json"
    );
    assert!(item.get("view_path").is_none());
    assert!(item.get("data_path").is_none());
    assert!(item.get("path").is_none());
    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    write_message(&root, "message_notification_list", 54, None);
    write_triage(&root, "message_notification_list", "archive seed");
    let (notification_uid, _archive_path, _) = create_notification(
        &root,
        "archive-locator",
        Some("message_notification_list"),
        Some("reference notification"),
        Some("file it"),
    );
    let (status, stdout) = run(&root, &["notification", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 1);
    let item = value["items"][0].clone();
    assert_eq!(item["notification_uid"], notification_uid);
    assert_eq!(item["notification_name"], "archive-locator");
    assert!(
        item["notification_dir"]
            .as_str()
            .is_some_and(|path| path.starts_with(&notification_uid))
    );
    assert_eq!(
        value["path_templates"]["view_path"],
        "notifications/{notification_dir}/notification.md"
    );
    assert_eq!(
        value["path_templates"]["data_path"],
        "notifications/{notification_dir}/data/notification.json"
    );
    assert!(item.get("view_path").is_none());
    assert!(item.get("data_path").is_none());
    assert!(item.get("path").is_none());
    assert!(item.get("message_count").is_none());

    let (status, stdout) = run(&root, &["case", "archive", &case_uid, "--reason", "done"]);
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(&root, &["case", "list"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["scope"], "active");
    assert_eq!(value["count"], 1);
    assert_eq!(value["active_count"], 0);
    assert_eq!(value["archiving_count"], 1);
    let item = value["items"][0].clone();
    assert_eq!(item["case_uid"], case_uid);
    assert_eq!(item["case_name"], "active-locator");
    assert_eq!(item["group"], "waiting");
    assert_eq!(item["archived"], true);
    assert_eq!(item["lifecycle"], "archiving");
    assert!(
        item["case_dir"]
            .as_str()
            .is_some_and(|path| path.starts_with(&case_uid))
    );
    assert_eq!(
        value["path_templates"]["archiving_view_path"],
        "archived-cases/{group}/{case_dir}/case.md"
    );
    assert_eq!(
        value["path_templates"]["archiving_data_path"],
        "archived-cases/{group}/{case_dir}/data/case.json"
    );
    assert!(item.get("view_path").is_none());
    assert!(item.get("data_path").is_none());
    assert!(item.get("path").is_none());

    let (status, stdout) = run(&root, &["case", "list", "--all"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["scope"], "all");
    assert_eq!(value["count"], 1);
    assert_eq!(value["items"][0]["case_uid"], case_uid);
    assert_eq!(value["items"][0]["group"], "waiting");
    assert_eq!(value["items"][0]["archived"], true);
    assert!(value.get("message_count").is_none());
    assert!(value.get("messages").is_none());
    assert_eq!(
        value["path_templates"]["archived_view_path"],
        "archived-cases/{group}/{case_dir}/case.md"
    );
    assert!(value.get("message_path_templates").is_none());

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["case_count"], 1);
    assert_eq!(value["active_case_count"], 0);
    assert_eq!(value["archiving_case_count"], 1);
    assert_eq!(value["archived_case_count"], 0);
    assert_eq!(value["notification_count"], 1);
    assert_eq!(value["progress"]["status"], "idle");
    assert!(value.get("cases").is_none());
    assert!(value.get("archived_cases").is_none());
    assert!(value.get("notification_categories").is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn header_related_messages_require_case_for_direct_disposition() {
    let root = temp_root("related-requires-case");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    let mut config = test_config(None, None);
    config["actions"]["message.notification"]["by_source_mailbox_id"]["inbox"]["steps"] = json!([]);
    write_json(&root.join(".afmail/config.json"), &config);
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

    let (status, stdout) = run(&root, &["triage", "list", "--details"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    let items = value["items"].as_array().cloned().unwrap_or_default();
    let parent = items
        .iter()
        .find(|item| item["message_id"] == "message_health_parent")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        parent["related_message_ids"],
        json!(["message_health_reply"])
    );
    assert_eq!(parent["requires_case"], true);
    assert_eq!(
        parent["related_messages"][0]["message_id"],
        "message_health_reply"
    );
    assert_eq!(parent["related_messages"][0]["workspace_status"], "triage");

    let (status, stdout) = run(
        &root,
        &["message", "show", "message_health_parent", "--meta"],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(
        value["related_message_ids"],
        json!(["message_health_reply"])
    );
    assert_eq!(
        value["related_messages"][0]["message_id"],
        "message_health_reply"
    );
    assert_eq!(value["workspace_status"], "triage");
    assert!(value.get("body_text").is_none());
    assert!(value.get("message").is_none());

    let (status, stdout) = run(&root, &["message", "show", "message_health_parent"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(
        value["related_message_ids"],
        json!(["message_health_reply"])
    );
    assert!(value.get("body_text").is_some());
    assert!(value.get("message").is_some());

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
    assert_eq!(value["code"], "message_has_related_conversation_use_case");
    assert_eq!(
        value["items"][0]["error"]["related_message_ids"],
        json!(["message_health_reply"])
    );
    assert!(
        value["items"][0]["error"]["suggested_commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .unwrap_or_default()
                .contains("afmail case add CASE_REF message_health_reply")))
    );
    assert!(
        value["items"][0]["error"]["suggested_commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .unwrap_or_default()
                .contains("afmail case trash CASE_REF")))
    );

    let (done_notification_uid, _done_archive_path, _) =
        create_notification(&root, "done", None, None, Some("done archive"));
    let (status, stdout) = run(
        &root,
        &[
            "notification",
            "add",
            &done_notification_uid,
            "message_health_reply",
            "--summary",
            "reply done",
            "--reason",
            "done",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "message_has_related_conversation_use_case");
    assert_eq!(
        value["items"][0]["error"]["related_message_ids"],
        json!(["message_health_parent"])
    );
    assert!(
        value["items"][0]["error"]["suggested_commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .unwrap_or_default()
                .contains("afmail case add CASE_REF message_health_parent")))
    );
    assert!(
        value["items"][0]["error"]["suggested_commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .unwrap_or_default()
                .contains("afmail case trash CASE_REF")))
    );

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
        value[0]["related_message_ids"],
        json!(["message_health_parent"])
    );
    let (status, stdout) = run(
        &root,
        &["message", "show", "message_health_parent", "--meta"],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["case_refs"][0]["case_uid"], case_uid);
    assert_eq!(
        value["related_messages"][0]["case_refs"][0]["case_uid"],
        case_uid
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
    assert_eq!(
        value["case_path"],
        format!("archived-cases/support/{case_uid}-health-conversation")
    );
    assert!(
        root.join(format!(
            "archived-cases/support/{case_uid}-health-conversation/case.md"
        ))
        .exists()
    );
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
fn render_refresh_does_not_create_or_delete_empty_disposition_state() {
    let root = temp_root("render-empty-dispositions");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let refreshed = parse_one(&stdout);
    assert_eq!(refreshed["spam_count"], 0);
    assert_eq!(refreshed["trash_count"], 0);
    assert_eq!(refreshed["deleted_count"], 0);
    for (dir, file) in [
        ("spam", "spam.json"),
        ("trash", "trash.json"),
        ("deleted", "deleted.json"),
    ] {
        assert!(!root.join(dir).exists());
        assert!(!root.join(dir).join("data").join(file).exists());
        assert!(!root.join(dir).join("index.md").exists());
    }

    for (dir, file, schema_name, status) in [
        ("spam", "spam.json", "spam", "spam"),
        ("trash", "trash.json", "trash", "trashed"),
        (
            "deleted",
            "deleted.json",
            "deleted_remote",
            "deleted_remote",
        ),
    ] {
        write_json(
            &root.join(dir).join("data").join(file),
            &json!({
                "schema_name": schema_name,
                "schema_version": 1,
                "collection_uid": dir,
                "collection_name": dir,
                "status": status,
                "items": []
            }),
        );
        assert!(fs::write(root.join(dir).join("index.md"), "stale index").is_ok());
    }

    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    let refreshed = parse_one(&stdout);
    assert_eq!(refreshed["spam_count"], 0);
    assert_eq!(refreshed["spam_written_count"], 0);
    assert_eq!(refreshed["stale_spam_removed_count"], 0);
    assert_eq!(refreshed["trash_count"], 0);
    assert_eq!(refreshed["trash_written_count"], 0);
    assert_eq!(refreshed["stale_trash_removed_count"], 0);
    assert_eq!(refreshed["deleted_count"], 0);
    assert_eq!(refreshed["deleted_written_count"], 0);
    assert_eq!(refreshed["stale_deleted_removed_count"], 0);
    for (dir, file) in [
        ("spam", "spam.json"),
        ("trash", "trash.json"),
        ("deleted", "deleted.json"),
    ] {
        assert!(root.join(dir).join("data").join(file).is_file());
        assert!(root.join(dir).join("index.md").is_file());
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn purge_deletes_old_local_discard_records() {
    let root = temp_root("purge-dispositions");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert_eq!(run(&root, &["purge", "--confirm"]).0, 2);
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
        set_local_disposition(&root, message_id, status, updated);
    }
    let (status, stdout) = run(&root, &["render", "refresh"]);
    assert_eq!(status, 0, "{stdout}");
    assert!(root.join("spam/message_spam_old.md").is_file());
    assert!(root.join("trash/message_trash_old.md").is_file());
    assert!(root.join("deleted/message_deleted_old.md").is_file());

    let (status, stdout) = run(&root, &["purge", "spam"]);
    assert_eq!(status, 0, "{stdout}");
    let plan = parse_one(&stdout);
    assert_eq!(plan["target"], "spam");
    assert_eq!(plan["candidate_count"], 1);
    assert!(root.join("messages/message_spam_old.json").is_file());
    let stale_plan_id = plan["plan_id"].as_str().unwrap_or_default();
    update_message_json(&root, "message_spam_old", |message| {
        message["subject"] = json!("changed after purge preview");
    });
    let (status, stdout) = run(&root, &["purge", "spam", "--confirm", stale_plan_id]);
    assert_eq!(status, 1, "{stdout}");
    let error = parse_one(&stdout);
    assert_eq!(error["code"], "purge_plan_changed");
    assert_eq!(error["local_deletions_started"], false);
    assert!(root.join("messages/message_spam_old.json").is_file());
    let (status, stdout) = run(&root, &["purge", "spam"]);
    assert_eq!(status, 0, "{stdout}");
    let plan = parse_one(&stdout);
    let plan_id = plan["plan_id"].as_str().unwrap_or_default();
    let (status, stdout) = run(&root, &["purge", "spam", "--confirm", plan_id]);
    assert_eq!(status, 0, "{stdout}");
    let purged = parse_one(&stdout);
    assert_eq!(purged["confirmed"], true);
    assert_eq!(purged["plan_id"], plan_id);
    assert_eq!(purged["purged_message_ids"], json!(["message_spam_old"]));
    assert_eq!(purged["purged_spam_count"], 1);
    assert_eq!(purged["purged_trash_count"], 0);
    assert_eq!(purged["purged_deleted_count"], 0);
    assert_eq!(purged["skipped"][0]["message_id"], "message_spam_recent");
    assert_eq!(purged["skipped"][0]["reason"], "recent");
    assert!(!root.join("messages/message_spam_old.json").exists());
    assert!(!root.join(".afmail/messages/message_spam_old.eml").exists());
    assert!(
        !root
            .join(".afmail/messages/message_spam_old.state.json")
            .exists()
    );
    assert!(
        !root
            .join(".afmail/messages/message_spam_old.remote.json")
            .exists()
    );
    assert!(!root.join("spam/message_spam_old.md").exists());
    assert!(root.join("messages/message_spam_recent.json").is_file());
    assert!(root.join("messages/message_trash_old.json").is_file());
    assert!(root.join("messages/message_deleted_old.json").is_file());
    assert!(root.join("trash/message_trash_old.md").is_file());
    assert!(root.join("deleted/message_deleted_old.md").is_file());

    let (status, stdout) = run(&root, &["purge"]);
    assert_eq!(status, 0, "{stdout}");
    let plan = parse_one(&stdout);
    assert!(root.join("messages/message_trash_old.json").is_file());
    let plan_id = plan["plan_id"].as_str().unwrap_or_default();
    let (status, stdout) = run(&root, &["purge", "--confirm", plan_id]);
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
    assert_eq!(purged["skipped"][0]["message_id"], "message_spam_recent");
    assert!(!root.join("messages/message_trash_old.json").exists());
    assert!(!root.join("messages/message_deleted_old.json").exists());
    assert!(!root.join("trash/message_trash_old.md").exists());
    assert!(!root.join("deleted/message_deleted_old.md").exists());
    assert!(root.join("messages/message_spam_recent.json").is_file());
    assert!(root.join("messages/message_triage_old.json").is_file());
    assert!(root.join("spam/index.md").is_file());
    assert!(root.join("trash/index.md").is_file());
    assert!(root.join("trash/data/trash.json").is_file());
    assert_eq!(
        read_json(&root.join("trash/data/trash.json"))["items"],
        json!([])
    );
    assert!(root.join("deleted/index.md").is_file());
    assert!(root.join("deleted/data/deleted.json").is_file());
    assert_eq!(
        read_json(&root.join("deleted/data/deleted.json"))["items"],
        json!([])
    );

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
    let plan = parse_one(&stdout);
    assert!(root.join("messages/message_missing.json").exists());
    let plan_id = plan["plan_id"].as_str().unwrap_or_default();
    let (status, stdout) = run(
        &root,
        &[
            "purge",
            "deleted",
            "--older-than-days",
            "0",
            "--confirm",
            plan_id,
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let purged = parse_one(&stdout);
    assert_eq!(purged["target"], "deleted");
    assert_eq!(purged["purged_message_ids"], json!(["message_missing"]));
    assert_eq!(purged["purged_deleted_count"], 1);
    assert!(!root.join("messages/message_missing.json").exists());
    assert!(!root.join("deleted/message_missing.md").exists());
    assert!(root.join("deleted/data/deleted.json").is_file());
    assert_eq!(
        read_json(&root.join("deleted/data/deleted.json"))["items"],
        json!([])
    );
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
    let _case_path = write_case_fixture(&root, "support", case_uid, "case-ref", &["message_spam"]);

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
    assert_eq!(value["code"], "message_referenced");
    assert!(value["message"].as_str().is_some_and(|error| {
        error.contains(&format!("cases/support/{case_uid}-case-ref/data/case.json"))
    }));
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
fn status_reports_progress_while_workspace_is_exclusively_locked() {
    let root = temp_root("status-progress-locked");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_json(&root.join(".afmail/config.json"), &test_config(None, None));
    write_json(
        &root.join(".afmail/cache/progress.json"),
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
        .open(workspace_runtime_lock_path(&root));
    assert!(workspace_lock.is_ok());
    let Ok(workspace_lock) = workspace_lock else {
        return;
    };
    assert!(FileExt::try_lock(&workspace_lock).is_ok());

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
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
    assert!(
        value["hint"]
            .as_str()
            .is_some_and(|text| text.contains("counts are omitted"))
    );
    let object = value.as_object().cloned().unwrap_or_default();
    assert!(!object.contains_key("message_count"), "{stdout}");
    assert!(!object.contains_key("push_count"), "{stdout}");

    assert!(FileExt::unlock(&workspace_lock).is_ok());
    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
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
        .open(workspace_runtime_lock_path(&root));
    assert!(workspace_lock.is_ok());
    let Ok(workspace_lock) = workspace_lock else {
        return;
    };
    assert!(FileExt::try_lock_shared(&workspace_lock).is_ok());

    let (status, stdout) = run(&root, &["status"]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    let (status, stdout) = run(&root, &["case", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    let (status, stdout) = run(&root, &["case", "notes", "show", &case_uid]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

    let (status, stdout) = run(
        &root,
        &[
            "case",
            "move",
            &case_uid,
            "--group",
            "waiting",
            "--reason",
            "locked writer check",
        ],
    );
    assert_eq!(status, 1);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "workspace_locked");
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
    assert_eq!(value["code"], "workspace_locked");
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
fn triage_assign_move_draft_and_send_contract_work() {
    let _network_guard = NETWORK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = temp_root("assign");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    write_message(&root, "message_001", 1, None);
    write_triage(&root, "message_001", "internal note");

    let (case_uid, case_path, _value) = create_case(
        &root,
        "acme-contract",
        Some("support"),
        Some("message_001"),
        Some("new contract case"),
    );
    assert!(case_path.join("case.md").is_file());
    assert!(!root.join("triage/message_001.md").exists());

    let message_data = fs::read_to_string(root.join("messages/message_001.json"));
    assert!(message_data.is_ok());
    let message_value: Result<Value, _> = serde_json::from_str(&message_data.unwrap_or_default());
    assert!(message_value.is_ok());
    let message_value = message_value.unwrap_or(Value::Null);
    assert_eq!(message_value["workspace"]["status"], "case");
    let case_messages: Value = serde_json::from_str(
        &fs::read_to_string(case_path.join("data/case.json")).unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(case_messages["items"][0]["message_id"], "message_001");

    let (status, _) = run(
        &root,
        &[
            "case",
            "move",
            &case_uid,
            "--group",
            "waiting",
            "--reason",
            "better group",
        ],
    );
    assert_eq!(status, 0);
    let waiting_path = root.join(format!("cases/waiting/{case_uid}-acme-contract"));
    assert!(waiting_path.join("case.md").is_file());

    let draft = draft_with_compose(format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: reply\nreply_to_message_id: message_001\nto:\n  - alice@example.com\ncc: []\nsubject: \"Re: Contract renewal\"\nattachments:\n---\n\nHi Alice\n"
    ));
    assert!(fs::write(waiting_path.join("drafts/reply.md"), &draft).is_ok());
    let (status, stdout) = run(&root, &["case", "draft", "validate", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    let _ = parse_one(&stdout);

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
    let (status, stdout) = run(&root, &["case", "draft", "upload", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["action"], "save_draft");
    assert!(root.join(".afmail/push").read_dir().is_ok());

    let (status, stdout) = run(&root, &["push", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["count"], 1);

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value.as_array().map(Vec::len), Some(1));
    assert!(!waiting_path.join("drafts/reply.md").exists());
    let staged_count = fs::read_dir(root.join("messages"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("message_sent_"))
                })
                .count()
        })
        .unwrap_or(0);
    assert_eq!(staged_count, 0);

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
    assert!(fs::write(waiting_path.join("drafts/reply.md"), draft).is_ok());
    let (status, stdout) = run(&root, &["case", "draft", "send", &case_uid, "reply.md"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["action"], "send");
    let push_id = value["push_id"].as_str().unwrap_or_default();
    let message_id = sent_message_id_for_push_id(push_id);

    let (status, stdout) = run(&root, &["push", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(
        value["items"][0]["actions"][0],
        "append_to_mailbox_id_drafts"
    );
    assert_eq!(value["items"][0]["actions"][1], "smtp_send");

    let (status, stdout) = run_confirmed_push(&root);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value.as_array().map(Vec::len), Some(1), "{value}");
    assert!(!waiting_path.join("drafts/reply.md").exists());
    assert!(
        root.join(format!(".afmail/messages/{message_id}.eml"))
            .is_file()
    );
    let outbound = fs::read_to_string(root.join(format!(".afmail/messages/{message_id}.eml")));
    assert!(
        outbound
            .as_ref()
            .map(|text| text.contains("In-Reply-To: <message_001@example.com>"))
            .unwrap_or(false)
    );
    assert!(
        outbound
            .as_ref()
            .map(|text| text.contains("Hi Alice"))
            .unwrap_or(false)
    );
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
    assert_eq!(appended.len(), 3);
    assert!(
        appended
            .iter()
            .any(|(folder, draft, body)| folder == "Drafts" && *draft && body.contains("Hi Alice"))
    );
    assert!(
        appended
            .iter()
            .any(|(folder, draft, body)| folder == "Sent" && !*draft && body.contains("Hi Alice"))
    );
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
        Some("support"),
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
    assert_eq!(value["group"], "support");
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
            "support",
            "--reason",
            "same case",
        ],
    );
    assert_eq!(status, 2);
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "cli_error");

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
    assert_eq!(parse_one(&stdout)[0]["created_case"], false);
    let messages = fs::read_to_string(case_path.join("data/case.json"));
    assert!(messages.is_ok());
    let messages: Result<Value, _> = serde_json::from_str(&messages.unwrap_or_default());
    assert!(messages.is_ok());
    let messages = messages.unwrap_or(Value::Null);
    assert_eq!(messages["items"].as_array().map(|a| a.len()), Some(2));

    let (status, stdout) = run(
        &root,
        &["message", "attachment", "fetch", "message_001", "2"],
    );
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["storage"], "message_cache");
    assert!(value.get("download_marked").is_some());
    assert!(
        root.join(".afmail/messages/message_001.files/pricing.txt")
            .is_file()
    );
    assert!(!case_path.join("files/pricing.txt").is_file());
    let message_view =
        fs::read_to_string(case_path.join("views/messages/message_001.md")).unwrap_or_default();
    assert!(message_view.contains("fetched: `.afmail/messages/message_001.files/pricing.txt`"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spam_attachment_fetch_is_blocked() {
    let root = temp_root("spam-attachment-fetch");
    assert!(fs::create_dir_all(&root).is_ok());
    assert_eq!(run(&root, &["init"]).0, 0);
    assert!(fs::create_dir_all(root.join("source")).is_ok());
    assert!(fs::write(root.join("source/malware.txt"), "payload").is_ok());

    write_message(
        &root,
        "message_spam_attachment",
        1,
        Some("source/malware.txt"),
    );
    write_triage(&root, "message_spam_attachment", "spam attachment");
    let (status, stdout) = run(
        &root,
        &[
            "message",
            "spam",
            "message_spam_attachment",
            "--reason",
            "unsafe attachment",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        &root,
        &[
            "message",
            "attachment",
            "fetch",
            "message_spam_attachment",
            "2",
        ],
    );
    assert_eq!(status, 1, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["code"], "attachment_fetch_blocked");
    assert!(
        !root
            .join(".afmail/messages/message_spam_attachment.files/malware.txt")
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}
