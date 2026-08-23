use crate::config::{ImapConfig, SpecialUseKind};
use crate::error::{AppError, Result};
use crate::types::RemoteLocation;
use rustls_connector::RustlsConnector;
use rustls_pemfile::certs;
use serde_json::json;
use std::fs;
use std::net::TcpStream;
use std::time::Duration as StdDuration;

const NETWORK_TIMEOUT: StdDuration = StdDuration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxInfo {
    pub name: String,
    pub delimiter: Option<String>,
    pub attributes: Vec<String>,
    pub special_use: Option<SpecialUseKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveOutcome {
    pub keyword_set: bool,
    pub keyword_error: Option<String>,
    pub seen_set: bool,
    pub seen_error: Option<String>,
    pub moved: bool,
    pub target_location: Option<RemoteLocation>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MoveFlagUpdates<'a> {
    pub mark_seen: bool,
    pub keyword: Option<&'a str>,
}

pub struct ImapClientSession {
    inner: Option<ImapClientSessionInner>,
}

enum ImapClientSessionInner {
    Plain(imap::Session<TcpStream>),
    Tls(Box<imap::Session<rustls_connector::TlsStream<TcpStream>>>),
}

impl ImapClientSession {
    pub fn connect(config: &ImapConfig) -> Result<Self> {
        let inner = if config.tls {
            ImapClientSessionInner::Tls(Box::new(login_tls(config)?))
        } else {
            ImapClientSessionInner::Plain(login_plain(config)?)
        };
        Ok(Self { inner: Some(inner) })
    }

    pub fn list_mailboxes(&mut self) -> Result<Vec<MailboxInfo>> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => list_mailboxes(session),
            ImapClientSessionInner::Tls(session) => list_mailboxes(session),
        }
    }

    pub fn append_message(&mut self, folder: &str, raw_eml: &[u8], draft: bool) -> Result<()> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => {
                append_message_session(session, folder, raw_eml, draft)
            }
            ImapClientSessionInner::Tls(session) => {
                append_message_session(session, folder, raw_eml, draft)
            }
        }
    }

    pub fn uid_mark_and_move(
        &mut self,
        source_folder: &str,
        expected_uid_validity: Option<u64>,
        uid: u64,
        target_folder: &str,
        rfc822_message_id: Option<&str>,
        flag_updates: MoveFlagUpdates<'_>,
    ) -> Result<MoveOutcome> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => uid_mark_and_move_session(
                session,
                source_folder,
                expected_uid_validity,
                uid,
                target_folder,
                rfc822_message_id,
                flag_updates,
            ),
            ImapClientSessionInner::Tls(session) => uid_mark_and_move_session(
                session,
                source_folder,
                expected_uid_validity,
                uid,
                target_folder,
                rfc822_message_id,
                flag_updates,
            ),
        }
    }

    pub fn uid_store_flags(
        &mut self,
        source_folder: &str,
        expected_uid_validity: Option<u64>,
        uid: u64,
        flags: &[String],
        add: bool,
    ) -> Result<()> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => uid_store_flags_session(
                session,
                source_folder,
                expected_uid_validity,
                uid,
                flags,
                add,
            ),
            ImapClientSessionInner::Tls(session) => uid_store_flags_session(
                session,
                source_folder,
                expected_uid_validity,
                uid,
                flags,
                add,
            ),
        }
    }

    pub fn find_uid_by_message_id(
        &mut self,
        folder: &str,
        rfc822_message_id: &str,
    ) -> Result<RemoteLocation> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => {
                find_uid_by_message_id_session(session, folder, rfc822_message_id)
            }
            ImapClientSessionInner::Tls(session) => {
                find_uid_by_message_id_session(session, folder, rfc822_message_id)
            }
        }
    }

    fn inner_mut(&mut self) -> Result<&mut ImapClientSessionInner> {
        self.inner.as_mut().ok_or_else(|| {
            AppError::new(
                "imap_session_closed",
                "IMAP session is already closed for this operation",
            )
        })
    }
}

impl Drop for ImapClientSession {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        match inner {
            ImapClientSessionInner::Plain(mut session) => {
                let _ = session.logout();
            }
            ImapClientSessionInner::Tls(mut session) => {
                let _ = session.logout();
            }
        }
    }
}

pub(crate) fn login_plain(config: &ImapConfig) -> Result<imap::Session<TcpStream>> {
    crate::util::ensure_plaintext_credentials_allowed(
        "IMAP",
        &config.host,
        !config.password_secret.is_empty(),
    )?;
    let stream = TcpStream::connect((config.host.as_str(), config.port)).map_err(|e| {
        retryable_imap_network_error(config, "imap_connect_failed", "connect", e.to_string())
    })?;
    configure_stream_timeout(&stream)?;
    let mut client = imap::Client::new(stream);
    client.read_greeting().map_err(|e| {
        retryable_imap_network_error(config, "imap_greeting_failed", "greeting", e.to_string())
    })?;
    client
        .login(&config.username, &config.password_secret)
        .map_err(|e| AppError::new("imap_login_failed", e.0.to_string()))
}

pub(crate) fn login_tls(
    config: &ImapConfig,
) -> Result<imap::Session<rustls_connector::TlsStream<TcpStream>>> {
    let stream = TcpStream::connect((config.host.as_str(), config.port)).map_err(|e| {
        retryable_imap_network_error(config, "imap_connect_failed", "connect", e.to_string())
    })?;
    configure_stream_timeout(&stream)?;
    let connector = tls_connector(config)?;
    let tls_stream = connector
        .connect(&config.host, stream)
        .map_err(|e| AppError::new("imap_tls_failed", e.to_string()))?;
    let mut client = imap::Client::new(tls_stream);
    client.read_greeting().map_err(|e| {
        retryable_imap_network_error(config, "imap_greeting_failed", "greeting", e.to_string())
    })?;
    client
        .login(&config.username, &config.password_secret)
        .map_err(|e| AppError::new("imap_login_failed", e.0.to_string()))
}

fn retryable_imap_network_error(
    config: &ImapConfig,
    code: &'static str,
    phase: &'static str,
    message: String,
) -> AppError {
    AppError::retryable(code, message).with_details(json!({
        "protocol": "imap",
        "phase": phase,
        "host": config.host,
        "port": config.port,
        "tls": config.tls
    }))
}

fn tls_connector(config: &ImapConfig) -> Result<RustlsConnector> {
    let mut tls = rustls_connector::RustlsConnectorConfig::new_with_webpki_root_certs();
    if let Some(path) = &config.tls_root_cert_path {
        let pem = fs::read(path)
            .map_err(|e| AppError::io(&format!("read IMAP TLS root cert {path}"), &e))?;
        let mut slice = pem.as_slice();
        let certs = certs(&mut slice)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| AppError::new("imap_tls_failed", e.to_string()))?;
        if certs.is_empty() {
            return Err(AppError::new(
                "imap_tls_failed",
                format!("no certificates found in IMAP TLS root cert {path}"),
            ));
        }
        tls = tls.with_parsable_certificates(certs);
    }
    tls.connector_with_no_client_auth()
        .map_err(|e| AppError::new("imap_tls_failed", e.to_string()))
}

fn configure_stream_timeout(stream: &TcpStream) -> Result<()> {
    stream
        .set_read_timeout(Some(NETWORK_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(NETWORK_TIMEOUT)))
        .map_err(|e| AppError::io("configure network timeout", &e))
}

pub(crate) fn list_mailboxes<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
) -> Result<Vec<MailboxInfo>> {
    let names = session
        .list(None, Some("*"))
        .map_err(|e| AppError::new("imap_list_failed", e.to_string()))?;
    let mut out = Vec::new();
    for name in names.iter() {
        let attributes = name
            .attributes()
            .iter()
            .map(format_name_attribute)
            .collect::<Vec<_>>();
        out.push(MailboxInfo {
            name: name.name().to_string(),
            delimiter: name.delimiter().map(ToString::to_string),
            special_use: special_use_from_attributes(&attributes),
            attributes,
        });
    }
    Ok(out)
}

pub(crate) fn capability_move<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
) -> Result<bool> {
    capability_has(session, "MOVE")
}

pub(crate) fn capability_uidplus<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
) -> Result<bool> {
    capability_has(session, "UIDPLUS")
}

pub(crate) fn capability_has<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    capability: &str,
) -> Result<bool> {
    let capabilities = session
        .capabilities()
        .map_err(|e| AppError::new("imap_capability_failed", e.to_string()))?;
    Ok(capabilities.has_str(capability))
}

fn format_name_attribute(attribute: &imap_proto::NameAttribute<'_>) -> String {
    match attribute {
        imap_proto::NameAttribute::NoInferiors => "\\Noinferiors".to_string(),
        imap_proto::NameAttribute::NoSelect => "\\Noselect".to_string(),
        imap_proto::NameAttribute::Marked => "\\Marked".to_string(),
        imap_proto::NameAttribute::Unmarked => "\\Unmarked".to_string(),
        imap_proto::NameAttribute::All => "\\All".to_string(),
        imap_proto::NameAttribute::Archive => "\\Archive".to_string(),
        imap_proto::NameAttribute::Drafts => "\\Drafts".to_string(),
        imap_proto::NameAttribute::Flagged => "\\Flagged".to_string(),
        imap_proto::NameAttribute::Junk => "\\Junk".to_string(),
        imap_proto::NameAttribute::Sent => "\\Sent".to_string(),
        imap_proto::NameAttribute::Trash => "\\Trash".to_string(),
        imap_proto::NameAttribute::Extension(value) => value.to_string(),
        other => format!("{other:?}"),
    }
}

pub(crate) fn append_message_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    folder: &str,
    raw_eml: &[u8],
    draft: bool,
) -> Result<()> {
    let mut append = session.append(folder, raw_eml);
    if draft {
        append.flag(imap::types::Flag::Draft);
    }
    append
        .finish()
        .map(|_| ())
        .map_err(|e| AppError::new("imap_append_failed", e.to_string()))
}

pub(crate) fn append_draft_and_find_uid_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    folder: &str,
    raw_eml: &[u8],
    rfc822_message_id: &str,
) -> Result<RemoteLocation> {
    append_message_session(session, folder, raw_eml, true)?;
    let mailbox_status = session
        .examine(folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    let uid_validity = mailbox_status.uid_validity.unwrap_or(0) as u64;
    let query = format!(
        "HEADER Message-ID {}",
        quote_search_string(rfc822_message_id)
    );
    let uids = session
        .uid_search(query)
        .map_err(|e| AppError::new("imap_search_failed", e.to_string()))?;
    let uid = uids
        .into_iter()
        .max()
        .ok_or_else(|| AppError::new("imap_uid_missing", "appended draft uid was not found"))?;
    Ok(RemoteLocation {
        mailbox_id: None,
        mailbox_name: folder.to_string(),
        uid_validity: Some(uid_validity),
        uid: Some(uid as u64),
        flags: Vec::new(),
        observed_rfc3339: crate::store::now_rfc3339(),
        missing_rfc3339: None,
    })
}

pub(crate) fn uid_move_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    source_folder: &str,
    uid: u64,
    target_folder: &str,
) -> Result<()> {
    let moved = source_folder != target_folder;
    let move_supported = moved && capability_move(session)?;
    if moved && !move_supported {
        require_uidplus_for_precise_move_fallback(session)?;
    }
    session
        .select(source_folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    if !moved {
        return Ok(());
    }
    if move_supported {
        session
            .uid_mv(uid.to_string(), target_folder)
            .map_err(|e| AppError::new("imap_move_failed", e.to_string()))
    } else {
        uid_copy_delete_expunge_session(session, uid, target_folder)
    }
}

pub(crate) fn uid_mark_and_move_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    source_folder: &str,
    expected_uid_validity: Option<u64>,
    uid: u64,
    target_folder: &str,
    rfc822_message_id: Option<&str>,
    flag_updates: MoveFlagUpdates<'_>,
) -> Result<MoveOutcome> {
    let moved = source_folder != target_folder;
    let mailbox_status = session
        .select(source_folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    ensure_expected_uid_validity(
        source_folder,
        uid,
        expected_uid_validity,
        mailbox_status.uid_validity.map(u64::from),
    )?;
    let move_supported = moved && capability_move(session)?;
    if moved && !move_supported {
        require_uidplus_for_precise_move_fallback(session)?;
    }
    let (seen_set, seen_error) = if flag_updates.mark_seen {
        session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Seen)")
            .map(|_| (true, None))
            .map_err(|e| AppError::new("imap_store_failed", e.to_string()))?
    } else {
        (false, None)
    };
    let (keyword_set, keyword_error) = if let Some(keyword) = flag_updates.keyword {
        let keyword_result =
            session.uid_store(uid.to_string(), format!("+FLAGS.SILENT ({keyword})"));
        match keyword_result {
            Ok(_) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        }
    } else {
        (false, None)
    };
    if moved {
        if move_supported {
            session
                .uid_mv(uid.to_string(), target_folder)
                .map_err(|e| AppError::new("imap_move_failed", e.to_string()))?;
        } else {
            uid_copy_delete_expunge_session(session, uid, target_folder)?;
        }
    }
    let target_location = match rfc822_message_id {
        Some(message_id) => {
            match find_uid_by_message_id_session(session, target_folder, message_id) {
                Ok(location) => Some(location),
                Err(err) if moved && err.error_code == "imap_uid_missing" => {
                    return Err(AppError::new(
                    "imap_move_target_uid_missing",
                    "remote move succeeded but the target message UID was not found",
                )
                .with_hint(
                    "Run `afmail pull`, then preview and confirm the push again. Recovery verifies the target message without replaying the move.",
                )
                .with_details(json!({
                    "target_mailbox_name": target_folder,
                    "rfc822_message_id": message_id,
                    "remote_effect_completed": true,
                })));
                }
                Err(err) => return Err(err),
            }
        }
        None => None,
    };
    Ok(MoveOutcome {
        keyword_set,
        keyword_error,
        seen_set,
        seen_error,
        moved,
        target_location,
    })
}

pub(crate) fn uid_store_flags_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    source_folder: &str,
    expected_uid_validity: Option<u64>,
    uid: u64,
    flags: &[String],
    add: bool,
) -> Result<()> {
    let mailbox_status = session
        .select(source_folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    ensure_expected_uid_validity(
        source_folder,
        uid,
        expected_uid_validity,
        mailbox_status.uid_validity.map(u64::from),
    )?;
    let flags = flags.join(" ");
    let operation = if add { "+" } else { "-" };
    session
        .uid_store(
            uid.to_string(),
            format!("{operation}FLAGS.SILENT ({flags})"),
        )
        .map_err(|e| AppError::new("imap_store_failed", e.to_string()))?;
    Ok(())
}

fn ensure_expected_uid_validity(
    source_folder: &str,
    uid: u64,
    expected_uid_validity: Option<u64>,
    actual_uid_validity: Option<u64>,
) -> Result<()> {
    let Some(expected_uid_validity) = expected_uid_validity else {
        return Ok(());
    };
    if actual_uid_validity == Some(expected_uid_validity) {
        return Ok(());
    }
    Err(AppError::new(
        "imap_uidvalidity_changed",
        format!("UIDVALIDITY changed for mailbox {source_folder}; refusing to use stale UID {uid}"),
    )
    .with_hint(
        "Run `afmail pull` to reconcile the mailbox, review the new push plan, then confirm that plan.",
    )
    .with_details(serde_json::json!({
        "mailbox_name": source_folder,
        "uid": uid,
        "expected_uid_validity": expected_uid_validity,
        "actual_uid_validity": actual_uid_validity,
        "remote_calls_started": false,
    })))
}

pub(crate) fn require_uidplus_for_precise_move_fallback<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
) -> Result<()> {
    if capability_uidplus(session)? {
        Ok(())
    } else {
        Err(AppError::new(
            "imap_uidplus_required",
            "remote IMAP server does not advertise MOVE or UIDPLUS; afmail cannot safely fallback to copy/delete without UID EXPUNGE",
        )
        .with_hint(
            "Manually archive the message in the mail client, or use a server that advertises MOVE or UIDPLUS before retrying this remote move.",
        )
        .with_details(serde_json::json!({
            "remote_calls_started": false,
        })))
    }
}

pub(crate) fn uid_copy_delete_expunge_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    uid: u64,
    target_folder: &str,
) -> Result<()> {
    let uid_set = uid.to_string();
    session
        .uid_copy(&uid_set, quote_mailbox_name(target_folder))
        .map_err(|e| AppError::new("imap_copy_failed", e.to_string()))?;
    session
        .uid_store(&uid_set, "+FLAGS.SILENT (\\Deleted)")
        .map_err(|e| AppError::new("imap_store_failed", e.to_string()))?;
    session
        .uid_expunge(&uid_set)
        .map(|_| ())
        .map_err(|e| AppError::new("imap_expunge_failed", e.to_string()))
}

fn quote_mailbox_name(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

pub(crate) fn require_move<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
) -> Result<()> {
    if capability_move(session)? {
        Ok(())
    } else {
        Err(AppError::new(
            "imap_move_unsupported",
            "remote IMAP server does not advertise MOVE",
        ))
    }
}

pub(crate) fn quote_search_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

pub(crate) fn find_uid_by_message_id_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    folder: &str,
    rfc822_message_id: &str,
) -> Result<RemoteLocation> {
    let mailbox_status = session
        .examine(folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    let uid_validity = mailbox_status.uid_validity.unwrap_or(0) as u64;
    let query = format!(
        "HEADER Message-ID {}",
        quote_search_string(rfc822_message_id)
    );
    let uid = session
        .uid_search(query)
        .map_err(|e| AppError::new("imap_search_failed", e.to_string()))?
        .into_iter()
        .max()
        .map(|uid| uid as u64)
        .ok_or_else(|| AppError::new("imap_uid_missing", "message uid was not found"))?;
    Ok(RemoteLocation {
        mailbox_id: None,
        mailbox_name: folder.to_string(),
        uid_validity: Some(uid_validity),
        uid: Some(uid),
        flags: Vec::new(),
        observed_rfc3339: crate::store::now_rfc3339(),
        missing_rfc3339: None,
    })
}

fn special_use_from_attributes(attributes: &[String]) -> Option<SpecialUseKind> {
    crate::config::special_use_kinds()
        .iter()
        .copied()
        .find(|kind| {
            attributes
                .iter()
                .any(|attribute| attribute.eq_ignore_ascii_case(kind.attribute()))
        })
}

#[cfg(test)]
mod tests {
    use super::ensure_expected_uid_validity;

    #[test]
    fn uid_validity_mismatch_is_a_pre_effect_error() {
        let error = ensure_expected_uid_validity("INBOX", 7, Some(44), Some(45))
            .err()
            .unwrap_or_else(|| {
                crate::error::AppError::new("test_failure", "expected UIDVALIDITY mismatch")
            });

        assert_eq!(error.error_code, "imap_uidvalidity_changed");
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|details| details.get("remote_calls_started")),
            Some(&serde_json::json!(false))
        );
        assert!(ensure_expected_uid_validity("INBOX", 7, Some(44), Some(44)).is_ok());
    }
}
