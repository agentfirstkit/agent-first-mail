use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAIL_BATCH_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mail-batch");

#[derive(Debug, Deserialize)]
pub struct MailBatch {
    pub schema_name: String,
    pub schema_version: u64,
    pub accounts: BTreeMap<String, FixtureAccount>,
    pub mailboxes: BTreeMap<String, FixtureMailbox>,
    pub messages: Vec<FixtureMessage>,
}

#[derive(Debug, Deserialize)]
pub struct FixtureAccount {
    pub login: String,
    pub password_secret: String,
    pub address: String,
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureMailbox {
    pub mailbox_name: String,
}

#[derive(Debug, Deserialize)]
pub struct FixtureMessage {
    pub fixture_id: String,
    pub eml_path: String,
    pub mailbox_id: String,
    pub rfc822_message_id: String,
    pub expected: FixtureExpected,
}

#[derive(Debug, Deserialize)]
pub struct FixtureExpected {
    pub subject: String,
    pub workspace_status: String,
    pub direction: String,
    pub tags: Vec<String>,
}

pub fn fixture_root() -> PathBuf {
    PathBuf::from(MAIL_BATCH_DIR)
}

pub fn load_mail_batch() -> Result<MailBatch, String> {
    let path = fixture_root().join("manifest.json");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("read fixture manifest {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse fixture manifest: {e}"))
}

impl MailBatch {
    pub fn primary_account(&self) -> Option<&FixtureAccount> {
        self.accounts.get("primary")
    }

    pub fn account(&self, id: &str) -> Option<&FixtureAccount> {
        self.accounts.get(id)
    }

    pub fn mailbox_name(&self, mailbox_id: &str) -> Option<&str> {
        self.mailboxes
            .get(mailbox_id)
            .map(|mailbox| mailbox.mailbox_name.as_str())
    }

    pub fn message(&self, fixture_id: &str) -> Option<&FixtureMessage> {
        self.messages
            .iter()
            .find(|message| message.fixture_id == fixture_id)
    }

    pub fn raw_message(&self, message: &FixtureMessage) -> Result<Vec<u8>, String> {
        let path = fixture_root().join(&message.eml_path);
        fs::read(&path).map_err(|e| format!("read fixture EML {}: {e}", path.display()))
    }

    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.schema_name != "afmail_fixture_mail_batch" {
            problems.push(format!("unexpected schema_name {}", self.schema_name));
        }
        if self.schema_version != 1 {
            problems.push(format!("unexpected schema_version {}", self.schema_version));
        }
        if self.messages.len() != 30 {
            problems.push(format!(
                "expected 30 fixture messages, found {}",
                self.messages.len()
            ));
        }

        let mut fixture_ids = BTreeSet::new();
        let mut message_ids = BTreeSet::new();
        let mut tags = BTreeSet::new();
        for message in &self.messages {
            if !fixture_ids.insert(message.fixture_id.as_str()) {
                problems.push(format!("duplicate fixture_id {}", message.fixture_id));
            }
            if !self.mailboxes.contains_key(&message.mailbox_id) {
                problems.push(format!(
                    "message {} references unknown mailbox_id {}",
                    message.fixture_id, message.mailbox_id
                ));
            }
            let path = fixture_root().join(&message.eml_path);
            if !path.is_file() {
                problems.push(format!("missing EML for {}", message.fixture_id));
                continue;
            }
            match read_rfc822_message_id(&path) {
                Ok(Some(actual)) => {
                    if actual != message.rfc822_message_id {
                        problems.push(format!(
                            "message {} manifest Message-ID {} != EML {}",
                            message.fixture_id, message.rfc822_message_id, actual
                        ));
                    }
                    if !message_ids.insert(actual.clone()) {
                        problems.push(format!("duplicate RFC822 Message-ID {actual}"));
                    }
                }
                Ok(None) => {
                    problems.push(format!("message {} has no Message-ID", message.fixture_id))
                }
                Err(err) => problems.push(err),
            }
            for tag in &message.expected.tags {
                tags.insert(tag.as_str());
            }
        }

        for required in [
            "thread",
            "attachment",
            "spam",
            "spam_candidate",
            "trash",
            "trash_candidate",
            "archive",
            "newsletter",
            "receipt",
            "sent",
            "reply_to",
        ] {
            if !tags.contains(required) {
                problems.push(format!("missing required fixture tag {required}"));
            }
        }
        if !self.messages.iter().any(|message| {
            message.mailbox_id == "inbox"
                && message
                    .expected
                    .tags
                    .iter()
                    .any(|tag| tag == "spam_candidate")
        }) {
            problems.push("missing INBOX spam candidate".to_string());
        }
        if !self.messages.iter().any(|message| {
            message.mailbox_id == "inbox"
                && message
                    .expected
                    .tags
                    .iter()
                    .any(|tag| tag == "trash_candidate")
        }) {
            problems.push("missing INBOX trash candidate".to_string());
        }
        problems
    }
}

fn read_rfc822_message_id(path: &Path) -> Result<Option<String>, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("read fixture header {}: {e}", path.display()))?;
    Ok(text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("message-id") {
            Some(value.trim().to_string())
        } else {
            None
        }
    }))
}
