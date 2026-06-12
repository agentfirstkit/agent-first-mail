use crate::config::{ImapConfig, SpecialUseKind};
use crate::error::{AppError, Result};
use crate::types::RemoteLocation;
use mail_parser::MessageParser;
use rustls_connector::RustlsConnector;
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
        uid: u64,
        target_folder: &str,
        rfc822_message_id: Option<&str>,
        mark_seen: bool,
        keyword: Option<&str>,
    ) -> Result<MoveOutcome> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => uid_mark_and_move_session(
                session,
                source_folder,
                uid,
                target_folder,
                rfc822_message_id,
                mark_seen,
                keyword,
            ),
            ImapClientSessionInner::Tls(session) => uid_mark_and_move_session(
                session,
                source_folder,
                uid,
                target_folder,
                rfc822_message_id,
                mark_seen,
                keyword,
            ),
        }
    }

    pub fn uid_store_flags(
        &mut self,
        source_folder: &str,
        uid: u64,
        flags: &[String],
        add: bool,
    ) -> Result<()> {
        match self.inner_mut()? {
            ImapClientSessionInner::Plain(session) => {
                uid_store_flags_session(session, source_folder, uid, flags, add)
            }
            ImapClientSessionInner::Tls(session) => {
                uid_store_flags_session(session, source_folder, uid, flags, add)
            }
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
    let stream = TcpStream::connect((config.host.as_str(), config.port))
        .map_err(|e| AppError::new("imap_connect_failed", e.to_string()))?;
    configure_stream_timeout(&stream)?;
    let mut client = imap::Client::new(stream);
    client
        .read_greeting()
        .map_err(|e| AppError::new("imap_greeting_failed", e.to_string()))?;
    client
        .login(&config.username, &config.password_secret)
        .map_err(|e| AppError::new("imap_login_failed", e.0.to_string()))
}

pub(crate) fn login_tls(
    config: &ImapConfig,
) -> Result<imap::Session<rustls_connector::TlsStream<TcpStream>>> {
    let stream = TcpStream::connect((config.host.as_str(), config.port))
        .map_err(|e| AppError::new("imap_connect_failed", e.to_string()))?;
    configure_stream_timeout(&stream)?;
    let connector = RustlsConnector::new_with_webpki_root_certs()
        .map_err(|e| AppError::new("imap_tls_failed", e.to_string()))?;
    let tls_stream = connector
        .connect(&config.host, stream)
        .map_err(|e| AppError::new("imap_tls_failed", e.to_string()))?;
    let mut client = imap::Client::new(tls_stream);
    client
        .read_greeting()
        .map_err(|e| AppError::new("imap_greeting_failed", e.to_string()))?;
    client
        .login(&config.username, &config.password_secret)
        .map_err(|e| AppError::new("imap_login_failed", e.0.to_string()))
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
    let capabilities = session
        .capabilities()
        .map_err(|e| AppError::new("imap_capability_failed", e.to_string()))?;
    Ok(capabilities.has_str("MOVE"))
}

fn format_name_attribute(attribute: &imap::types::NameAttribute<'_>) -> String {
    match attribute {
        imap::types::NameAttribute::NoInferiors => "\\Noinferiors".to_string(),
        imap::types::NameAttribute::NoSelect => "\\Noselect".to_string(),
        imap::types::NameAttribute::Marked => "\\Marked".to_string(),
        imap::types::NameAttribute::Unmarked => "\\Unmarked".to_string(),
        imap::types::NameAttribute::Custom(value) => value.to_string(),
    }
}

pub(crate) fn create_folder<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    folder: &str,
) -> Result<()> {
    session
        .create(folder)
        .map_err(|e| AppError::new("imap_create_failed", e.to_string()))
}

pub(crate) fn append_message_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    folder: &str,
    raw_eml: &[u8],
    draft: bool,
) -> Result<()> {
    if draft {
        session
            .append_with_flags(folder, raw_eml, &[imap::types::Flag::Draft])
            .map_err(|e| AppError::new("imap_append_failed", e.to_string()))
    } else {
        session
            .append(folder, raw_eml)
            .map_err(|e| AppError::new("imap_append_failed", e.to_string()))
    }
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
    require_move(session)?;
    session
        .select(source_folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    session
        .uid_mv(uid.to_string(), target_folder)
        .map_err(|e| AppError::new("imap_move_failed", e.to_string()))
}

pub(crate) fn uid_mark_and_move_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    source_folder: &str,
    uid: u64,
    target_folder: &str,
    rfc822_message_id: Option<&str>,
    mark_seen: bool,
    keyword: Option<&str>,
) -> Result<MoveOutcome> {
    require_move(session)?;
    session
        .select(source_folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
    let (seen_set, seen_error) = if mark_seen {
        session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Seen)")
            .map(|_| (true, None))
            .map_err(|e| AppError::new("imap_store_failed", e.to_string()))?
    } else {
        (false, None)
    };
    let (keyword_set, keyword_error) = if let Some(keyword) = keyword {
        let keyword_result =
            session.uid_store(uid.to_string(), format!("+FLAGS.SILENT ({keyword})"));
        match keyword_result {
            Ok(_) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        }
    } else {
        (false, None)
    };
    let moved = source_folder != target_folder;
    if moved {
        session
            .uid_mv(uid.to_string(), target_folder)
            .map_err(|e| AppError::new("imap_move_failed", e.to_string()))?;
    }
    let target_location = match rfc822_message_id {
        Some(message_id) => Some(find_uid_by_message_id_session(
            session,
            target_folder,
            message_id,
        )?),
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
    uid: u64,
    flags: &[String],
    add: bool,
) -> Result<()> {
    session
        .select(source_folder)
        .map_err(|e| AppError::new("imap_select_failed", e.to_string()))?;
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
        .map(|uid| uid as u64);
    let uid = match uid {
        Some(uid) => uid,
        None => fetch_uid_by_message_id_session(session, rfc822_message_id)?
            .ok_or_else(|| AppError::new("imap_uid_missing", "moved message uid was not found"))?,
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

pub(crate) fn fetch_uid_by_message_id_session<T: std::io::Read + std::io::Write>(
    session: &mut imap::Session<T>,
    rfc822_message_id: &str,
) -> Result<Option<u64>> {
    let target = normalize_message_id(rfc822_message_id);
    let fetches = session
        .uid_fetch("1:*", "(UID BODY.PEEK[HEADER])")
        .map_err(|e| AppError::new("imap_fetch_failed", e.to_string()))?;
    let mut uid = None;
    for fetch in fetches.iter() {
        let Some(candidate_uid) = fetch.uid else {
            continue;
        };
        let Some(body) = fetch.header().or_else(|| fetch.body()) else {
            continue;
        };
        if header_body_contains_message_id(body, &target) {
            uid = Some(candidate_uid as u64);
        }
    }
    Ok(uid)
}

fn header_body_contains_message_id(body: &[u8], target: &str) -> bool {
    if let Some(message_id) = rfc822_message_id(body) {
        if normalize_message_id(&message_id) == target {
            return true;
        }
    }
    String::from_utf8_lossy(body)
        .split(['<', '>', ',', ';', ' ', '\t', '\r', '\n'])
        .map(normalize_message_id)
        .any(|message_id| message_id == target)
}

fn rfc822_message_id(raw_eml: &[u8]) -> Option<String> {
    MessageParser::default()
        .parse(raw_eml)
        .and_then(|message| message.message_id().map(ToString::to_string))
}

fn normalize_message_id(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| matches!(ch, '<' | '>' | ',' | ';'))
        .trim()
        .to_ascii_lowercase()
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
