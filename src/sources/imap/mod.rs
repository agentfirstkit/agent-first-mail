mod identity;
mod session;
mod special_use;

use crate::ingest::*;
use identity::*;
use session::*;
use special_use::*;

pub use special_use::resolve_special_use_from_mailboxes;

use crate::config::{
    ImapConfig, MailConfig, MailDirection, PullImportAs, SpecialUseKind, SpecialUseSource,
    SpecialUseTarget,
};
use crate::error::{AppError, Result};
pub use crate::imap_client::{MailboxInfo, MoveOutcome};
use crate::imap_client::{
    MoveFlagUpdates, append_draft_and_find_uid_session, append_message_session, capability_move,
    capability_uidplus, list_mailboxes, login_plain, login_tls, require_move,
    uid_mark_and_move_session, uid_move_session,
};
use crate::progress::ProgressCallback;
#[cfg(test)]
use crate::types::MessageFile;
use crate::types::{MessageStatus, RemoteLocation, RemoteState};
use crate::util::canonical_flags;
#[cfg(test)]
use crate::util::write_json_pretty;
#[cfg(test)]
use chrono::FixedOffset;
use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

const DEFAULT_INITIAL_SINCE_DAYS: u32 = 90;
const DEFAULT_MAX_NEW_MESSAGES: usize = 500;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PullOptions {
    pub(crate) since_days: Option<u32>,
    pub(crate) max_new_messages: usize,
    pub(crate) max_total_bytes: u64,
    pub(crate) all_history: bool,
    pub(crate) dry_run: bool,
}

impl PullOptions {
    pub(crate) fn new(
        since_days: Option<u32>,
        max_new_messages: Option<usize>,
        max_total_bytes: Option<u64>,
        all_history: bool,
        dry_run: bool,
    ) -> Self {
        Self {
            since_days,
            max_new_messages: max_new_messages.unwrap_or(DEFAULT_MAX_NEW_MESSAGES),
            max_total_bytes: max_total_bytes.unwrap_or(DEFAULT_MAX_TOTAL_BYTES),
            all_history,
            dry_run,
        }
    }

    fn effective_since_days(self) -> Option<u32> {
        if self.all_history {
            None
        } else {
            Some(self.since_days.unwrap_or(DEFAULT_INITIAL_SINCE_DAYS))
        }
    }
}

impl Default for PullOptions {
    fn default() -> Self {
        Self::new(None, None, None, false, false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyFetchMode {
    BoundedPartial,
    CompatibilityFull,
}

fn body_fetch_mode(
    config: &ImapConfig,
    targets: &[PullTarget],
    options: PullOptions,
) -> Result<BodyFetchMode> {
    if config.allow_unbounded_body_fetch {
        return Ok(BodyFetchMode::CompatibilityFull);
    }
    let Some(target) = targets.first() else {
        return Ok(BodyFetchMode::BoundedPartial);
    };
    let probe = if config.tls {
        let mut session = login_tls(config)?;
        let result = probe_partial_body_fetch(&mut session, &target.mailbox, options);
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = probe_partial_body_fetch(&mut session, &target.mailbox, options);
        let _ = session.logout();
        result
    };
    match probe {
        Ok(()) => Ok(BodyFetchMode::BoundedPartial),
        Err(error) => Err(AppError::new(
            "imap_partial_fetch_unsupported",
            format!(
                "IMAP server does not support bounded partial BODY fetch: {error}"
            ),
        )
        .with_hint(
            "Use a server with RFC partial BODY fetch support. Only if this server is trusted, set imap.allow_unbounded_body_fetch=true to permit one full literal at a time.",
        )
        .with_details(json!({
            "mailbox_name": target.mailbox,
            "allow_unbounded_body_fetch": false,
            "message_writes_started": false,
        }))),
    }
}

fn probe_partial_body_fetch<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    mailbox: &str,
    options: PullOptions,
) -> Result<()> {
    session
        .examine(mailbox)
        .map_err(|error| AppError::new("imap_select_failed", error.to_string()))?;
    let sequence = mailbox_uid_sequence(session, options.effective_since_days(), None)?;
    if sequence.is_empty() {
        return Ok(());
    }
    let uid = session
        .uid_search("ALL")
        .map_err(|error| AppError::new("imap_search_failed", error.to_string()))?
        .into_iter()
        .min();
    let Some(uid) = uid else { return Ok(()) };
    session
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[]<0.1>)")
        .map_err(|error| AppError::new("imap_fetch_failed", error.to_string()))?;
    Ok(())
}

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
        mail_config.mailbox(id).ok().is_some_and(|mailbox| {
            mailbox.resolve.len() != 1
                || mailbox
                    .resolve
                    .iter()
                    .any(|rule| rule.special_use.as_ref().is_some())
        })
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
        let mailbox = if let Some(mailboxes) = &mailboxes {
            resolve_configured_mailbox(&id, configured, mailboxes)?
        } else {
            mail_config.offline_mailbox_name(&id)?
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

pub(crate) fn resolve_configured_mailbox(
    id: &str,
    configured: &crate::config::ImapMailboxConfig,
    mailboxes: &[MailboxInfo],
) -> Result<String> {
    for (index, rule) in configured.resolve.iter().enumerate() {
        if let Some(name) = rule.mailbox_name.as_deref() {
            let matches = mailboxes
                .iter()
                .filter(|mailbox| mailbox.name.eq_ignore_ascii_case(name))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [mailbox] => return Ok(mailbox.name.clone()),
                [] => {}
                _ => {
                    return Err(AppError::new(
                        "imap_mailbox_ambiguous",
                        format!(
                            "mailboxes.{id}.resolve[{index}].mailbox_name {name} matched multiple remote mailboxes"
                        ),
                    ));
                }
            }
        }
        if let Some(special_use) = rule.special_use.as_deref() {
            let matches = mailboxes
                .iter()
                .filter(|mailbox| {
                    mailbox
                        .attributes
                        .iter()
                        .any(|attribute| attribute.eq_ignore_ascii_case(special_use))
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [mailbox] => return Ok(mailbox.name.clone()),
                [] => {}
                _ => {
                    return Err(AppError::new(
                        "imap_mailbox_ambiguous",
                        format!(
                            "mailboxes.{id}.resolve[{index}].special_use {special_use} matched multiple remote mailboxes"
                        ),
                    ));
                }
            }
        }
    }
    Err(AppError::new(
        "imap_mailbox_unresolved",
        format!("mailboxes.{id}.resolve matched no remote mailbox"),
    ))
}

pub(crate) fn pull_workspace(
    root: &Path,
    mail_config: &MailConfig,
    config: &ImapConfig,
    targets: &[PullTarget],
    options: PullOptions,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    let mut progress = progress;
    let body_fetch_mode = if options.dry_run {
        BodyFetchMode::BoundedPartial
    } else {
        body_fetch_mode(config, targets, options)?
    };
    if config.tls {
        let mut session = login_tls(config)?;
        let result = if options.dry_run {
            estimate_pull_workspace_session(root, targets, options, &mut session)
        } else {
            pull_workspace_session(
                root,
                mail_config,
                targets,
                options,
                body_fetch_mode,
                &mut session,
                &mut progress,
            )
        };
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = if options.dry_run {
            estimate_pull_workspace_session(root, targets, options, &mut session)
        } else {
            pull_workspace_session(
                root,
                mail_config,
                targets,
                options,
                body_fetch_mode,
                &mut session,
                &mut progress,
            )
        };
        let _ = session.logout();
        result
    }
}

pub(crate) fn pull_used_incremental_cursors(result: &Value) -> bool {
    result
        .get("__afmail_pull_all_incremental")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn remove_internal_pull_fields(result: &mut Value) {
    let Some(map) = result.as_object_mut() else {
        return;
    };
    map.remove("__afmail_pull_all_incremental");
    map.remove("__afmail_pull_cursor_updates");
}

pub(crate) fn commit_pull_cursors(root: &Path, result: &Value, full_reconcile: bool) -> Result<()> {
    commit_pull_cursor_updates(root, result, full_reconcile)
}

pub fn remote_test(config: &ImapConfig) -> Result<Value> {
    let started = Instant::now();
    let (move_supported, uidplus_supported) = if config.tls {
        let mut session = login_tls(config)?;
        let move_supported = capability_move(&mut session)?;
        let uidplus_supported = capability_uidplus(&mut session)?;
        session
            .logout()
            .map_err(|e| AppError::new("imap_logout_failed", e.to_string()))?;
        (move_supported, uidplus_supported)
    } else {
        let mut session = login_plain(config)?;
        let move_supported = capability_move(&mut session)?;
        let uidplus_supported = capability_uidplus(&mut session)?;
        session
            .logout()
            .map_err(|e| AppError::new("imap_logout_failed", e.to_string()))?;
        (move_supported, uidplus_supported)
    };
    Ok(json!({
        "ok": true,
        "host": config.host,
        "port": config.port,
        "tls": config.tls,
        "capabilities": {
            "move": move_supported,
            "uidplus": uidplus_supported
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
        "mailboxes": mailboxes,
        "special_use_targets": targets,
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
            None,
            uid,
            target_folder,
            rfc822_message_id,
            MoveFlagUpdates { mark_seen, keyword },
        );
        let _ = session.logout();
        result
    } else {
        let mut session = login_plain(config)?;
        let result = uid_mark_and_move_session(
            &mut session,
            source_folder,
            None,
            uid,
            target_folder,
            rfc822_message_id,
            MoveFlagUpdates { mark_seen, keyword },
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
    size_bytes: Option<u64>,
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

    fn case_collection(
        case_uid: &str,
        case_name: &str,
        message_id: &str,
    ) -> crate::types::CaseMessages {
        let mut collection =
            crate::types::CaseMessages::new_case(case_uid, case_name, "2026-05-22T10:00:00Z");
        collection.upsert_item(message_id, None, "2026-05-22T10:00:00Z");
        collection
    }

    #[test]
    fn resolve_special_use_prefers_config_attribute_then_fallback_name() {
        let mut cfg = MailConfig::default();
        if let Some(archive) = cfg.mailboxes.get_mut("archive") {
            archive.resolve = vec![crate::config::MailboxResolveRule::mailbox_name(
                "Configured Archive",
            )];
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
        assert_eq!(junk.source, SpecialUseSource::Mailboxes);
        assert_eq!(junk.flag, Some("$Junk".to_string()));

        let trash = resolve_special_use_from_mailboxes(&cfg, SpecialUseKind::Trash, &mailboxes);
        assert_eq!(trash.mailbox_name, "Trash");
        assert_eq!(trash.source, SpecialUseSource::Mailboxes);
    }

    #[test]
    fn resolve_configured_mailbox_uses_ordered_rules() -> Result<()> {
        let cfg = MailConfig::default();
        let archive = cfg.mailbox("archive")?;
        let mailboxes = vec![mailbox("Archive", &[])];
        assert_eq!(
            resolve_configured_mailbox("archive", archive, &mailboxes).as_deref(),
            Ok("Archive")
        );

        let sent = cfg.mailbox("sent")?;
        let mailboxes = vec![mailbox("Sent", &[]), mailbox("Sent Messages", &["\\Sent"])];
        assert_eq!(
            resolve_configured_mailbox("sent", sent, &mailboxes).as_deref(),
            Ok("Sent Messages")
        );

        let junk = cfg.mailbox("junk")?;
        let mailboxes = vec![mailbox("Spam", &[])];
        assert_eq!(
            resolve_configured_mailbox("junk", junk, &mailboxes).as_deref(),
            Ok("Spam")
        );
        Ok(())
    }

    #[test]
    fn resolve_configured_mailbox_errors_when_no_rule_matches() -> Result<()> {
        let cfg = MailConfig::default();
        let archive = cfg.mailbox("archive")?;
        let err = resolve_configured_mailbox("archive", archive, &[mailbox("INBOX", &[])]);
        assert!(err.is_err());
        assert_eq!(
            err.err().map(|err| err.error_code),
            Some("imap_mailbox_unresolved")
        );
        Ok(())
    }

    #[test]
    fn save_remote_message_writes_three_files_and_triage() -> Result<()> {
        let root = temp_root("save");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let _ = fs::create_dir_all(root.join("triage"));
        let raw = b"Message-ID: <m1@example.com>\r\nFrom: Alice <alice@example.com>\r\nTo: Me <me@example.com>\r\nSubject: Hello\r\nContent-Type: text/plain\r\n\r\nHello";
        let config = MailConfig::default();
        let render_context = IngestRenderContext::new(&root, &config)?;
        let result = ingest_message(
            &root,
            &mut ExistingIndex::default(),
            &render_context,
            &config,
            IngestMessage {
                raw_eml: raw.to_vec(),
                remote_location: Some(RemoteLocation {
                    mailbox_id: Some("inbox".into()),
                    mailbox_name: "INBOX".into(),
                    uid_validity: Some(10),
                    uid: Some(20),
                    flags: vec![],
                    observed_rfc3339: crate::store::now_rfc3339(),
                    missing_rfc3339: None,
                }),
                delivery: None,
                import_as: PullImportAs::Triage,
                direction: MailDirection::Inbound,
            },
        );
        assert!(result.is_ok());
        let id = result
            .map(|saved| saved.message_id().to_string())
            .unwrap_or_default();
        assert!(root.join(format!(".afmail/messages/{id}.eml")).exists());
        assert!(
            !root
                .join(format!(".afmail/messages/{id}.state.json"))
                .exists()
        );
        assert!(root.join(format!(".afmail/messages/{id}.json")).exists());
        assert!(!root.join(format!(".afmail/messages/{id}.txt")).exists());
        assert!(root.join(format!("messages/{id}.json")).exists());
        assert!(root.join(format!("triage/{id}.md")).exists());
        let _ = fs::remove_dir_all(root);
        Ok(())
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
        let id_utc = stable_message_id(&remote.raw_eml, utc);
        let id_plus8 = stable_message_id(&remote.raw_eml, plus8);
        // 02:24 +08:00 is the previous day in UTC, the same day in +08:00.
        assert!(id_utc.starts_with("message_20250615_"), "{id_utc}");
        assert!(id_plus8.starts_with("message_20250616_"), "{id_plus8}");
        // Same offset is deterministic; the hash suffix is offset-independent.
        assert_eq!(id_plus8, stable_message_id(&remote.raw_eml, plus8));
        assert_eq!(id_utc.rsplit('_').next(), id_plus8.rsplit('_').next());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn message_id_accepts_qq_style_date_header() {
        let root = temp_root("id-qq-date");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let raw = b"Message-ID: <qq-date@example.com>\r\nDate: Wed Apr 15 2026 11:39:16 GMT+0800\r\nFrom: a@example.com\r\nSubject: x\r\n\r\nhi".to_vec();
        let remote = RemoteMessage {
            mailbox: "INBOX".to_string(),
            uid_validity: 1,
            uid: 1,
            flags: Vec::new(),
            raw_eml: raw,
        };
        let plus8 =
            FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| chrono::Offset::fix(&chrono::Utc));
        let id = stable_message_id(&remote.raw_eml, plus8);
        assert!(id.starts_with("message_20260415_"), "{id}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn message_id_distinguishes_duplicate_rfc822_ids_by_content() {
        let offset = chrono::Offset::fix(&chrono::Utc);
        let first = RemoteMessage {
            mailbox: "INBOX".to_string(),
            uid_validity: 1,
            uid: 1,
            flags: Vec::new(),
            raw_eml: b"Message-ID: <duplicate@example.com>\r\nDate: Mon, 16 Jun 2025 02:24:07 +0000\r\nFrom: a@example.com\r\nSubject: first\r\n\r\nfirst"
                .to_vec(),
        };
        let second = RemoteMessage {
            mailbox: "INBOX".to_string(),
            uid_validity: 1,
            uid: 2,
            flags: Vec::new(),
            raw_eml: b"Message-ID: <duplicate@example.com>\r\nDate: Mon, 16 Jun 2025 02:24:07 +0000\r\nFrom: a@example.com\r\nSubject: second\r\n\r\nsecond"
                .to_vec(),
        };
        let exact_copy = RemoteMessage {
            mailbox: "Archive".to_string(),
            uid_validity: 1,
            uid: 3,
            flags: Vec::new(),
            raw_eml: first.raw_eml.clone(),
        };

        let first_id = stable_message_id(&first.raw_eml, offset);
        let second_id = stable_message_id(&second.raw_eml, offset);
        let copy_id = stable_message_id(&exact_copy.raw_eml, offset);
        assert_ne!(first_id, second_id);
        assert_eq!(first_id, copy_id);
    }

    #[test]
    fn message_id_falls_back_to_sender_received_date_without_date_header() {
        let root = temp_root("id-received-date");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let raw = concat!(
            "Received: from mx.example by inbound.example; Thu, 16 Apr 2026 00:01:00 +0000 (UTC)\r\n",
            "Received: from smtp.qq.com by sender.example; Wed, 15 Apr 2026 23:59:00 +0800\r\n",
            "Message-ID: <received-fallback@example.com>\r\n",
            "From: a@example.com\r\n",
            "Subject: x\r\n",
            "\r\n",
            "hi"
        )
        .as_bytes()
        .to_vec();
        let remote = RemoteMessage {
            mailbox: "INBOX".to_string(),
            uid_validity: 1,
            uid: 1,
            flags: Vec::new(),
            raw_eml: raw,
        };
        let plus8 =
            FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| chrono::Offset::fix(&chrono::Utc));
        let id = stable_message_id(&remote.raw_eml, plus8);
        assert!(id.starts_with("message_20260415_"), "{id}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reply_headers_match_suggested_case_uids() {
        let root = temp_root("suggest");
        let _ = fs::create_dir_all(root.join(".afmail/messages"));
        let _ = write_json_pretty(
            &root.join(".afmail/config.json"),
            &crate::config::MailConfig::default(),
        );
        let existing = MessageFile {
            schema_name: "message".to_string(),
            schema_version: crate::types::MESSAGE_SCHEMA_VERSION,
            message_id: "message_case_1".to_string(),
            origin: None,
            rfc822_message_id: Some("<Case-One@Example.com>".to_string()),
            in_reply_to: None,
            references: Vec::new(),
            remote: None,
            agent_suggestion: None,
            human_suggestion: None,
            case_relationship_suggestion: None,
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
            body_markdown: None,
            eml_path: None,
            attachments: Vec::new(),
            contact: None,
            identity: None,
            identity_email: None,
            identity_match: None,
            identity_candidates: Vec::new(),
            observed_recipient_emails: Vec::new(),
            workspace: crate::types::WorkspaceState {
                status: "case".to_string(),
                notification_uid: None,
                notification_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
        };
        let second = MessageFile {
            message_id: "message_case_2".to_string(),
            origin: None,
            rfc822_message_id: Some("<case-two@example.com>".to_string()),
            workspace: crate::types::WorkspaceState {
                status: "case".to_string(),
                notification_uid: None,
                notification_rfc3339: None,
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
        let _ = fs::create_dir_all(root.join("cases/support/c20260521001-case-one/data"));
        let _ = fs::create_dir_all(root.join("cases/support/c20260521002-case-two/data"));
        let _ = write_json_pretty(
            &root.join("cases/support/c20260521001-case-one/data/case.json"),
            &case_collection("c20260521001", "Case One", "message_case_1"),
        );
        let _ = write_json_pretty(
            &root.join("cases/support/c20260521002-case-two/data/case.json"),
            &case_collection("c20260521002", "Case Two", "message_case_2"),
        );
        let index = load_existing_remote_index(&root);
        assert!(index.is_ok(), "{:?}", index.as_ref().err());
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
            vec!["c20260521001".to_string(), "c20260521002".to_string()]
        );
        let _ = fs::remove_dir_all(root);
    }
}
