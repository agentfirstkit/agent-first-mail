use crate::config::{MailConfig, ResolvedIdentity, SmtpConfig};
use crate::draft_content::outbound_body;
use crate::error::{AppError, Result};
use crate::frontmatter::{CaseFrontmatter, DraftFrontmatter};
use crate::mail::parse_outbound_message_with_status;
use crate::markdown::read_doc;
use crate::types::{CaseMessages, PreparedOutbound};
#[cfg(test)]
use crate::types::{MessageAuthentication, MessageFile};
use crate::util::{write_bytes_atomic, write_json_pretty};
use chrono::DateTime;
use lettre::address::{Address, Envelope};
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart, header};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use sanitize_filename::{Options as SanitizeFilenameOptions, sanitize_with_options};
use std::fs;
use std::path::{Path, PathBuf};

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
    Ok(PreparedOutbound {
        message_id,
        raw,
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
    write_bytes_atomic(
        &messages_dir.join(format!("{message_id}.eml")),
        raw,
        "write sent eml",
    )?;
    crate::store::Workspace::at(root).write_message_artifacts(&parsed.message)?;
    let mut case_data = read_case_messages(case_path, case_uid)?;
    let added_rfc3339 = parsed
        .message
        .sent_rfc3339
        .clone()
        .unwrap_or_else(crate::store::now_rfc3339);
    case_data.upsert_item(
        message_id,
        parsed.message.subject.as_deref(),
        &added_rfc3339,
    );
    update_case_metadata_after_append(case_path, &case_data)?;
    Ok(())
}

pub fn send_raw_message(
    config: &MailConfig,
    envelope_from: &str,
    envelope_to: &[String],
    raw: &[u8],
) -> Result<()> {
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
    sender
        .send_raw(&envelope, raw)
        .map_err(|e| AppError::new("smtp_send_failed", e.to_string()))?;
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
        let path = draft_attachment_path(case_path, attachment)?;
        let data = fs::read(&path).map_err(|e| AppError::io("read draft attachment", &e))?;
        let raw_filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(ToString::to_string)
            .ok_or_else(|| AppError::new("draft_invalid", "invalid attachment file name"))?;
        let filename = safe_outbound_attachment_filename(&raw_filename);
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

fn draft_attachment_path(case_path: &Path, attachment: &str) -> Result<PathBuf> {
    let path = Path::new(attachment);
    if attachment.trim().is_empty() || path.is_absolute() {
        return Err(AppError::new(
            "draft_invalid",
            format!("invalid draft attachment path: {attachment}"),
        ));
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => safe.push(part),
            _ => {
                return Err(AppError::new(
                    "draft_invalid",
                    format!("invalid draft attachment path: {attachment}"),
                ));
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err(AppError::new(
            "draft_invalid",
            format!("invalid draft attachment path: {attachment}"),
        ));
    }
    Ok(case_path.join(safe))
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

fn read_case_messages(case_path: &Path, case_uid: &str) -> Result<CaseMessages> {
    let path = case_path.join("data").join("case.json");
    let data = fs::read_to_string(&path).map_err(|e| AppError::io("read case metadata", &e))?;
    let messages: CaseMessages =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse case metadata", &e))?;
    if messages.schema_name != crate::types::CASE_SCHEMA_NAME
        || messages.schema_version != crate::types::MESSAGE_COLLECTION_SCHEMA_VERSION
        || messages.collection_uid != case_uid
    {
        return Err(AppError::new(
            "case_metadata_invalid",
            "invalid case metadata schema",
        ));
    }
    Ok(messages)
}

fn update_case_metadata_after_append(case_path: &Path, case: &CaseMessages) -> Result<()> {
    let path = case_path.join("data").join("case.json");
    let mut updated: CaseFrontmatter = case.clone();
    updated.updated_rfc3339 = Some(crate::store::now_rfc3339());
    updated.normalize(
        crate::types::CASE_SCHEMA_NAME,
        &case.collection_uid,
        &case.collection_name,
    );
    write_json_pretty(&path, &updated)
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
            schema_version: 1,
            message_id: message_id.to_string(),
            rfc822_message_id: Some(rfc822_id.to_string()),
            in_reply_to: None,
            references: references.iter().map(|s| s.to_string()).collect(),
            remote: None,
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
