use crate::config::{ImapConfig, SpecialUseKind};
use crate::error::{AppError, Result};
use crate::types::RemoteLocation;
use rustls_connector::RustlsConnector;
use rustls_pemfile::certs;
use serde_json::json;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
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

/// Where a completed `UID MOVE` / `UID COPY` put the message, as the server
/// itself reported it in a `COPYUID` response code (RFC 4315 §3; for MOVE,
/// RFC 6851 §4.3 sends it as an untagged `OK`). Servers may omit it, so the
/// receipt is optional and the `HEADER Message-ID` search stays as fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CopyUidReceipt {
    pub uid_validity: u64,
    pub source_uids: Vec<u64>,
    pub target_uids: Vec<u64>,
}

impl CopyUidReceipt {
    /// The target UID of `source_uid`, when the receipt maps it exactly once.
    /// A receipt whose two sets differ in length, or that names the source
    /// more than once, is not trusted: the caller falls back to a lookup.
    pub fn target_for(&self, source_uid: u64) -> Option<u64> {
        if self.source_uids.len() != self.target_uids.len() {
            return None;
        }
        let mut targets = self
            .source_uids
            .iter()
            .zip(&self.target_uids)
            .filter(|(source, _)| **source == source_uid)
            .map(|(_, target)| *target);
        let first = targets.next()?;
        targets.next().is_none().then_some(first)
    }
}

/// Largest UID set a receipt is expanded for. afmail moves one message per
/// command, so anything past this is a malformed or hostile response.
const COPYUID_MAX_EXPANDED_UIDS: usize = 4096;

fn expand_uid_set(members: &[imap_proto::rfc4315::UidSetMember]) -> Option<Vec<u64>> {
    let mut out = Vec::new();
    for member in members {
        match member {
            imap_proto::rfc4315::UidSetMember::Uid(uid) => out.push(u64::from(*uid)),
            imap_proto::rfc4315::UidSetMember::UidRange(range) => {
                let (start, end) = (*range.start(), *range.end());
                if start > end || (end - start) as usize >= COPYUID_MAX_EXPANDED_UIDS {
                    return None;
                }
                out.extend((start..=end).map(u64::from));
            }
        }
        if out.len() > COPYUID_MAX_EXPANDED_UIDS {
            return None;
        }
    }
    Some(out)
}

/// Scan a complete command response — every untagged line plus the tagged
/// completion — for a `COPYUID` code. Returns `None` when the server sent
/// none or the bytes do not parse; neither says anything about whether the
/// command succeeded, which the caller already knows.
pub(crate) fn parse_copyuid_receipt(response: &[u8]) -> Option<CopyUidReceipt> {
    let mut rest = response;
    while !rest.is_empty() {
        let (remaining, parsed) = imap_proto::Response::parse(rest).ok()?;
        let code = match &parsed {
            imap_proto::Response::Data { outcome, .. }
            | imap_proto::Response::Done { outcome, .. } => outcome.code.as_ref(),
            _ => None,
        };
        if let Some(imap_proto::ResponseCode::CopyUid(uid_validity, sources, targets)) = code {
            return Some(CopyUidReceipt {
                uid_validity: u64::from(*uid_validity),
                source_uids: expand_uid_set(sources)?,
                target_uids: expand_uid_set(targets)?,
            });
        }
        if remaining.len() >= rest.len() {
            return None;
        }
        rest = remaining;
    }
    None
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
    let stream = connect_stream(config)?;
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
    let stream = connect_stream(config)?;
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

fn connect_stream(config: &ImapConfig) -> Result<TcpStream> {
    let addresses = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|error| {
            retryable_imap_network_error(
                config,
                "imap_connect_failed",
                "connect",
                error.to_string(),
            )
        })?;
    connect_addresses(addresses, TcpStream::connect_timeout).map_err(|error| {
        retryable_imap_network_error(config, "imap_connect_failed", "connect", error.to_string())
    })
}

/// DNS resolution precedes this loop; each resolved address has its own bound.
fn connect_addresses<T>(
    addresses: impl IntoIterator<Item = SocketAddr>,
    mut connect: impl FnMut(&SocketAddr, StdDuration) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut last_error = std::io::Error::new(
        std::io::ErrorKind::AddrNotAvailable,
        "IMAP host resolved to no addresses",
    );
    for address in addresses {
        match connect(&address, NETWORK_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
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

/// `LIST` through the raw command path, parsed here.
///
/// The imap crate spends `imap-proto`'s types in its own signatures without
/// re-exporting the crate, so the attributes its `list` returns belong to whichever
/// `imap-proto` that crate happens to depend on — not a type afmail can name. Reading
/// the response here keeps mailbox attributes on afmail's own types, and makes the
/// parse testable against raw bytes the way the COPYUID receipt already is.
pub(crate) fn list_mailboxes<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
) -> Result<Vec<MailboxInfo>> {
    // Same wire form the imap crate's `list(None, Some("*"))` sends: quoted empty
    // reference, unquoted wildcard pattern.
    let (response, _done_at) = session
        .run("LIST \"\" *")
        .map_err(|e| AppError::new("imap_list_failed", e.to_string()))?;
    parse_mailbox_list(&response)
}

/// Collect every untagged `LIST` line in a complete response.
///
/// The raw command path has already turned a non-`OK` tagged completion into an
/// error, so the completion line is just one more response to step over here.
fn parse_mailbox_list(response: &[u8]) -> Result<Vec<MailboxInfo>> {
    let unparsable = || {
        AppError::new(
            "imap_list_failed",
            "server sent a LIST response afmail could not parse",
        )
    };
    let mut rest = response;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (remaining, parsed) = imap_proto::Response::parse(rest).map_err(|_| unparsable())?;
        if let imap_proto::Response::MailboxData(imap_proto::MailboxDatum::List(list)) = &parsed {
            let attributes = list
                .name_attributes
                .iter()
                .map(format_name_attribute)
                .collect::<Vec<_>>();
            out.push(MailboxInfo {
                name: list.name.to_string(),
                delimiter: list.delimiter.as_ref().map(ToString::to_string),
                special_use: special_use_from_attributes(&attributes),
                attributes,
            });
        }
        if remaining.len() >= rest.len() {
            return Err(unparsable());
        }
        rest = remaining;
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
        .map_err(|error| rejected_effect_error("imap_append_failed", error))
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
        uid_move_with_receipt_session(session, uid, target_folder).map(|_| ())
    } else {
        uid_copy_delete_expunge_session(session, uid, target_folder).map(|_| ())
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
    let receipt = if moved {
        if move_supported {
            uid_move_with_receipt_session(session, uid, target_folder)?
        } else {
            uid_copy_delete_expunge_session(session, uid, target_folder)?
        }
    } else {
        None
    };
    // The server's own COPYUID is the authoritative answer to "where did it
    // go". Only without one does afmail fall back to searching the target
    // mailbox, and that search may legitimately come back empty or ambiguous
    // right after the move; both leave the effect completed and the target
    // unknown, which pull + recovery resolve without a second MOVE.
    let receipt_target = receipt
        .as_ref()
        .and_then(|receipt| {
            receipt
                .target_for(uid)
                .map(|target_uid| (receipt, target_uid))
        })
        .map(|(receipt, target_uid)| RemoteLocation {
            mailbox_id: None,
            mailbox_name: target_folder.to_string(),
            uid_validity: Some(receipt.uid_validity),
            uid: Some(target_uid),
            flags: Vec::new(),
            observed_rfc3339: crate::store::now_rfc3339(),
            missing_rfc3339: None,
        });
    let target_location = match (receipt_target, rfc822_message_id) {
        (Some(location), _) => Some(location),
        (None, Some(message_id)) => {
            match find_uid_by_message_id_session(session, target_folder, message_id) {
                Ok(location) => Some(location),
                Err(err)
                    if moved
                        && matches!(err.error_code, "imap_uid_missing" | "imap_uid_ambiguous") =>
                {
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
                        "copyuid_received": receipt.is_some(),
                        "lookup_error_code": err.error_code,
                        "lookup_error": err.message,
                    })));
                }
                Err(err) => return Err(err),
            }
        }
        (None, None) => None,
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

/// `UID MOVE` through the raw command path so the complete response — the
/// untagged `OK [COPYUID …]` RFC 6851 servers send, and the tagged line —
/// is available for the receipt. The imap crate's `uid_mv` discards both.
pub(crate) fn uid_move_with_receipt_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    uid: u64,
    target_folder: &str,
) -> Result<Option<CopyUidReceipt>> {
    let (response, _done_at) = session
        .run(format!(
            "UID MOVE {uid} {}",
            quote_mailbox_name(target_folder)
        ))
        .map_err(|error| rejected_effect_error("imap_move_failed", error))?;
    Ok(parse_copyuid_receipt(&response))
}

pub(crate) fn uid_copy_delete_expunge_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    uid: u64,
    target_folder: &str,
) -> Result<Option<CopyUidReceipt>> {
    let uid_set = uid.to_string();
    // RFC 4315 puts COPY's COPYUID on the tagged OK, which
    // `run_command_and_read_response` would truncate away; `run` keeps it.
    let (response, _done_at) = session
        .run(format!(
            "UID COPY {uid_set} {}",
            quote_mailbox_name(target_folder)
        ))
        .map_err(|error| rejected_effect_error("imap_copy_failed", error))?;
    let receipt = parse_copyuid_receipt(&response);
    session
        .uid_store(&uid_set, "+FLAGS.SILENT (\\Deleted)")
        .map_err(|e| AppError::new("imap_store_failed", e.to_string()))?;
    session
        .uid_expunge(&uid_set)
        .map(|_| ())
        .map_err(|e| AppError::new("imap_expunge_failed", e.to_string()))?;
    Ok(receipt)
}

fn rejected_effect_error(code: &'static str, error: imap::Error) -> AppError {
    let rejected = matches!(error, imap::Error::No(_) | imap::Error::Bad(_));
    let app_error = AppError::new(code, error.to_string());
    if rejected {
        app_error.with_details(json!({"remote_effect_rejected": true}))
    } else {
        app_error
    }
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
    let mut uids = session
        .uid_search(query)
        .map_err(|e| AppError::new("imap_search_failed", e.to_string()))?
        .into_iter()
        .map(u64::from)
        .collect::<Vec<_>>();
    uids.sort_unstable();
    uids.dedup();
    // Message-ID is a header any sender can repeat, so several messages in
    // one mailbox may carry it. Picking the largest UID would silently pin
    // the wrong message as the move target; refuse and let the caller fall
    // back to pull-based recovery instead.
    let uid = match uids.as_slice() {
        [] => {
            return Err(AppError::new(
                "imap_uid_missing",
                "message uid was not found",
            ));
        }
        [uid] => *uid,
        candidates => {
            return Err(AppError::new(
                "imap_uid_ambiguous",
                format!(
                    "{} messages in {folder} share Message-ID {rfc822_message_id}",
                    candidates.len()
                ),
            )
            .with_details(json!({
                "mailbox_name": folder,
                "rfc822_message_id": rfc822_message_id,
                "candidate_uids": candidates,
            })));
        }
    };
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
    use super::{
        CopyUidReceipt, NETWORK_TIMEOUT, connect_addresses, ensure_expected_uid_validity,
        parse_copyuid_receipt, parse_mailbox_list,
    };
    use crate::config::SpecialUseKind;

    #[test]
    fn tagged_imap_rejections_are_distinct_from_lost_connections() {
        struct Reply(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Reply {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                std::io::Read::read(&mut self.0, bytes)
            }
        }
        impl std::io::Write for Reply {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for status in ["NO", "BAD"] {
            let stream = Reply(std::io::Cursor::new(
                format!("a1 {status} rejected\r\n").into_bytes(),
            ));
            let error = imap::Client::new(stream)
                .login("user", "password")
                .err()
                .unwrap()
                .0;
            let error = super::rejected_effect_error("imap_move_failed", error);
            assert_eq!(error.details.unwrap()["remote_effect_rejected"], true);
        }
        let error = super::rejected_effect_error("imap_move_failed", imap::Error::ConnectionLost);
        assert!(error.details.is_none());
    }

    /// A session over a scripted server: reads replay `replies`, writes are
    /// discarded. Running out of replies is the peer closing the connection.
    fn scripted_session(replies: &str) -> imap::Session<ScriptedServer> {
        let stream = ScriptedServer(std::io::Cursor::new(
            format!("a1 OK logged in\r\n{replies}").into_bytes(),
        ));
        match imap::Client::new(stream).login("user", "password") {
            Ok(session) => session,
            Err((error, _)) => panic!("scripted login failed: {error}"),
        }
    }

    struct ScriptedServer(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for ScriptedServer {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            std::io::Read::read(&mut self.0, bytes)
        }
    }
    impl std::io::Write for ScriptedServer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn rejected_marker(error: &crate::error::AppError) -> Option<&serde_json::Value> {
        error
            .details
            .as_ref()
            .and_then(|details| details.get("remote_effect_rejected"))
    }

    #[test]
    fn append_marks_a_tagged_no_rejected_and_a_dropped_connection_unknown() {
        let raw = b"Subject: test\r\n\r\nbody\r\n";

        let mut session = scripted_session("+ go ahead\r\na2 NO [OVERQUOTA] mailbox full\r\n");
        let error =
            super::append_message_session(&mut session, "Sent", raw, false).expect_err("tagged NO");
        assert_eq!(error.error_code, "imap_append_failed");
        assert_eq!(rejected_marker(&error), Some(&serde_json::json!(true)));

        // The literal went out and the connection closed before any tagged
        // reply: the message may or may not be in the mailbox.
        let mut session = scripted_session("+ go ahead\r\n");
        let error = super::append_message_session(&mut session, "Sent", raw, false)
            .expect_err("connection closed");
        assert_eq!(error.error_code, "imap_append_failed");
        assert_eq!(rejected_marker(&error), None);
    }

    #[test]
    fn move_marks_a_tagged_no_rejected_and_a_dropped_connection_unknown() {
        let mut session = scripted_session("a2 NO [TRYCREATE] no such mailbox\r\n");
        let error = super::uid_move_with_receipt_session(&mut session, 7, "Archive")
            .expect_err("tagged NO");
        assert_eq!(error.error_code, "imap_move_failed");
        assert_eq!(rejected_marker(&error), Some(&serde_json::json!(true)));

        let mut session = scripted_session("");
        let error = super::uid_move_with_receipt_session(&mut session, 7, "Archive")
            .expect_err("connection closed");
        assert_eq!(error.error_code, "imap_move_failed");
        assert_eq!(rejected_marker(&error), None);

        let mut session = scripted_session("a2 NO [TRYCREATE] no such mailbox\r\n");
        let error = super::uid_copy_delete_expunge_session(&mut session, 7, "Archive")
            .expect_err("tagged NO");
        assert_eq!(error.error_code, "imap_copy_failed");
        assert_eq!(rejected_marker(&error), Some(&serde_json::json!(true)));
    }

    #[test]
    fn every_resolved_address_uses_the_network_timeout() {
        let addresses = [
            "127.0.0.1:143".parse().expect("first address"),
            "[::1]:143".parse().expect("second address"),
        ];
        let mut attempted = Vec::new();
        let stream = connect_addresses(addresses, |address, timeout| {
            assert_eq!(timeout, NETWORK_TIMEOUT);
            attempted.push(*address);
            if attempted.len() == 1 {
                Err(std::io::Error::from(std::io::ErrorKind::TimedOut))
            } else {
                Ok("connected")
            }
        })
        .expect("try the next resolved address after its predecessor times out");
        assert_eq!(stream, "connected");
        assert_eq!(attempted, addresses);
    }

    #[test]
    fn an_empty_or_failed_address_set_returns_a_connect_error() {
        let addresses = [
            "127.0.0.1:143".parse().expect("first address"),
            "[::1]:143".parse().expect("second address"),
        ];
        let mut attempted = 0;
        let error = connect_addresses::<()>(addresses, |_address, timeout| {
            assert_eq!(timeout, NETWORK_TIMEOUT);
            attempted += 1;
            Err(std::io::Error::from(if attempted == 1 {
                std::io::ErrorKind::TimedOut
            } else {
                std::io::ErrorKind::ConnectionRefused
            }))
        })
        .expect_err("all addresses failed");
        assert_eq!(attempted, 2);
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
        let error = connect_addresses::<()>([], |_address, _timeout| Ok(()))
            .expect_err("no resolved address");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrNotAvailable);
    }

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

    #[test]
    fn copyuid_receipt_is_read_from_the_untagged_move_response() {
        let response = b"* OK [COPYUID 1815441239 562 210] Moved\r\nA0007 OK Move completed\r\n";
        let receipt = parse_copyuid_receipt(response).expect("receipt");
        assert_eq!(receipt.uid_validity, 1815441239);
        assert_eq!(receipt.source_uids, vec![562]);
        assert_eq!(receipt.target_uids, vec![210]);
        assert_eq!(receipt.target_for(562), Some(210));
        assert_eq!(receipt.target_for(563), None);
    }

    #[test]
    fn copyuid_receipt_is_read_from_the_tagged_copy_response() {
        let response = b"A0007 OK [COPYUID 44 7 193] COPY completed\r\n";
        let receipt = parse_copyuid_receipt(response).expect("receipt");
        assert_eq!(
            (receipt.uid_validity, receipt.target_for(7)),
            (44, Some(193))
        );
    }

    #[test]
    fn copyuid_receipt_expands_ranges_pairwise_and_refuses_uneven_sets() {
        let response = b"* OK [COPYUID 44 7:9 100:102] Moved\r\nA1 OK done\r\n";
        let receipt = parse_copyuid_receipt(response).expect("receipt");
        assert_eq!(receipt.target_for(8), Some(101));
        let uneven = CopyUidReceipt {
            uid_validity: 44,
            source_uids: vec![7, 8],
            target_uids: vec![100],
        };
        assert_eq!(uneven.target_for(7), None);
    }

    #[test]
    fn copyuid_receipt_is_absent_when_the_server_sends_none() {
        assert_eq!(
            parse_copyuid_receipt(b"* 1 EXPUNGE\r\nA1 OK Move completed\r\n"),
            None
        );
        assert_eq!(parse_copyuid_receipt(b"garbage without crlf"), None);
    }

    #[test]
    fn mailbox_list_carries_attributes_delimiter_and_special_use() {
        let response = concat!(
            "* LIST (\\HasNoChildren \\Archive) \"/\" \"Archive\"\r\n",
            "* LIST (\\HasChildren) \"/\" INBOX\r\n",
            "A1 OK LIST completed\r\n",
        )
        .as_bytes();
        let mailboxes = parse_mailbox_list(response).expect("mailboxes");

        assert_eq!(
            mailboxes
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Archive", "INBOX"]
        );
        let archive = mailboxes.first().expect("archive");
        assert_eq!(archive.delimiter.as_deref(), Some("/"));
        assert_eq!(archive.attributes, vec!["\\HasNoChildren", "\\Archive"]);
        assert_eq!(archive.special_use, Some(SpecialUseKind::Archive));
        assert_eq!(mailboxes.get(1).and_then(|m| m.special_use.as_ref()), None);
    }

    #[test]
    fn mailbox_list_refuses_a_response_it_cannot_read() {
        let error = parse_mailbox_list(b"* LIST (\\HasNoChildren")
            .err()
            .unwrap_or_else(|| {
                crate::error::AppError::new("test_failure", "expected a parse failure")
            });
        assert_eq!(error.error_code, "imap_list_failed");
    }
}
