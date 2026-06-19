#[cfg(feature = "ui")]
use agent_first_ui::{
    prepare_user_action, validate_document, AfuiMessage, PrepareUserActionOptions, PreparedStatus,
};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "support/fixtures.rs"]
mod test_fixtures;

const GREENMAIL_SMTP_PORT: &str = "3025/tcp";
const GREENMAIL_IMAP_PORT: &str = "3143/tcp";
const GREENMAIL_IMAGE: &str = "greenmail/standalone:2.1.8";
const MAIL_DOMAIN: &str = "localhost";
const ME_LOGIN: &str = "me";
const ALICE_LOGIN: &str = "alice";
const PASSWORD: &str = "secret";

#[test]
fn mail_batch_fixture_manifest_is_valid() -> Result<(), Box<dyn std::error::Error>> {
    let batch = test_fixtures::load_mail_batch().map_err(fixture_error)?;
    let problems = batch.validate();
    assert!(
        problems.is_empty(),
        "mail batch fixture is invalid: {problems:#?}"
    );
    let primary = batch.primary_account();
    assert!(primary.is_some());
    if let Some(primary) = primary {
        assert_eq!(primary.login, "me");
        assert_eq!(primary.password_secret, "secret");
        assert_eq!(primary.address, "me@localhost");
        assert_eq!(primary.display_name, "Me");
    }
    let alice = batch.account("alice");
    assert!(alice.is_some());
    if let Some(alice) = alice {
        assert_eq!(alice.login, "alice");
        assert_eq!(alice.password_secret, "secret");
        assert_eq!(alice.address, "alice@localhost");
        assert_eq!(alice.display_name, "Alice Customer");
    }
    assert_eq!(batch.messages.len(), 30);
    Ok(())
}

#[test]
#[ignore]
fn docker_greenmail_fixture_batch_e2e() -> Result<(), Box<dyn std::error::Error>> {
    let batch = test_fixtures::load_mail_batch().map_err(fixture_error)?;
    let problems = batch.validate();
    assert!(
        problems.is_empty(),
        "mail batch fixture is invalid: {problems:#?}"
    );

    let suffix = unique_suffix();
    let container_name = format!("afmail-fixture-e2e-greenmail-{suffix}");
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
            GREENMAIL_IMAGE,
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
    create_fixture_remote_folders(imap_port, &batch);
    seed_fixture_batch(imap_port, &batch).map_err(fixture_error)?;

    let root = TempRoot::new("greenmail-fixture-e2e");
    assert!(fs::create_dir_all(root.path()).is_ok());
    assert_eq!(run(root.path(), &["init"]).0, 0);
    write_json(
        &root.path().join(".afmail/config.json"),
        &test_config(imap_port, smtp_port),
    );

    let (status, stdout) = run(root.path(), &["remote", "test"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "remote_test_result");

    let (status, stdout) = run(root.path(), &["pull"]);
    assert_eq!(status, 0, "{stdout}");
    let pull = parse_one(&stdout);
    assert_eq!(pull["code"], "pull_result");
    assert_eq!(pull["new_message_count"], 30);
    assert_eq!(pull["triage_created_count"], 28);
    assert_eq!(pull["spam_message_count"], 1);
    assert_eq!(pull["trashed_message_count"], 1);

    assert_fixture_imports(root.path(), &batch);

    let refund_initial = message_id_by_fixture(root.path(), &batch, "refund_initial");
    let refund_followup = message_id_by_fixture(root.path(), &batch, "refund_followup");
    let receipt_cloud = message_id_by_fixture(root.path(), &batch, "receipt_cloud");
    let inbox_phish = message_id_by_fixture(root.path(), &batch, "inbox_password_phish");
    let inbox_duplicate = message_id_by_fixture(root.path(), &batch, "inbox_duplicate_alert");
    let attachment_id = message_id_by_fixture(root.path(), &batch, "contract_attachment");

    let (status, stdout) = run(
        root.path(),
        &["message", "attachment", "fetch", &attachment_id],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "attachments_saved");

    let (status, stdout) = run(
        root.path(),
        &[
            "case",
            "create",
            "--name",
            "refund-order-4471",
            "--message",
            &refund_initial,
            "--group",
            "open",
            "--reason",
            "customer refund thread needs a reply",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let case_created = parse_one(&stdout);
    let case_uid = case_created["case_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let case_path = case_created["case_path"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!case_uid.is_empty(), "{case_created}");
    assert!(!case_path.is_empty(), "{case_created}");

    let (status, stdout) = run(
        root.path(),
        &[
            "case",
            "add",
            &case_uid,
            &refund_followup,
            "--reason",
            "same refund thread",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    let (status, stdout) = run(
        root.path(),
        &["case", "draft", "reply", &case_uid, &refund_followup],
    );
    assert_eq!(status, 0, "{stdout}");
    let reply = parse_one(&stdout);
    let draft_name = reply["draft_name"].as_str().unwrap_or_default().to_string();
    assert!(!draft_name.is_empty(), "{reply}");
    let draft_path = root
        .path()
        .join(&case_path)
        .join("drafts")
        .join(&draft_name);
    write_demo_reply_draft(&draft_path, &case_uid, &refund_followup);

    let (status, stdout) = run(
        root.path(),
        &["case", "draft", "validate", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(
        root.path(),
        &["case", "draft", "send", &case_uid, &draft_name],
    );
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_queued");

    let (status, stdout) = run(
        root.path(),
        &[
            "archive",
            "message",
            "create",
            "--name",
            "receipts",
            "--message",
            &receipt_cloud,
            "--summary",
            "cloud service receipt",
            "--reason",
            "receipt can be filed",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let archive_created = parse_one(&stdout);
    assert_eq!(archive_created["code"], "archive_message_created");
    let archive_uid = archive_created["archive_uid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!archive_uid.is_empty(), "{archive_created}");

    let (status, stdout) = run(
        root.path(),
        &[
            "message",
            "spam",
            &inbox_phish,
            "--reason",
            "obvious phishing in inbox",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let (status, stdout) = run(
        root.path(),
        &[
            "message",
            "trash",
            &inbox_duplicate,
            "--reason",
            "duplicate notification from inbox",
        ],
    );
    assert_eq!(status, 0, "{stdout}");

    #[cfg(feature = "ui")]
    assert_fixture_ui_snapshot(root.path(), &attachment_id, &case_uid, &archive_uid);

    let (status, stdout) = run(root.path(), &["push", "--dry-run"]);
    assert_eq!(status, 0, "{stdout}");
    assert_eq!(parse_one(&stdout)["code"], "push_dry_run");

    let (status, stdout) = run(root.path(), &["push", "--confirm"]);
    assert_eq!(status, 0, "{stdout}");
    let pushed = parse_one(&stdout);
    assert_eq!(pushed["code"], "push_result");
    assert_eq!(pushed["failed_count"], 0, "{pushed}");

    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ALICE_LOGIN,
            PASSWORD,
            "INBOX",
            "shipping refund has been approved"
        )),
        "recipient mailbox did not receive the fixture reply"
    );
    assert!(
        mailbox_contains(imap_port, ME_LOGIN, PASSWORD, "Archive", "Receipt INV-1001"),
        "receipt was not moved into Archive"
    );
    assert!(
        mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "Junk",
            "mailbox password expires"
        ),
        "inbox phishing message was not moved into Junk"
    );
    assert!(
        mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "Trash",
            "Duplicate notification"
        ),
        "inbox duplicate message was not moved into Trash"
    );

    let (status, stdout) = run(root.path(), &["pull"]);
    assert_eq!(status, 0, "{stdout}");
    let second_pull = parse_one(&stdout);
    assert_eq!(second_pull["new_message_count"], 0);
    assert_eq!(push_json_count(root.path()), 0);
    Ok(())
}

#[test]
#[ignore]
fn docker_greenmail_pull_reply_send_e2e() {
    let suffix = unique_suffix();
    let container_name = format!("afmail-e2e-greenmail-{suffix}");
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
            GREENMAIL_IMAGE,
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

    // GreenMail's `setup.test.all` only provisions INBOX, so create the standard
    // mail folders the workspace config pins by name (drafts/sent/archive/junk/
    // trash). GreenMail doesn't advertise RFC 6154 special-use, so these plain
    // named folders are what pull resolves against.
    for folder in ["Drafts", "Sent", "Archive", "Junk", "Trash"] {
        create_remote_folder(imap_port, ME_LOGIN, PASSWORD, folder);
    }

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
    let (status, stdout) = run(
        root.path(),
        &["case", "draft", "send", &case_uid, "reply.md"],
    );
    assert_eq!(status, 0, "{stdout}");
    let queued = parse_one(&stdout);
    assert_eq!(queued["code"], "push_queued");
    let push_id = queued["push_id"].as_str().unwrap_or_default();
    let outbound_message_id = format!(
        "message_sent_{}",
        push_id.strip_prefix("push_").unwrap_or(push_id)
    );
    assert!(!outbound_message_id.is_empty());

    let (status, stdout) = run(root.path(), &["push", "--confirm"]);
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
            "spam",
            &old_spam_message_id,
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

    let (status, stdout) = run(root.path(), &["push", "--confirm"]);
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
            // GreenMail does not advertise RFC 6154 special-use attributes, so
            // pin folders by name (pull resolves mailbox_name directly). The
            // matching folders are created over IMAP in the test setup.
            "inbox": {"mailbox_name": "INBOX", "special_use": null},
            "sent": {"mailbox_name": "Sent", "special_use": null},
            "archive": {"mailbox_name": "Archive", "special_use": null},
            "junk": {"mailbox_name": "Junk", "special_use": null},
            "trash": {"mailbox_name": "Trash", "special_use": null},
            "drafts": {"mailbox_name": "Drafts", "special_use": null}
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
            // case.add queues a "case" push item that no `push` subcommand
            // drains (categories are drafts/archive/spam/trash), so keep it
            // effect-free — matching afmail's default — or the queue never empties.
            "case.add": {"steps": []},
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
        "identities": [
            {"identity": "me", "name": "Me", "email": "me@localhost", "default": true}
        ],
        "audit": {"reason_mode": "required"},
        "smtp": {
            "host": "127.0.0.1",
            "port": smtp_port,
            "starttls": false,
            "tls_wrapper": false,
            "username": null,
            "password_secret": null
        },
        "workspace": {"language_bcp47": null, "timezone_utc_offset": "UTC"}
    })
}

fn fixture_error(err: String) -> std::io::Error {
    std::io::Error::other(err)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_default()).unwrap_or(Value::Null)
}

fn create_fixture_remote_folders(imap_port: u16, batch: &test_fixtures::MailBatch) {
    for mailbox in batch.mailboxes.values() {
        if mailbox.mailbox_name != "INBOX" {
            create_remote_folder(imap_port, ME_LOGIN, PASSWORD, &mailbox.mailbox_name);
        }
    }
}

fn seed_fixture_batch(imap_port: u16, batch: &test_fixtures::MailBatch) -> Result<(), String> {
    let mut session = imap_login(imap_port, ME_LOGIN, PASSWORD)
        .ok_or_else(|| "connect to GreenMail IMAP for fixture seed failed".to_string())?;
    for message in &batch.messages {
        let folder = batch.mailbox_name(&message.mailbox_id).ok_or_else(|| {
            format!(
                "fixture {} references unknown mailbox_id {}",
                message.fixture_id, message.mailbox_id
            )
        })?;
        let raw = batch.raw_message(message)?;
        let appended = session.append(folder, raw.as_slice());
        if let Err(err) = appended {
            let _ = session.logout();
            return Err(format!(
                "append fixture {} to {folder} failed: {err}",
                message.fixture_id
            ));
        }
    }
    let _ = session.logout();
    Ok(())
}

fn assert_fixture_imports(root: &Path, batch: &test_fixtures::MailBatch) {
    for message in &batch.messages {
        let message_id = message_id_by_subject(root, &message.expected.subject);
        let json = read_json(&root.join(format!("messages/{message_id}.json")));
        assert_eq!(json["subject"], message.expected.subject, "{json}");
        assert_eq!(json["direction"], message.expected.direction, "{json}");
        assert_eq!(
            json["workspace"]["status"], message.expected.workspace_status,
            "{json}"
        );
        let expected_mailbox = batch.mailbox_name(&message.mailbox_id).unwrap_or_default();
        let has_expected_location = json["remote"]["locations"]
            .as_array()
            .map(|locations| {
                locations
                    .iter()
                    .any(|location| location["mailbox_name"].as_str() == Some(expected_mailbox))
            })
            .unwrap_or(false);
        assert!(
            has_expected_location,
            "{} was not imported from {expected_mailbox}: {json}",
            message.fixture_id
        );

        match message.expected.workspace_status.as_str() {
            "triage" => assert!(
                root.join(format!("triage/{message_id}.md")).is_file(),
                "missing triage view for {}",
                message.fixture_id
            ),
            "spam" => assert!(
                root.join(format!("spam/{message_id}.md")).is_file(),
                "missing spam view for {}",
                message.fixture_id
            ),
            "trashed" => assert!(
                root.join(format!("trash/{message_id}.md")).is_file(),
                "missing trash view for {}",
                message.fixture_id
            ),
            _ => {}
        }
    }
}

#[cfg(feature = "ui")]
fn assert_fixture_ui_snapshot(
    root: &Path,
    triage_message_id: &str,
    case_uid: &str,
    archive_uid: &str,
) {
    let (status, stdout) = run(root, &["ui", "snapshot"]);
    assert_eq!(status, 0, "{stdout}");
    let value = parse_one(&stdout);
    assert_eq!(value["type"], "ui_snapshot");
    let message = serde_json::from_value::<AfuiMessage>(value);
    assert!(message.is_ok(), "{message:?}");
    let Ok(message) = message else {
        return;
    };
    let snapshot = message.as_snapshot();
    assert!(snapshot.is_ok(), "{snapshot:?}");
    let Ok(snapshot) = snapshot else {
        return;
    };
    let validation = validate_document(&snapshot.document);
    assert!(validation.is_ok(), "{validation:?}");

    for view_id in [
        "mailbox_status",
        "inbox_triage",
        "active_cases",
        "push_queue",
        "message_archives",
        "case_archives",
        "activity_log",
        "mail_terminal",
    ] {
        assert!(
            snapshot.document.find_view(view_id).is_some(),
            "{view_id} missing"
        );
    }
    let pulse = snapshot.document.find_view("mailbox_status");
    assert!(pulse.is_some());
    let Some(pulse) = pulse else {
        return;
    };
    assert_eq!(pulse.field("title"), Some(&json!("Mailbox Status")));
    assert_eq!(
        pulse.field("dockview"),
        Some(&json!({
            "role": "utility",
            "priority": 30,
            "preferred_height": 170
        }))
    );

    let triage = snapshot.document.find_view("inbox_triage");
    assert!(triage.is_some());
    let Some(triage) = triage else {
        return;
    };
    let triage_items = snapshot.state.as_value()["ui"]["triage_items"].as_array();
    assert!(
        triage_items.is_some(),
        "{:?}",
        snapshot.state.as_value()["ui"]
    );
    let Some(triage_items) = triage_items else {
        return;
    };
    assert!(
        triage_items.len() >= 20,
        "UI snapshot should reflect the 30-message Docker fixture corpus"
    );
    let triage_item = triage_items
        .iter()
        .find(|item| item["message"]["message_id"].as_str() == Some(triage_message_id));
    assert!(triage_item.is_some(), "{triage_items:?}");
    let Some(triage_item) = triage_item else {
        return;
    };
    let prepared = prepare_user_action(
        &snapshot,
        Some(triage),
        "afmail.message.trash",
        Some(triage_item),
        &PrepareUserActionOptions::default(),
    );
    assert!(prepared.is_ok(), "{prepared:?}");
    let Ok(prepared) = prepared else {
        return;
    };
    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input["message_id"], triage_message_id);

    let cases = snapshot.document.find_view("active_cases");
    assert!(cases.is_some());
    let Some(cases) = cases else {
        return;
    };
    let case_items = snapshot.state.as_value()["ui"]["case_items"].as_array();
    assert!(
        case_items.is_some(),
        "{:?}",
        snapshot.state.as_value()["ui"]
    );
    let Some(case_items) = case_items else {
        return;
    };
    let case_item = case_items
        .iter()
        .find(|item| item["case"]["collection_uid"].as_str() == Some(case_uid));
    assert!(case_item.is_some(), "{case_items:?}");
    let Some(case_item) = case_item else {
        return;
    };
    let prepared = prepare_user_action(
        &snapshot,
        Some(cases),
        "afmail.case.show",
        Some(case_item),
        &PrepareUserActionOptions::default(),
    );
    assert!(prepared.is_ok(), "{prepared:?}");
    let Ok(prepared) = prepared else {
        return;
    };
    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input["case_ref"], case_uid);

    let archives = snapshot.document.find_view("message_archives");
    assert!(archives.is_some());
    let Some(archives) = archives else {
        return;
    };
    let archive_items = snapshot.state.as_value()["ui"]["archive_message_items"].as_array();
    assert!(
        archive_items.is_some(),
        "{:?}",
        snapshot.state.as_value()["ui"]
    );
    let Some(archive_items) = archive_items else {
        return;
    };
    let archive_item = archive_items
        .iter()
        .find(|item| item["archive"]["collection_uid"].as_str() == Some(archive_uid));
    assert!(archive_item.is_some(), "{archive_items:?}");
    let Some(archive_item) = archive_item else {
        return;
    };
    let prepared = prepare_user_action(
        &snapshot,
        Some(archives),
        "afmail.archive.message.show",
        Some(archive_item),
        &PrepareUserActionOptions::default(),
    );
    assert!(prepared.is_ok(), "{prepared:?}");
    let Ok(prepared) = prepared else {
        return;
    };
    assert_eq!(prepared.status, PreparedStatus::Ready);
    assert_eq!(prepared.input["archive_ref"], archive_uid);
}

fn message_id_by_fixture(
    root: &Path,
    batch: &test_fixtures::MailBatch,
    fixture_id: &str,
) -> String {
    let message = batch.message(fixture_id);
    assert!(message.is_some(), "fixture_id {fixture_id} not found");
    let Some(message) = message else {
        return String::new();
    };
    message_id_by_subject(root, &message.expected.subject)
}

fn write_demo_reply_draft(path: &Path, case_uid: &str, reply_to_message_id: &str) {
    let draft = format!(
        "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: reply\nreply_to_message_id: {reply_to_message_id}\nto:\n  - Alice Customer <alice@localhost>\ncc: []\nsubject: \"Re: Refund request for order 4471\"\nattachments:\n---\n\nHi Alice,\n\nThe shipping refund has been approved. I am sorry the cold pack arrived warm, and I have added a note to the order.\n\nBest,\nMe\n"
    );
    assert!(fs::write(path, draft).is_ok());
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

// Create an IMAP mailbox if it doesn't already exist (idempotent: an
// "already exists" error from a re-run is ignored).
fn create_remote_folder(port: u16, username: &str, password: &str, folder: &str) {
    if let Some(mut session) = imap_login(port, username, password) {
        let _ = session.create(folder);
        let _ = session.logout();
    }
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

// Enumerate every local message id from its durable `.eml` record. `direction`
// and `subject` come from `message show`, so helpers pair this with that command.
fn message_ids(root: &Path) -> Vec<String> {
    let mut ids: Vec<String> = fs::read_dir(root.join(".afmail/messages"))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    entry
                        .file_name()
                        .to_str()?
                        .strip_suffix(".eml")
                        .map(ToString::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

// Resolve the message ids matching a `message show` field (e.g. direction or
// subject), expecting exactly one.
fn message_ids_where(root: &Path, field: &str, expected: &str) -> Vec<String> {
    message_ids(root)
        .into_iter()
        .filter(|id| {
            let (status, stdout) = run(root, &["message", "show", id]);
            status == 0 && parse_one(&stdout)[field].as_str() == Some(expected)
        })
        .collect()
}

fn single_message_id(root: &Path, direction: &str) -> String {
    let ids = message_ids_where(root, "direction", direction);
    assert_eq!(ids.len(), 1, "expected one {direction} message: {ids:?}");
    ids.into_iter().next().unwrap_or_default()
}

fn message_id_by_subject(root: &Path, subject: &str) -> String {
    let ids = message_ids_where(root, "subject", subject);
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

fn minimal_config() -> Value {
    json!({
        "schema_name": "config",
        "schema_version": 1,
        "imap": {
            "host": null,
            "port": 993,
            "tls": true,
            "username": null,
            "password_secret": null
        },
        "mailboxes": {
            "inbox": {"mailbox_name": "INBOX", "special_use": null},
            "sent": {"special_use": "\\Sent", "mailbox_name": null},
            "archive": {"special_use": "\\Archive", "mailbox_name": null},
            "junk": {"special_use": "\\Junk", "mailbox_name": null},
            "trash": {"special_use": "\\Trash", "mailbox_name": null},
            "drafts": {"special_use": "\\Drafts", "mailbox_name": null}
        },
        "actions": {
            "pull": {
                "default_mailbox_ids": ["inbox"],
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
            "draft.save": {"steps": []},
            "draft.send": {"steps": []},
            "message.spam": {"steps": []},
            "message.trash": {"steps": []},
            "message.archive": {"by_source_mailbox_id": {
                "inbox": {"steps": []},
                "sent": {"steps": []},
                "archive": {"steps": []},
                "junk": {"steps": []},
                "trash": {"steps": []},
                "drafts": {"steps": []}
            }}
        },
        "case": {"default_group": "open"},
        "contact": {"default_group": "people"},
        "identities": [
            {"identity": "me", "name": "Me", "email": "me@example.com", "default": true}
        ],
        "audit": {"reason_mode": "optional"},
        "smtp": {
            "host": null,
            "port": 587,
            "starttls": true,
            "tls_wrapper": false,
            "username": null,
            "password_secret": null
        },
        "workspace": {"language_bcp47": null, "timezone_utc_offset": "UTC"}
    })
}

#[test]
fn contact_crud_local_e2e() {
    let root = TempRoot::new("contact-crud");
    assert!(fs::create_dir_all(root.path()).is_ok());

    // Step 1: init workspace
    let (status, stdout) = run(root.path(), &["init"]);
    assert_eq!(status, 0, "init failed: {stdout}");
    write_json(&root.path().join(".afmail/config.json"), &minimal_config());

    // Step 2: create first contact
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "create",
            "--name",
            "Zhang San",
            "--email",
            "zhang@example.com",
        ],
    );
    assert_eq!(status, 0, "contact create failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["code"], "contact_created");
    let p001 = result["contact_uid"].as_str().unwrap_or("").to_string();
    assert!(p001.starts_with('p'), "uid should start with p: {p001}");
    assert_eq!(result["emails"], json!(["zhang@example.com"]));
    // Contact file should exist
    let contacts_dir = root.path().join("contacts").join("people");
    assert!(contacts_dir.exists(), "contacts/people dir missing");
    let contact_file_count = fs::read_dir(&contacts_dir)
        .ok()
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    assert_eq!(contact_file_count, 1);

    // Step 3: contact list
    let (status, stdout) = run(root.path(), &["contact", "list"]);
    assert_eq!(status, 0, "contact list failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["count"], 1);
    assert_eq!(result["contacts"][0]["contact_uid"], p001);

    // Step 4: contact show
    let (status, stdout) = run(root.path(), &["contact", "show", &p001]);
    assert_eq!(status, 0, "contact show failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["code"], "contact_show");
    assert!(result["content"]
        .as_str()
        .unwrap_or("")
        .contains("Zhang San"));

    // Step 5: tag
    let (status, stdout) = run(root.path(), &["contact", "tag", &p001, "vip"]);
    assert_eq!(status, 0, "contact tag failed: {stdout}");
    let result = parse_one(&stdout);
    assert!(result["tags"]
        .as_array()
        .is_some_and(|t| t.contains(&json!("vip"))));

    // Step 6: untag
    let (status, stdout) = run(root.path(), &["contact", "untag", &p001, "vip"]);
    assert_eq!(status, 0, "contact untag failed: {stdout}");
    let result = parse_one(&stdout);
    assert!(!result["tags"]
        .as_array()
        .is_some_and(|t| t.contains(&json!("vip"))));

    // Step 7: notes show
    let (status, _) = run(root.path(), &["contact", "notes", "show", &p001]);
    assert_eq!(status, 0, "contact notes show failed");

    // Step 8: notes append
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "notes",
            "append",
            &p001,
            "--text",
            "some notes here",
        ],
    );
    assert_eq!(status, 0, "contact notes append failed: {stdout}");

    // Step 9: notes replace
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "notes",
            "replace",
            &p001,
            "--text",
            "replaced content",
        ],
    );
    assert_eq!(status, 0, "contact notes replace failed: {stdout}");
    // Verify replace took effect
    let (_, show_stdout) = run(root.path(), &["contact", "notes", "show", &p001]);
    let show_result = parse_one(&show_stdout);
    assert!(
        show_result["notes"]
            .as_str()
            .unwrap_or("")
            .contains("replaced content"),
        "notes should contain replaced content"
    );

    // Step 10: rename
    let (status, stdout) = run(
        root.path(),
        &["contact", "rename", &p001, "--name", "Zhang Sanfeng"],
    );
    assert_eq!(status, 0, "contact rename failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["display_name"], "Zhang Sanfeng");
    // Original ref still resolves
    let (status, _) = run(root.path(), &["contact", "show", &p001]);
    assert_eq!(status, 0, "ref after rename failed");

    // Step 11: email add
    let (status, stdout) = run(
        root.path(),
        &["contact", "email", "add", &p001, "zhang-extra@example.com"],
    );
    assert_eq!(status, 0, "contact email add failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["emails"].as_array().map(|a| a.len()), Some(2));

    // Step 12: email remove
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "email",
            "remove",
            &p001,
            "zhang-extra@example.com",
        ],
    );
    assert_eq!(status, 0, "contact email remove failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["emails"].as_array().map(|a| a.len()), Some(1));

    // Step 13: phone add
    let (status, stdout) = run(
        root.path(),
        &["contact", "phone", "add", &p001, "13800138000"],
    );
    assert_eq!(status, 0, "contact phone add failed: {stdout}");
    let result = parse_one(&stdout);
    assert_eq!(result["phones"].as_array().map(|a| a.len()), Some(1));

    // Step 14: phone remove
    let (status, stdout) = run(
        root.path(),
        &["contact", "phone", "remove", &p001, "13800138000"],
    );
    assert_eq!(status, 0, "contact phone remove failed: {stdout}");
    let result = parse_one(&stdout);
    assert!(result["phones"].as_array().is_some_and(|a| a.is_empty()));

    // Step 15: email uniqueness is enforced
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "create",
            "--name",
            "Li Si",
            "--email",
            "zhang@example.com",
        ],
    );
    assert_ne!(status, 0, "duplicate email should fail");
    assert!(
        stdout.contains("email_conflict"),
        "error should say email_conflict: {stdout}"
    );

    // Step 16: create second contact with different email
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "create",
            "--name",
            "Li Si",
            "--email",
            "lisi@example.com",
        ],
    );
    assert_eq!(status, 0, "second contact create failed: {stdout}");
    let result = parse_one(&stdout);
    let p002 = result["contact_uid"].as_str().unwrap_or("").to_string();
    assert!(p002.starts_with('p') && p002 != p001);

    // Step 17: move p001 to vip group
    let (status, stdout) = run(root.path(), &["contact", "move", &p001, "--group", "vip"]);
    assert_eq!(status, 0, "contact move failed: {stdout}");
    assert!(root.path().join("contacts").join("vip").exists());
    // ref still resolves
    let (status, _) = run(root.path(), &["contact", "show", &p001]);
    assert_eq!(status, 0, "show after move failed");

    // Step 18: archive p001
    let (status, stdout) = run(root.path(), &["contact", "archive", &p001]);
    assert_eq!(status, 0, "contact archive failed: {stdout}");
    assert_eq!(parse_one(&stdout)["code"], "contact_archived");
    assert!(root.path().join("archive").join("contacts").exists());
    let (_, list_stdout) = run(root.path(), &["contact", "list"]);
    let list = parse_one(&list_stdout);
    assert_eq!(list["count"], 1, "only p002 should be active");

    // Step 19: reopen p001
    let (status, stdout) = run(root.path(), &["contact", "reopen", &p001]);
    assert_eq!(status, 0, "contact reopen failed: {stdout}");
    assert_eq!(parse_one(&stdout)["code"], "contact_reopened");
    let (_, list_stdout) = run(root.path(), &["contact", "list"]);
    let list = parse_one(&list_stdout);
    assert_eq!(list["count"], 2, "both contacts should be active again");
}

#[test]
#[ignore]
fn docker_greenmail_contact_cards_e2e() {
    let suffix = unique_suffix();
    let container_name = format!("afmail-e2e-contact-{suffix}");
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
            GREENMAIL_IMAGE,
        ],
        "start GreenMail container for contact e2e",
    );
    let smtp_port = docker_mapped_port(&container_name, GREENMAIL_SMTP_PORT);
    let imap_port = docker_mapped_port(&container_name, GREENMAIL_IMAP_PORT);
    assert!(
        wait_until(Duration::from_secs(45), || imap_login_works(
            imap_port, ME_LOGIN, PASSWORD
        )),
        "GreenMail IMAP did not become ready"
    );

    let root = TempRoot::new("greenmail-contact");
    assert!(fs::create_dir_all(root.path()).is_ok());
    assert_eq!(run(root.path(), &["init"]).0, 0);
    let mut config = test_config(imap_port, smtp_port);
    config["contact"] = json!({"default_group": "people"});
    config["audit"] = json!({"reason_mode": "optional"});
    write_json(&root.path().join(".afmail/config.json"), &config);

    for folder in ["Drafts", "Sent", "Archive", "Junk", "Trash"] {
        create_remote_folder(imap_port, ME_LOGIN, PASSWORD, folder);
    }

    // Seed a message from alice@localhost
    seed_inbound_message(smtp_port, &suffix);
    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "INBOX",
            "Hello from real GreenMail"
        )),
        "seeded message from alice not visible"
    );

    // Step 1: pull
    let (status, stdout) = run(root.path(), &["pull"]);
    assert_eq!(status, 0, "pull failed: {stdout}");
    let alice_msg_id = single_message_id(root.path(), "inbound");

    // Step 2: create contact for alice
    let (status, stdout) = run(
        root.path(),
        &[
            "contact",
            "create",
            "--name",
            "Alice Customer",
            "--email",
            "alice@localhost",
        ],
    );
    assert_eq!(status, 0, "contact create alice failed: {stdout}");
    let result = parse_one(&stdout);
    let p001 = result["contact_uid"].as_str().unwrap_or("").to_string();

    // Step 3: message show carries contact enrichment with no rebuild —
    //         `contact create` auto-updates matching messages.
    let (status, stdout) = run(root.path(), &["message", "show", &alice_msg_id]);
    assert_eq!(status, 0, "message show failed: {stdout}");
    let msg = parse_one(&stdout);
    assert_eq!(
        msg["contact"]["contact_uid"], p001,
        "message show should have contact.contact_uid"
    );
    assert_eq!(msg["contact"]["display_name"], "Alice Customer");

    // Step 4: create a case from alice's message; the contact link stays
    //         materialized on the message after it moves into the case.
    let (status, stdout) = run(
        root.path(),
        &[
            "case",
            "create",
            "--name",
            "alice-inquiry",
            "--message",
            &alice_msg_id,
            "--group",
            "open",
        ],
    );
    assert_eq!(status, 0, "case create failed: {stdout}");
    let (status, stdout) = run(root.path(), &["message", "show", &alice_msg_id]);
    assert_eq!(status, 0, "message show after case create failed: {stdout}");
    assert_eq!(
        parse_one(&stdout)["contact"]["contact_uid"],
        p001,
        "contact link should survive case creation"
    );

    // Step 5: seed a second message from bob@external.example
    let bob_subject = format!("bob-subject-{suffix}");
    let bob_raw = format!(
        "Message-ID: <bob-{suffix}@external.example>\r\nFrom: Bob External <bob@external.example>\r\nTo: Me <me@localhost>\r\nDate: Thu, 21 May 2026 11:00:00 +0000\r\nSubject: {bob_subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello from bob\r\n"
    );
    smtp_send(
        smtp_port,
        "bob@external.example",
        "me@localhost",
        bob_raw.as_bytes(),
    );
    assert!(
        wait_until(Duration::from_secs(15), || mailbox_contains(
            imap_port,
            ME_LOGIN,
            PASSWORD,
            "INBOX",
            &bob_subject
        )),
        "bob's seeded message not visible"
    );

    let (status, stdout) = run(root.path(), &["pull"]);
    assert_eq!(status, 0, "second pull failed: {stdout}");
    let bob_msg_ids = message_ids_where(root.path(), "subject", &bob_subject);
    assert_eq!(
        bob_msg_ids.len(),
        1,
        "expected one bob message: {bob_msg_ids:?}"
    );
    let bob_msg_id = bob_msg_ids[0].clone();

    // Step 6: extract a contact stub for bob from triage; extract auto-updates
    //         bob's message link.
    let (status, stdout) = run(root.path(), &["contact", "extract", "--from-triage"]);
    assert_eq!(status, 0, "contact extract failed: {stdout}");
    let extract = parse_one(&stdout);
    assert_eq!(
        extract["created_count"], 1,
        "should have created 1 stub contact for bob"
    );
    let p002 = extract["created_uids"][0]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(p002.starts_with('p'));

    // Step 7: bob's message now shows the extracted contact, no rebuild needed.
    let (status, stdout) = run(root.path(), &["message", "show", &bob_msg_id]);
    assert_eq!(status, 0, "bob message show failed: {stdout}");
    assert_eq!(
        parse_one(&stdout)["contact"]["contact_uid"],
        p002,
        "bob's message should link to the extracted contact"
    );

    // Step 8: create a case for bob; the link survives.
    let (status, stdout) = run(
        root.path(),
        &[
            "case",
            "create",
            "--name",
            "bob-inquiry",
            "--message",
            &bob_msg_id,
            "--group",
            "open",
        ],
    );
    assert_eq!(status, 0, "case create for bob failed: {stdout}");
    let (status, stdout) = run(root.path(), &["message", "show", &bob_msg_id]);
    assert_eq!(status, 0, "bob message show after case failed: {stdout}");
    assert_eq!(
        parse_one(&stdout)["contact"]["contact_uid"],
        p002,
        "bob's case message should keep contact = p002"
    );

    // Step 9: contact list shows both
    let (status, stdout) = run(root.path(), &["contact", "list"]);
    assert_eq!(status, 0, "contact list failed: {stdout}");
    assert_eq!(parse_one(&stdout)["count"], 2);

    // Step 10: rename alice; the message's contact name snapshot refreshes.
    let (status, stdout) = run(
        root.path(),
        &["contact", "rename", &p001, "--name", "Alice VIP"],
    );
    assert_eq!(status, 0, "contact rename failed: {stdout}");
    let (status, stdout) = run(root.path(), &["message", "show", &alice_msg_id]);
    assert_eq!(status, 0, "message show after rename failed: {stdout}");
    assert_eq!(
        parse_one(&stdout)["contact"]["display_name"],
        "Alice VIP",
        "rename should refresh the message's contact name snapshot"
    );

    // Step 11: add second email to alice
    let (status, stdout) = run(
        root.path(),
        &["contact", "email", "add", &p001, "alice-work@localhost"],
    );
    assert_eq!(status, 0, "email add failed: {stdout}");
    assert_eq!(
        parse_one(&stdout)["emails"].as_array().map(|a| a.len()),
        Some(2)
    );

    // Step 12: move p001 to vip group
    let (status, stdout) = run(root.path(), &["contact", "move", &p001, "--group", "vip"]);
    assert_eq!(status, 0, "contact move failed: {stdout}");
    let _ = parse_one(&stdout);
    assert!(root.path().join("contacts").join("vip").exists());

    // Step 13: archive p002 (bob stub)
    let (status, stdout) = run(root.path(), &["contact", "archive", &p002]);
    assert_eq!(status, 0, "contact archive p002 failed: {stdout}");
    assert_eq!(parse_one(&stdout)["code"], "contact_archived");

    // Step 14: active contact list shows only p001
    let (status, stdout) = run(root.path(), &["contact", "list"]);
    assert_eq!(status, 0);
    assert_eq!(parse_one(&stdout)["count"], 1, "only p001 should be active");
}
