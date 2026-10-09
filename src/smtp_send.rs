use crate::config::{MailConfig, ResolvedIdentity, SmtpConfig};
use crate::draft_content::outbound_body;
use crate::error::{AppError, Result};
use crate::frontmatter::DraftFrontmatter;
use crate::mail::parse_outbound_message_with_status;
use crate::markdown::read_doc;
use crate::types::PreparedOutbound;
#[cfg(test)]
use crate::types::{MessageAuthentication, MessageFile};
use crate::util::write_bytes_atomic;
#[cfg(test)]
use crate::util::write_json_pretty;
use chrono::DateTime;
use lettre::address::{Address, Envelope};
#[cfg(feature = "review")]
use lettre::message::Mailboxes;
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart, header};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use sanitize_filename::{Options as SanitizeFilenameOptions, sanitize_with_options};
use std::fs;
use std::path::Path;

pub fn prepare_outbound(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    draft_name: &str,
    config: &MailConfig,
    existing_message_id: Option<&str>,
    message_date_rfc3339: Option<&str>,
) -> Result<PreparedOutbound> {
    let message_id = existing_message_id
        .map(ToString::to_string)
        .unwrap_or_else(|| unique_outbound_id(root));
    let message = build_draft_message(
        root,
        case_path,
        case_uid,
        draft_name,
        config,
        &message_id,
        message_date_rfc3339,
    )?;
    let raw = message.formatted();
    let envelope = message.envelope();
    let envelope_from = envelope
        .from()
        .map(ToString::to_string)
        .ok_or_else(|| AppError::new("draft_invalid", "draft envelope from is required"))?;
    let envelope_to = envelope
        .to()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut delivery_message = message;
    delivery_message.headers_mut().remove::<header::Bcc>();
    let delivery_raw = delivery_message.formatted();
    Ok(PreparedOutbound {
        message_id,
        raw,
        delivery_raw,
        envelope_from,
        envelope_to,
    })
}

pub fn message_id_for_push(push_id: &str) -> String {
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

pub fn mark_sent_and_append_case(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    message_id: &str,
    raw: &[u8],
    _config: &MailConfig,
) -> Result<()> {
    let sent = crate::store::now_rfc3339();
    let parsed = parse_outbound_message_with_status(
        message_id.to_string(),
        raw,
        case_uid.to_string(),
        "case".to_string(),
        Some(sent),
    )?;
    let messages_dir = root.join(".afmail/messages");
    fs::create_dir_all(&messages_dir).map_err(|e| AppError::io("create messages dir", &e))?;
    let eml_path = messages_dir.join(format!("{message_id}.eml"));
    if eml_path.is_file() {
        let existing =
            fs::read(&eml_path).map_err(|error| AppError::io("read existing sent eml", &error))?;
        if existing != raw {
            return Err(sent_message_id_conflict(message_id));
        }
    } else {
        let metadata_path = messages_dir.join(format!("{message_id}.json"));
        let materialized_path = root.join("messages").join(format!("{message_id}.json"));
        if metadata_path.exists() || materialized_path.exists() {
            return Err(sent_message_id_conflict(message_id));
        }
        write_bytes_atomic(&eml_path, raw, "write sent eml")?;
    }
    let workspace = crate::store::Workspace::at(root);
    workspace.write_message_artifacts(&parsed.message)?;
    let added_rfc3339 = parsed
        .message
        .sent_rfc3339
        .clone()
        .unwrap_or_else(crate::store::now_rfc3339);
    workspace.append_sent_message_to_case(
        case_path,
        case_uid,
        message_id,
        parsed.message.subject.as_deref(),
        &added_rfc3339,
    )
}

fn sent_message_id_conflict(message_id: &str) -> AppError {
    AppError::new(
        "message_id_conflict",
        format!("sent message id already belongs to different local content: {message_id}"),
    )
    .with_hint(
        "Keep the existing sent message evidence and requeue the draft so afmail can allocate a new outbound identity.",
    )
    .with_details(serde_json::json!({
        "message_id": message_id,
    }))
}

/// Everything a send needs before anything can leave the machine.
///
/// The relay configuration, the transport and the envelope are all built from
/// local state, and every way they can fail is a mistake that delivered
/// nothing. Doing it here lets the caller find those out while the step is
/// still plainly retryable, instead of after it has written down that this
/// step's outcome is unknown — which is a state a queue cannot recover from on
/// its own.
pub fn prepare_send(
    config: &MailConfig,
    envelope_from: &str,
    envelope_to: &[String],
) -> Result<(lettre::SmtpTransport, Envelope)> {
    let smtp = config.require_smtp()?;
    let sender = build_transport(&smtp)?;
    let from = envelope_from
        .parse::<Address>()
        .map_err(|e| AppError::new("smtp_send_failed", format!("invalid envelope from: {e}")))?;
    let mut to = Vec::new();
    for address in envelope_to {
        to.push(address.parse::<Address>().map_err(|e| {
            AppError::new(
                "smtp_send_failed",
                format!("invalid envelope recipient: {e}"),
            )
        })?);
    }
    let envelope = Envelope::new(Some(from), to)
        .map_err(|e| AppError::new("smtp_send_failed", e.to_string()))?;
    Ok((sender, envelope))
}

pub fn send_raw_message(
    config: &MailConfig,
    envelope_from: &str,
    envelope_to: &[String],
    raw: &[u8],
) -> Result<()> {
    let (sender, envelope) = prepare_send(config, envelope_from, envelope_to)?;
    // From here on the outcome genuinely can be unknown: the connection, the
    // handshake, the envelope exchange and DATA all happen inside this call.
    // A negative SMTP reply is different: the server explicitly rejected it.
    sender.send_raw(&envelope, raw).map_err(|error| {
        let rejected = error.is_permanent() || error.is_transient();
        let app_error = AppError::new("smtp_send_failed", error.to_string());
        if rejected {
            app_error.with_details(serde_json::json!({"remote_effect_rejected": true}))
        } else {
            app_error
        }
    })?;
    Ok(())
}

fn build_draft_message(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    draft_name: &str,
    config: &MailConfig,
    message_id: &str,
    message_date_rfc3339: Option<&str>,
) -> Result<Message> {
    let draft_path = case_path.join("drafts").join(draft_name);
    let draft_text = fs::read_to_string(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
    let (fm, raw_body) = read_doc::<DraftFrontmatter>(&draft_text)?;
    let body = outbound_body(&raw_body)
        .map_err(|error| AppError::new("draft_invalid", error.to_string()))?;
    let identity = config
        .resolve_identity(root, fm.identity.as_deref())
        .map_err(identity_draft_error)?;
    let rfc822_message_id = format!("<{message_id}@afmail.local>");
    build_message(
        root,
        case_path,
        case_uid,
        &fm,
        &body,
        &identity,
        StableMessageHeaders {
            rfc822_message_id: &rfc822_message_id,
            message_id,
            date_rfc3339: message_date_rfc3339,
        },
    )
}

#[derive(Clone, Copy)]
struct StableMessageHeaders<'a> {
    rfc822_message_id: &'a str,
    message_id: &'a str,
    date_rfc3339: Option<&'a str>,
}

fn build_message(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    fm: &DraftFrontmatter,
    body: &str,
    identity: &ResolvedIdentity,
    stable_headers: StableMessageHeaders<'_>,
) -> Result<Message> {
    if fm.case_uid != case_uid {
        return Err(AppError::new(
            "draft_invalid",
            "draft case_uid does not match case",
        ));
    }
    let mut builder = Message::builder()
        .from(identity_mailbox(identity)?)
        .message_id(Some(stable_headers.rfc822_message_id.to_string()))
        .subject(
            fm.subject
                .clone()
                .ok_or_else(|| AppError::new("draft_invalid", "draft subject is required"))?,
        );
    if let Some(value) = stable_headers.date_rfc3339 {
        let date = DateTime::parse_from_rfc3339(value).map_err(|error| {
            AppError::new(
                "push_item_invalid",
                format!("queued_rfc3339 is invalid: {error}"),
            )
        })?;
        builder = builder.date(std::time::SystemTime::from(date));
    }
    for to in &fm.to {
        builder = builder.to(parse_mailbox(to, "to")?);
    }
    for cc in &fm.cc {
        builder = builder.cc(parse_mailbox(cc, "cc")?);
    }
    for bcc in &fm.bcc {
        builder = builder.bcc(parse_mailbox(bcc, "bcc")?);
    }
    // Keep Bcc in the private Drafts/Sent evidence. `prepare_outbound` derives
    // separate SMTP DATA bytes by removing this header while preserving the
    // envelope recipients that lettre computed from it.
    builder = builder.keep_bcc();
    if let Some(reply_id) = fm.reply_to_message_id.as_ref() {
        let headers = reply_headers(root, reply_id)?;
        builder = builder
            .in_reply_to(headers.in_reply_to)
            .references(headers.references);
    }
    let attachments = &fm.attachments;
    if attachments.is_empty() {
        return builder
            .header(header::ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| AppError::new("draft_invalid", e.to_string()));
    }
    let mut multipart = MultiPart::mixed()
        .boundary(format!("afmail-{}-mixed", stable_headers.message_id))
        .singlepart(SinglePart::plain(body.to_string()));
    for attachment in attachments {
        let open = crate::attachments::open_draft_attachment(case_path, attachment)?;
        let data = crate::attachments::read_attachment(&open)?;
        let filename = safe_outbound_attachment_filename(&open.filename);
        let content_type_str = mime_guess::from_path(&filename)
            .first_raw()
            .unwrap_or("application/octet-stream");
        let content_type = header::ContentType::parse(content_type_str)
            .map_err(|e| AppError::new("draft_invalid", e.to_string()))?;
        multipart = multipart.singlepart(Attachment::new(filename).body(data, content_type));
    }
    builder
        .multipart(multipart)
        .map_err(|e| AppError::new("draft_invalid", e.to_string()))
}

fn identity_mailbox(identity: &ResolvedIdentity) -> Result<Mailbox> {
    let address = identity
        .email
        .parse::<Address>()
        .map_err(|e| AppError::new("draft_invalid", format!("invalid identity email: {e}")))?;
    Ok(Mailbox::new(Some(identity.name.clone()), address))
}

fn identity_draft_error(err: AppError) -> AppError {
    match err.error_code {
        "unknown_identity" | "config_invalid" => AppError::new("draft_invalid", err.message),
        _ => err,
    }
}

fn safe_outbound_attachment_filename(filename: &str) -> String {
    let sanitized = sanitize_with_options(
        filename.trim(),
        SanitizeFilenameOptions {
            windows: true,
            truncate: true,
            replacement: "_",
        },
    );
    if sanitized.trim().is_empty() {
        "attachment".to_string()
    } else {
        sanitized.trim().to_string()
    }
}

fn build_transport(config: &SmtpConfig) -> Result<SmtpTransport> {
    let mut builder = if config.tls_wrapper {
        SmtpTransport::relay(&config.host)
            .map_err(|e| AppError::new("smtp_connect_failed", e.to_string()))?
            .port(config.port)
    } else if config.starttls {
        SmtpTransport::starttls_relay(&config.host)
            .map_err(|e| AppError::new("smtp_connect_failed", e.to_string()))?
            .port(config.port)
    } else {
        crate::util::ensure_plaintext_credentials_allowed(
            "SMTP",
            &config.host,
            config.username.is_some() && config.password_secret.is_some(),
        )?;
        SmtpTransport::builder_dangerous(&config.host).port(config.port)
    };
    if let (Some(username), Some(password)) = (&config.username, &config.password_secret) {
        builder = builder.credentials(Credentials::new(username.clone(), password.clone()));
    }
    Ok(builder.build())
}

pub(crate) fn remote_test(config: &SmtpConfig) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let transport = build_transport(config)?;
    let connected = transport
        .test_connection()
        .map_err(|error| retryable_smtp_test_error(config, "connect", error.to_string()))?;
    if !connected {
        return Err(retryable_smtp_test_error(
            config,
            "connect",
            "SMTP server closed the test connection".to_string(),
        ));
    }
    Ok(serde_json::json!({
        "status": "succeeded",
        "host": config.host,
        "port": config.port,
        "starttls": config.starttls,
        "tls_wrapper": config.tls_wrapper,
        "authenticated": config.username.is_some(),
        "message_sent": false,
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

fn retryable_smtp_test_error(
    config: &SmtpConfig,
    phase: &'static str,
    message: String,
) -> AppError {
    AppError::retryable("smtp_test_failed", message).with_details(serde_json::json!({
        "protocol": "smtp",
        "phase": phase,
        "host": config.host,
        "port": config.port,
        "starttls": config.starttls,
        "tls_wrapper": config.tls_wrapper
    }))
}

pub(crate) fn normalize_recipient_address(value: &str, field: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.parse::<Mailbox>().is_ok() {
        return Ok(trimmed.to_string());
    }
    fallback_angle_address(trimmed)
        .map(|address| address.to_string())
        .ok_or_else(|| AppError::new("draft_invalid", format!("invalid {field} address")))
}

#[cfg(feature = "review")]
pub(crate) fn normalize_recipient_list_text(value: &str, field: &str) -> Result<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    trimmed
        .parse::<Mailboxes>()
        .map(|mailboxes| {
            mailboxes
                .into_iter()
                .map(|mailbox| mailbox.to_string())
                .collect()
        })
        .map_err(|error| {
            AppError::new(
                "draft_invalid",
                format!("invalid {field} address list: {error}"),
            )
        })
}

fn parse_mailbox(value: &str, field: &str) -> Result<Mailbox> {
    let trimmed = value.trim();
    match trimmed.parse::<Mailbox>() {
        Ok(mailbox) => Ok(mailbox),
        Err(err) => {
            if let Some(address) = fallback_angle_address(trimmed) {
                Ok(Mailbox::new(None, address))
            } else {
                Err(AppError::new(
                    "draft_invalid",
                    format!("invalid {field} address: {err}"),
                ))
            }
        }
    }
}

fn fallback_angle_address(value: &str) -> Option<Address> {
    let (display, rest) = value.rsplit_once('<')?;
    if display.trim().is_empty() || !rest.ends_with('>') {
        return None;
    }
    let address = rest.strip_suffix('>')?.trim();
    address.parse::<Address>().ok()
}

struct ReplyHeaders {
    in_reply_to: String,
    references: String,
}

/// Build RFC 5322 threading headers for a reply to `message_id`.
/// `In-Reply-To` is the parent's own Message-ID; `References` is the parent's
/// own References chain plus the parent's Message-ID appended.
fn reply_headers(root: &Path, message_id: &str) -> Result<ReplyHeaders> {
    let message = crate::store::Workspace::at(root).read_message_by_id(message_id)?;
    let parent_id = message.rfc822_message_id.ok_or_else(|| {
        AppError::new(
            "draft_invalid",
            format!("reply message has no rfc822_message_id: {message_id}"),
        )
    })?;
    let mut refs = message.references.clone();
    if !refs.contains(&parent_id) {
        refs.push(parent_id.clone());
    }
    let references = refs
        .iter()
        .map(|id| ensure_brackets(id))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(ReplyHeaders {
        in_reply_to: ensure_brackets(&parent_id),
        references,
    })
}

/// Wrap a bare message-id in angle brackets unless it already has them.
fn ensure_brackets(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        trimmed.to_string()
    } else {
        format!("<{trimmed}>")
    }
}

fn unique_outbound_id(root: &Path) -> String {
    let base = format!(
        "message_sent_{}",
        crate::store::now_rfc3339().replace([':', '-'], "")
    );
    let dir = root.join(".afmail/messages");
    if !dir.join(format!("{base}.json")).exists() {
        return base;
    }
    for i in 1..1000 {
        let candidate = format!("{base}_{i}");
        if !dir.join(format!("{candidate}.json")).exists() {
            return candidate;
        }
    }
    base
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
        std::env::temp_dir().join(format!("afmail-smtp-{name}-{}-{stamp}", std::process::id()))
    }

    #[test]
    fn everything_that_cannot_have_sent_mail_fails_before_the_send() {
        // The queue writes "outcome unknown" immediately before the send and
        // cannot recover from it on its own, so a failure that plainly
        // delivered nothing has to surface earlier than that. These are all
        // local construction, and `prepare_send` is where they now happen.
        let mut config = MailConfig::default();
        config.smtp.host = Some("smtp.example.com".to_string());
        config.smtp.port = 587;
        config.smtp.starttls = true;

        // A recipient the envelope will not accept.
        assert!(
            prepare_send(&config, "me@example.com", &["not an address".to_string()]).is_err(),
            "an unusable recipient must fail before anything is sent"
        );
        // A sender the envelope will not accept.
        assert!(prepare_send(&config, "@", &["you@example.com".to_string()]).is_err());
        // No relay host at all.
        let unconfigured = MailConfig::default();
        assert!(
            prepare_send(
                &unconfigured,
                "me@example.com",
                &["you@example.com".to_string()]
            )
            .is_err()
        );

        // A usable configuration builds a transport and an envelope without
        // opening a connection — this must not touch the network.
        assert!(
            prepare_send(&config, "me@example.com", &["you@example.com".to_string()]).is_ok(),
            "preparation must not require the server to be reachable"
        );
    }

    #[derive(Clone, Copy)]
    enum SmtpScript {
        RejectRecipient,
        DeferSender,
        CloseAfterData,
    }

    /// One-connection SMTP server on a loopback port that follows `script`.
    fn scripted_smtp_server(script: SmtpScript) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind SMTP listener");
        let port = listener.local_addr().expect("SMTP listener address").port();
        let handle = std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let Ok(mut writer) = stream.try_clone() else {
                return;
            };
            let mut reader = BufReader::new(stream);
            let _ = writer.write_all(b"220 localhost ESMTP\r\n");
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let upper = line.to_ascii_uppercase();
                let reply: &[u8] = if upper.starts_with("MAIL FROM") {
                    match script {
                        SmtpScript::DeferSender => b"451 4.3.0 try again later\r\n",
                        _ => b"250 OK\r\n",
                    }
                } else if upper.starts_with("RCPT TO") {
                    match script {
                        SmtpScript::RejectRecipient => b"550 5.1.1 no such user\r\n",
                        _ => b"250 OK\r\n",
                    }
                } else if upper.starts_with("DATA") {
                    let _ = writer.write_all(b"354 end with <CRLF>.<CRLF>\r\n");
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == ".\r\n" {
                            break;
                        }
                    }
                    match script {
                        // The whole message arrived and the server said
                        // nothing: it may or may not have accepted it.
                        SmtpScript::CloseAfterData => return,
                        _ => b"250 OK\r\n",
                    }
                } else if upper.starts_with("QUIT") {
                    let _ = writer.write_all(b"221 bye\r\n");
                    return;
                } else {
                    b"250 OK\r\n"
                };
                if writer.write_all(reply).is_err() {
                    return;
                }
            }
        });
        (port, handle)
    }

    fn send_to_scripted_server(script: SmtpScript) -> AppError {
        let (port, server) = scripted_smtp_server(script);
        let mut config = MailConfig::default();
        config.smtp.host = Some("127.0.0.1".to_string());
        config.smtp.port = port;
        config.smtp.starttls = false;
        config.smtp.tls_wrapper = false;
        let result = send_raw_message(
            &config,
            "me@example.com",
            &["you@example.com".to_string()],
            b"Subject: test\r\n\r\nbody\r\n",
        );
        assert!(server.join().is_ok());
        result.expect_err("the scripted server never accepts a message")
    }

    #[test]
    fn a_negative_smtp_reply_is_rejected_and_a_dropped_connection_is_unknown() {
        for script in [SmtpScript::RejectRecipient, SmtpScript::DeferSender] {
            let error = send_to_scripted_server(script);
            assert_eq!(error.error_code, "smtp_send_failed");
            assert_eq!(
                error
                    .details
                    .as_ref()
                    .and_then(|details| details.get("remote_effect_rejected")),
                Some(&serde_json::json!(true)),
                "{error:?}"
            );
        }
        let error = send_to_scripted_server(SmtpScript::CloseAfterData);
        assert_eq!(error.error_code, "smtp_send_failed");
        assert!(error.details.is_none(), "{error:?}");
    }

    #[test]
    fn sent_message_record_is_idempotent_but_never_overwrites_different_raw() {
        let root = temp_root("sent-message-conflict");
        assert!(crate::store::Workspace::at(&root).init().is_ok());
        let case_uid = "c202608030001";
        let case_path = root.join(format!("cases/general/{case_uid}-test"));
        assert!(fs::create_dir_all(case_path.join("data")).is_ok());
        let case = crate::types::CaseMessages::new_case(case_uid, "test", "2026-08-03T00:00:00Z");
        assert!(crate::util::write_json_pretty(&case_path.join("data/case.json"), &case,).is_ok());
        let config = MailConfig::default();
        let raw = b"Message-ID: <message_sent_test@afmail.local>\r\nFrom: a@example.com\r\nTo: b@example.com\r\nSubject: first\r\n\r\nfirst";
        let changed = b"Message-ID: <message_sent_test@afmail.local>\r\nFrom: a@example.com\r\nTo: b@example.com\r\nSubject: second\r\n\r\nsecond";

        let first = mark_sent_and_append_case(
            &root,
            &case_path,
            case_uid,
            "message_sent_test",
            raw,
            &config,
        );
        assert!(first.is_ok(), "{first:?}");
        let retry = mark_sent_and_append_case(
            &root,
            &case_path,
            case_uid,
            "message_sent_test",
            raw,
            &config,
        );
        assert!(retry.is_ok(), "{retry:?}");
        let conflict = mark_sent_and_append_case(
            &root,
            &case_path,
            case_uid,
            "message_sent_test",
            changed,
            &config,
        );
        assert_eq!(
            conflict.err().map(|error| error.error_code),
            Some("message_id_conflict")
        );
        assert_eq!(
            fs::read(root.join(".afmail/messages/message_sent_test.eml")).ok(),
            Some(raw.to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    fn draft(text: &str) -> DraftFrontmatter {
        crate::markdown::parse_frontmatter(text).unwrap_or_default()
    }

    fn test_identity() -> ResolvedIdentity {
        ResolvedIdentity {
            identity: "me".to_string(),
            name: "Me".to_string(),
            email: "me@example.com".to_string(),
            default: true,
            footer: None,
            notes: None,
        }
    }

    fn stable_headers(rfc822_message_id: &str) -> StableMessageHeaders<'_> {
        StableMessageHeaders {
            rfc822_message_id,
            message_id: "test-message",
            date_rfc3339: Some("2026-05-21T10:00:00Z"),
        }
    }

    #[test]
    fn builds_plain_draft_message() {
        let root = temp_root("build");
        let case_path = root.join("cases/support/c20260521001");
        let _ = fs::create_dir_all(case_path.join("drafts"));
        let fm = draft(
            "kind: draft\ncase_uid: c20260521001\nto:\n  - alice@example.com\nsubject: Hello",
        );
        let msg = build_message(
            &root,
            &case_path,
            "c20260521001",
            &fm,
            "Hi",
            &test_identity(),
            stable_headers("<msg@example.com>"),
        );
        assert!(msg.is_ok());
        let raw = msg
            .map(|m| String::from_utf8(m.formatted()).unwrap_or_default())
            .unwrap_or_default();
        assert!(raw.contains("Subject: Hello"));
        assert!(raw.contains("Hi"));
        assert!(raw.contains("Message-ID: <msg@example.com>"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bcc_is_in_the_envelope_and_private_evidence_but_not_smtp_data() {
        let root = temp_root("bcc-delivery");
        let case_uid = "c20260521001";
        let case_path = root.join(format!("cases/support/{case_uid}"));
        let drafts_path = case_path.join("drafts");
        assert!(fs::create_dir_all(&drafts_path).is_ok());
        let draft_name = "new-private.md";
        let draft_text = format!(
            "---\nkind: draft\ncase_uid: {case_uid}\nsend_intent: new\nsubject: Private update\nto: []\ncc: []\nbcc:\n  - Hidden One <hidden-one@example.com>\n  - hidden-two@example.com\nattachments: []\n---\n<!-- afmail:compose:start -->\nPrivate body\n<!-- afmail:compose:end -->\n"
        );
        assert!(fs::write(drafts_path.join(draft_name), draft_text).is_ok());

        let prepared = prepare_outbound(
            &root,
            &case_path,
            case_uid,
            draft_name,
            &MailConfig::default(),
            Some("message_sent_bcc"),
            Some("2026-05-21T10:00:00Z"),
        );
        assert!(prepared.is_ok(), "{prepared:?}");
        let prepared = prepared.unwrap_or_else(|_| PreparedOutbound {
            message_id: String::new(),
            raw: Vec::new(),
            delivery_raw: Vec::new(),
            envelope_from: String::new(),
            envelope_to: Vec::new(),
        });
        assert_eq!(
            prepared.envelope_to,
            vec![
                "hidden-one@example.com".to_string(),
                "hidden-two@example.com".to_string()
            ]
        );
        let private_raw = String::from_utf8(prepared.raw).unwrap_or_default();
        let delivery_raw = String::from_utf8(prepared.delivery_raw).unwrap_or_default();
        assert!(private_raw.contains("Bcc:"));
        assert!(private_raw.contains("hidden-one@example.com"));
        assert!(private_raw.contains("hidden-two@example.com"));
        assert!(!delivery_raw.contains("\r\nBcc:"));
        assert!(!delivery_raw.starts_with("Bcc:"));
        assert!(!delivery_raw.contains("hidden-one@example.com"));
        assert!(!delivery_raw.contains("hidden-two@example.com"));
        assert!(delivery_raw.contains("Private body"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_message_with_email_as_display_name_recipient() {
        let root = temp_root("email-display");
        let case_path = root.join("cases/support/c20260521001");
        let _ = fs::create_dir_all(case_path.join("drafts"));
        let fm = draft(
            "kind: draft\ncase_uid: c20260521001\nto:\n  - alice@example.com <alice@example.com>\nsubject: Hello",
        );
        let msg = build_message(
            &root,
            &case_path,
            "c20260521001",
            &fm,
            "Hi",
            &test_identity(),
            stable_headers("<msg@example.com>"),
        );
        assert!(msg.is_ok());
        let envelope_to = msg
            .map(|message| {
                message
                    .envelope()
                    .to()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(envelope_to, vec!["alice@example.com"]);
        let _ = fs::remove_dir_all(root);
    }

    fn write_parent(root: &Path, message_id: &str, rfc822_id: &str, references: &[&str]) {
        let _ = fs::create_dir_all(root.join(".afmail"));
        let _ = write_json_pretty(&root.join(".afmail/config.json"), &MailConfig::default());
        let msg = MessageFile {
            schema_name: "message".to_string(),
            schema_version: crate::types::MESSAGE_SCHEMA_VERSION,
            message_id: message_id.to_string(),
            rfc822_message_id: Some(rfc822_id.to_string()),
            in_reply_to: None,
            references: references.iter().map(|s| s.to_string()).collect(),
            remote: None,
            agent_suggestion: None,
            human_suggestion: None,
            case_relationship_suggestion: None,
            direction: Some("inbound".to_string()),
            subject: Some("Hi".to_string()),
            from: Some("a@example.com".to_string()),
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
            authentication: MessageAuthentication::default(),
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
                status: "triage".to_string(),
                notification_uid: None,
                notification_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
        };
        let _ = crate::store::Workspace::at(root).write_message_materialized_cache(&msg);
    }

    #[test]
    fn reply_builds_full_references_chain() {
        let root = temp_root("reply-chain");
        let case_path = root.join("cases/support/c20260521001");
        let _ = fs::create_dir_all(case_path.join("drafts"));
        // Parent already carries a References chain (bracket-less, as stored).
        write_parent(
            &root,
            "message_p",
            "parent@example.com",
            &["root@example.com"],
        );
        let identity = test_identity();
        let fm = draft(
            "kind: draft\ncase_uid: c20260521001\nto:\n  - a@example.com\nsubject: \"Re: Hi\"\nreply_to_message_id: message_p",
        );
        let message = build_message(
            &root,
            &case_path,
            "c20260521001",
            &fm,
            "reply body",
            &identity,
            stable_headers("<reply@afmail.local>"),
        );
        assert!(message.is_ok(), "{:?}", message.as_ref().err());
        let raw = message
            .map(|m| String::from_utf8(m.formatted()).unwrap_or_default())
            .unwrap_or_default();
        assert!(raw.contains("In-Reply-To: <parent@example.com>"), "{raw}");
        assert!(raw.contains("References: <root@example.com> <parent@example.com>"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reply_without_parent_references_falls_back_to_parent_id() {
        let root = temp_root("reply-fallback");
        let case_path = root.join("cases/support/c20260521001");
        let _ = fs::create_dir_all(case_path.join("drafts"));
        write_parent(&root, "message_p", "parent@example.com", &[]);
        let identity = test_identity();
        let fm = draft(
            "kind: draft\ncase_uid: c20260521001\nto:\n  - a@example.com\nsubject: \"Re: Hi\"\nreply_to_message_id: message_p",
        );
        let message = build_message(
            &root,
            &case_path,
            "c20260521001",
            &fm,
            "reply body",
            &identity,
            stable_headers("<reply@afmail.local>"),
        );
        assert!(message.is_ok(), "{:?}", message.as_ref().err());
        let raw = message
            .map(|m| String::from_utf8(m.formatted()).unwrap_or_default())
            .unwrap_or_default();
        assert!(raw.contains("In-Reply-To: <parent@example.com>"), "{raw}");
        assert!(raw.contains("References: <parent@example.com>"));
        let _ = fs::remove_dir_all(root);
    }
}
