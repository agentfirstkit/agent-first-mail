mod identity;
mod session;
mod special_use;

use identity::*;
use session::*;
use special_use::*;

pub use special_use::resolve_special_use_from_mailboxes;

use crate::config::{
    ImapConfig, MailConfig, MailDirection, PullImportAs, SpecialUseKind, SpecialUseSource,
    SpecialUseTarget,
};
use crate::error::{AppError, Result};
use crate::imap_client::{
    append_draft_and_find_uid_session, append_message_session, capability_move, create_folder,
    list_mailboxes, login_plain, login_tls, require_move, uid_mark_and_move_session,
    uid_move_session,
};
pub use crate::imap_client::{MailboxInfo, MoveOutcome};
use crate::mail::parse_inbound_message;
use crate::progress::ProgressCallback;
use crate::types::{ImapRef, MessageFile, RemoteLocation, RemoteState};
#[cfg(test)]
use crate::util::write_json_pretty;
use crate::util::{canonical_flags, write_bytes_atomic, write_string_atomic};
use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, Utc};
use mail_parser::{HeaderValue, MessageParser};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullTarget {
    pub id: String,
    pub mailbox: String,
    pub import_as: PullImportAs,
    pub direction: MailDirection,
}

pub fn resolve_pull_targets(
    mail_config: &MailConfig,
    imap: &ImapConfig,
    ids: &[String],
) -> Result<Vec<PullTarget>> {
    let ids = mail_config.selected_pull_ids(ids)?;
    let needs_list = ids.iter().any(|id| {
        mail_config
            .mailbox(id)
            .ok()
            .and_then(|mailbox| mailbox.special_use.as_ref())
            .is_some()
    });
    let mailboxes = if needs_list {
        Some(fetch_mailboxes(imap)?)
    } else {
        None
    };
    let mut targets = Vec::new();
    let mut resolved_names = BTreeSet::new();
    for id in ids {
        let configured = mail_config.mailbox(&id)?;
        let pull_action = mail_config.pull_action(&id)?;
        let mailbox = match configured.mailbox_name.as_deref() {
            Some(mailbox) => mailbox.to_string(),
            None => {
                let special_use = configured.special_use.as_deref().ok_or_else(|| {
                    AppError::new(
                        "config_invalid",
                        format!("mailboxes.{id} is missing mailbox selector"),
                    )
                })?;
                let matches = mailboxes
                    .as_ref()
                    .map(|items| {
                        items
                            .iter()
                            .filter(|mailbox| {
                                mailbox
                                    .attributes
                                    .iter()
                                    .any(|attribute| attribute.eq_ignore_ascii_case(special_use))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                match matches.as_slice() {
                    [mailbox] => mailbox.name.clone(),
                    [] => {
                        return Err(AppError::new(
                            "imap_mailbox_unresolved",
                            format!(
                                "mailboxes.{id}.special_use {special_use} matched no remote mailbox"
                            ),
                        ));
                    }
                    _ => {
                        return Err(AppError::new(
                            "imap_mailbox_ambiguous",
                            format!(
                                "mailboxes.{id}.special_use {special_use} matched multiple remote mailboxes"
                            ),
                        ));
                    }
                }
            }
        };
        if !resolved_names.insert(mailbox.clone()) {
            return Err(AppError::new(
                "imap_mailbox_ambiguous",
                format!("multiple selected mailbox ids resolve to remote mailbox {mailbox}"),
            ));
        }
        targets.push(PullTarget {
            id,
            mailbox,
            import_as: pull_action.import_as,
            direction: pull_action.direction,
        });
    }
    Ok(targets)
}

pub fn pull_workspace(
    root: &Path,
    mail_config: &MailConfig,
    config: &ImapConfig,
    targets: &[PullTarget],
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    let mut progress = progress;
    if config.tls {
        let mut session = login_tls(config)?;
        let result =
            pull_workspace_session(root, mail_config, targets, &mut session, &mut progress);
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result =
            pull_workspace_session(root, mail_config, targets, &mut session, &mut progress);
        let _ = session.logout();
        result
    }
}

pub fn remote_test(config: &ImapConfig) -> Result<Value> {
    let started = Instant::now();
    let move_supported = if config.tls {
        let mut session = login_tls(config)?;
        let move_supported = capability_move(&mut session)?;
        session
            .logout()
            .map_err(|e| AppError::new("imap_logout_failed", e.to_string()))?;
        move_supported
    } else {
        let mut session = login_plain(config)?;
        let move_supported = capability_move(&mut session)?;
        session
            .logout()
            .map_err(|e| AppError::new("imap_logout_failed", e.to_string()))?;
        move_supported
    };
    Ok(json!({
        "code": "remote_test_result",
        "ok": true,
        "host": config.host,
        "port": config.port,
        "tls": config.tls,
        "capabilities": {
            "move": move_supported
        },
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

pub fn remote_folders(config: &MailConfig, imap: &ImapConfig) -> Result<Value> {
    let started = Instant::now();
    let (mailboxes, targets) = if imap.tls {
        let mut session = login_tls(imap)?;
        let result = list_folders_json(config, &mut session);
        let _ = session.logout();
        result?
    } else {
        let mut session = login_plain(imap)?;
        let result = list_folders_json(config, &mut session);
        let _ = session.logout();
        result?
    };
    Ok(json!({
        "code": "remote_mailboxes",
        "mailboxes": mailboxes,
        "special_use_targets": targets,
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

pub fn remote_mkdir(config: &ImapConfig, folder: &str) -> Result<Value> {
    let started = Instant::now();
    if config.tls {
        let mut session = login_tls(config)?;
        create_folder(&mut session, folder)?;
        let _ = session.logout();
    } else {
        let mut session = login_plain(config)?;
        create_folder(&mut session, folder)?;
        let _ = session.logout();
    }
    Ok(json!({
        "code": "remote_mailbox_created",
        "mailbox_name": folder,
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

pub fn resolve_special_use(
    config: &MailConfig,
    imap: &ImapConfig,
    kind: SpecialUseKind,
) -> Result<SpecialUseTarget> {
    let mailboxes = fetch_mailboxes(imap)?;
    Ok(resolve_special_use_from_mailboxes(config, kind, &mailboxes))
}

pub fn resolve_all_pull_folders(config: &MailConfig, imap: &ImapConfig) -> Result<Vec<String>> {
    let mailboxes = if imap.tls {
        let mut session = login_tls(imap)?;
        let result = list_mailboxes(&mut session);
        let _ = session.logout();
        result?
    } else {
        let mut session = login_plain(imap)?;
        let result = list_mailboxes(&mut session);
        let _ = session.logout();
        result?
    };
    let mut folders = Vec::new();
    push_unique_folder(&mut folders, "INBOX".to_string());
    for folder in &imap.mailboxes {
        push_unique_folder(&mut folders, folder.clone());
    }
    for kind in [
        SpecialUseKind::Archive,
        SpecialUseKind::Junk,
        SpecialUseKind::Trash,
        SpecialUseKind::Sent,
        SpecialUseKind::Drafts,
        SpecialUseKind::Flagged,
        SpecialUseKind::All,
    ] {
        let target = resolve_special_use_from_mailboxes(config, kind, &mailboxes);
        if mailboxes
            .iter()
            .any(|mailbox| mailbox.name == target.mailbox_name)
        {
            push_unique_folder(&mut folders, target.mailbox_name);
        }
    }
    Ok(folders)
}

pub fn append_message(
    config: &ImapConfig,
    folder: &str,
    raw_eml: &[u8],
    draft: bool,
) -> Result<Value> {
    let started = Instant::now();
    if config.tls {
        let mut session = login_tls(config)?;
        append_message_session(&mut session, folder, raw_eml, draft)?;
        let _ = session.logout();
    } else {
        let mut session = login_plain(config)?;
        append_message_session(&mut session, folder, raw_eml, draft)?;
        let _ = session.logout();
    }
    Ok(json!({
        "code": "remote_append_result",
        "mailbox_name": folder,
        "draft": draft,
        "size_bytes": raw_eml.len(),
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

pub fn append_draft_and_find_uid(
    config: &ImapConfig,
    folder: &str,
    raw_eml: &[u8],
    rfc822_message_id: &str,
) -> Result<RemoteLocation> {
    if config.tls {
        let mut session = login_tls(config)?;
        let result =
            append_draft_and_find_uid_session(&mut session, folder, raw_eml, rfc822_message_id);
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result =
            append_draft_and_find_uid_session(&mut session, folder, raw_eml, rfc822_message_id);
        let _ = session.logout();
        result
    }
}

pub fn uid_move(
    config: &ImapConfig,
    source_folder: &str,
    uid: u64,
    target_folder: &str,
) -> Result<()> {
    if config.tls {
        let mut session = login_tls(config)?;
        let result = uid_move_session(&mut session, source_folder, uid, target_folder);
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = uid_move_session(&mut session, source_folder, uid, target_folder);
        let _ = session.logout();
        result
    }
}

pub fn uid_mark_and_move(
    config: &ImapConfig,
    source_folder: &str,
    uid: u64,
    target_folder: &str,
    rfc822_message_id: Option<&str>,
    mark_seen: bool,
    keyword: Option<&str>,
) -> Result<MoveOutcome> {
    if config.tls {
        let mut session = login_tls(config)?;
        let result = uid_mark_and_move_session(
            &mut session,
            source_folder,
            uid,
            target_folder,
            rfc822_message_id,
            mark_seen,
            keyword,
        );
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = uid_mark_and_move_session(
            &mut session,
            source_folder,
            uid,
            target_folder,
            rfc822_message_id,
            mark_seen,
            keyword,
        );
        let _ = session.logout();
        result
    }
}

pub fn uid_store_flags(
    config: &ImapConfig,
    source_folder: &str,
    uid: u64,
    flags: &[String],
) -> Result<()> {
    uid_store_flags_with_operation(config, source_folder, uid, flags, true)
}

pub fn uid_remove_flags(
    config: &ImapConfig,
    source_folder: &str,
    uid: u64,
    flags: &[String],
) -> Result<()> {
    uid_store_flags_with_operation(config, source_folder, uid, flags, false)
}

pub fn fetch_uid_snapshots(config: &ImapConfig) -> Result<Vec<FolderUidSnapshot>> {
    if config.tls {
        let mut session = login_tls(config)?;
        let result = fetch_uid_snapshots_session(&mut session, &config.mailboxes);
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = fetch_uid_snapshots_session(&mut session, &config.mailboxes);
        let _ = session.logout();
        result
    }
}

pub fn ensure_move_supported(config: &ImapConfig) -> Result<()> {
    if config.tls {
        let mut session = login_tls(config)?;
        let result = require_move(&mut session);
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = require_move(&mut session);
        let _ = session.logout();
        result
    }
}

#[derive(Clone, Debug)]
pub struct RemoteMessage {
    pub mailbox: String,
    pub uid_validity: u64,
    pub uid: u64,
    pub flags: Vec<String>,
    pub raw_eml: Vec<u8>,
}

#[derive(Clone, Debug)]
struct RemoteEnvelope {
    mailbox: String,
    uid_validity: u64,
    uid: u64,
    flags: Vec<String>,
    header: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderUidSnapshot {
    pub mailbox: String,
    pub uid_validity: u64,
    pub uids: BTreeSet<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("afmail-imap-{name}-{}-{stamp}", std::process::id()))
    }

    fn mailbox(name: &str, attributes: &[&str]) -> MailboxInfo {
        let attributes = attributes
            .iter()
            .map(|attribute| (*attribute).to_string())
            .collect::<Vec<_>>();
        MailboxInfo {
            name: name.to_string(),
            delimiter: Some("/".to_string()),
            special_use: special_use_from_attributes(&attributes),
            attributes,
        }
    }

    #[test]
    fn resolve_special_use_prefers_config_attribute_then_fallback_name() {
        let mut cfg = MailConfig::default();
        if let Some(archive) = cfg.mailboxes.get_mut("archive") {
            archive.mailbox_name = Some("Configured Archive".to_string());
            archive.special_use = None;
        }
        let mailboxes = vec![
            mailbox("RFC Archive", &["\\Archive"]),
            mailbox("Spam", &[]),
            mailbox("Trash", &[]),
        ];
        let archive = resolve_special_use_from_mailboxes(&cfg, SpecialUseKind::Archive, &mailboxes);
        assert_eq!(archive.mailbox_name, "Configured Archive");
        assert_eq!(archive.source, SpecialUseSource::Mailboxes);
        assert_eq!(archive.attribute, "\\Archive");
        assert!(archive.can_move_to);

        let junk = resolve_special_use_from_mailboxes(&cfg, SpecialUseKind::Junk, &mailboxes);
        assert_eq!(junk.mailbox_name, "Spam");
        assert_eq!(junk.source, SpecialUseSource::FallbackName);
        assert_eq!(junk.flag, Some("$Junk".to_string()));

        let trash = resolve_special_use_from_mailboxes(&cfg, SpecialUseKind::Trash, &mailboxes);
        assert_eq!(trash.mailbox_name, "Trash");
        assert_eq!(trash.source, SpecialUseSource::FallbackName);
    }

    #[test]
    fn save_remote_message_writes_three_files_and_triage() {
        let root = temp_root("save");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let _ = fs::create_dir_all(root.join("triage"));
        let raw = b"Message-ID: <m1@example.com>\r\nFrom: Alice <alice@example.com>\r\nTo: Me <me@example.com>\r\nSubject: Hello\r\nContent-Type: text/plain\r\n\r\nHello";
        let result = save_remote_message(
            &root,
            RemoteMessage {
                mailbox: "INBOX".to_string(),
                uid_validity: 10,
                uid: 20,
                flags: Vec::new(),
                raw_eml: raw.to_vec(),
            },
            &CaseSuggestion::default(),
            &PullTarget {
                id: "inbox".to_string(),
                mailbox: "INBOX".to_string(),
                import_as: PullImportAs::Triage,
                direction: MailDirection::Inbound,
            },
            chrono::Offset::fix(&chrono::Utc),
            &MailConfig::default(),
        );
        assert!(result.is_ok());
        let id = result.map(|saved| saved.message_id).unwrap_or_default();
        assert!(root.join(format!(".afmail/messages/{id}.eml")).exists());
        assert!(root
            .join(format!(".afmail/messages/{id}.state.json"))
            .exists());
        assert!(root
            .join(format!(".afmail/messages/{id}.remote.json"))
            .exists());
        assert!(!root.join(format!(".afmail/messages/{id}.txt")).exists());
        assert!(!root.join(format!(".afmail/messages/{id}.json")).exists());
        assert!(root.join(format!("messages/{id}.json")).exists());
        assert!(root.join(format!("triage/{id}.md")).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn message_id_carries_date_in_configured_offset() {
        let root = temp_root("id-date");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let raw = b"Message-ID: <tz1@example.com>\r\nDate: Mon, 16 Jun 2025 02:24:07 +0800\r\nFrom: a@example.com\r\nSubject: x\r\n\r\nhi".to_vec();
        let remote = RemoteMessage {
            mailbox: "INBOX".to_string(),
            uid_validity: 1,
            uid: 1,
            flags: Vec::new(),
            raw_eml: raw,
        };
        let utc = chrono::Offset::fix(&chrono::Utc);
        let plus8 =
            FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| chrono::Offset::fix(&chrono::Utc));
        let id_utc = stable_message_id(&root, &remote, utc);
        let id_plus8 = stable_message_id(&root, &remote, plus8);
        // 02:24 +08:00 is the previous day in UTC, the same day in +08:00.
        assert!(id_utc.starts_with("message_20250615_"), "{id_utc}");
        assert!(id_plus8.starts_with("message_20250616_"), "{id_plus8}");
        // Same offset is deterministic; the hash suffix is offset-independent.
        assert_eq!(id_plus8, stable_message_id(&root, &remote, plus8));
        assert_eq!(id_utc.rsplit('_').next(), id_plus8.rsplit('_').next());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reply_headers_match_suggested_case_uids() {
        let root = temp_root("suggest");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let existing = MessageFile {
            schema_name: "message".to_string(),
            schema_version: 1,
            message_id: "message_case_1".to_string(),
            rfc822_message_id: Some("<Case-One@Example.com>".to_string()),
            in_reply_to: None,
            references: Vec::new(),
            remote: None,
            direction: Some("inbound".to_string()),
            subject: Some("Case".to_string()),
            from: Some("alice@example.com".to_string()),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            sender: None,
            delivered_to: Vec::new(),
            x_original_to: Vec::new(),
            envelope_to: Vec::new(),
            list_id: None,
            mailing_list_headers: Vec::new(),
            authentication: crate::types::MessageAuthentication::default(),
            received_rfc3339: None,
            sent_rfc3339: None,
            body_text: String::new(),
            eml_path: None,
            attachments: Vec::new(),
            workspace: crate::types::WorkspaceState {
                status: "case".to_string(),
                archive_uid: None,
                archived_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
        };
        let second = MessageFile {
            message_id: "message_case_2".to_string(),
            rfc822_message_id: Some("<case-two@example.com>".to_string()),
            workspace: crate::types::WorkspaceState {
                status: "case".to_string(),
                archive_uid: None,
                archived_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
            ..existing.clone()
        };
        let _ = fs::write(
            root.join(".afmail/messages/message_case_1.eml"),
            "Message-ID: <Case-One@Example.com>\r\nFrom: Alice <alice@example.com>\r\nTo: Me <me@example.com>\r\nSubject: Case\r\n\r\nCase",
        );
        let _ = crate::store::Workspace::at(&root).write_message_artifacts(&existing);
        let _ = fs::write(
            root.join(".afmail/messages/message_case_2.eml"),
            "Message-ID: <case-two@example.com>\r\nFrom: Alice <alice@example.com>\r\nTo: Me <me@example.com>\r\nSubject: Case\r\n\r\nCase",
        );
        let _ = crate::store::Workspace::at(&root).write_message_artifacts(&second);
        let _ = fs::create_dir_all(root.join("cases/open/case-one/data"));
        let _ = fs::create_dir_all(root.join("cases/open/case-two/data"));
        let _ = write_json_pretty(
            &root.join("cases/open/case-one/data/messages.json"),
            &crate::types::CaseMessages {
                schema_name: "case_messages".to_string(),
                schema_version: 1,
                case_uid: "case-one".to_string(),
                message_ids: vec!["message_case_1".to_string()],
            },
        );
        let _ = write_json_pretty(
            &root.join("cases/open/case-two/data/messages.json"),
            &crate::types::CaseMessages {
                schema_name: "case_messages".to_string(),
                schema_version: 1,
                case_uid: "case-two".to_string(),
                message_ids: vec!["message_case_2".to_string()],
            },
        );
        let index = load_existing_remote_index(&root);
        assert!(index.is_ok());
        let raw = concat!(
            "Message-ID: <reply@example.com>\r\n",
            "In-Reply-To: <case-one@example.com>\r\n",
            "References: <other@example.com> <case-one@example.com> <case-two@example.com>\r\n",
            "From: Alice <alice@example.com>\r\n",
            "To: Me <me@example.com>\r\n",
            "Subject: Re: Case\r\n\r\n",
            "Reply"
        );
        let suggestion = index.unwrap_or_default().suggest_case(raw.as_bytes());
        assert_eq!(
            suggestion.case_uids,
            vec!["case-one".to_string(), "case-two".to_string()]
        );
        let _ = fs::remove_dir_all(root);
    }
}
