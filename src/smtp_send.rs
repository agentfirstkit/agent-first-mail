use crate::config::{MailConfig, SmtpConfig};
use crate::error::{AppError, Result};
use crate::frontmatter::{CaseFrontmatter, DraftFrontmatter};
use crate::mail::parse_outbound_message_with_status;
use crate::markdown::read_doc;
use crate::types::CaseMessages;
#[cfg(test)]
use crate::types::{MessageAuthentication, MessageFile};
use crate::util::{write_bytes_atomic, write_json_pretty};
use lettre::address::{Address, Envelope};
use lettre::message::{header, Attachment, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use sanitize_filename::{sanitize_with_options, Options as SanitizeFilenameOptions};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedOutbound {
    pub message_id: String,
    pub raw: Vec<u8>,
    pub envelope_from: String,
    pub envelope_to: Vec<String>,
}

pub fn prepare_outbound(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    draft_name: &str,
    config: &MailConfig,
    existing_message_id: Option<&str>,
) -> Result<PreparedOutbound> {
    let message_id = existing_message_id
        .map(ToString::to_string)
        .unwrap_or_else(|| unique_outbound_id(root));
    let message = build_draft_message(root, case_path, case_uid, draft_name, config, &message_id)?;
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
    persist_outbound(root, &message_id, &raw, case_uid, "push_queued", None)?;
    Ok(PreparedOutbound {
        message_id,
        raw,
        envelope_from,
        envelope_to,
    })
}

pub fn mark_staged(root: &Path, message_id: &str, raw: &[u8], case_uid: &str) -> Result<()> {
    persist_outbound(root, message_id, raw, case_uid, "staged_draft", None)
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
    let messages_path = case_path.join("data").join("messages.json");
    let mut case_messages = read_case_messages(&messages_path, case_uid)?;
    case_messages.merge_ids(&[message_id.to_string()]);
    write_json_pretty(&messages_path, &case_messages)?;
    update_case_metadata_after_append(case_path, case_messages.message_ids.len())?;
    crate::store::Workspace::at(root).render_refresh()?;
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
) -> Result<Message> {
    let draft_path = case_path.join("drafts").join(draft_name);
    let draft_text = fs::read_to_string(&draft_path).map_err(|e| AppError::io("read draft", &e))?;
    let (fm, raw_body) = read_doc::<DraftFrontmatter>(&draft_text)?;
    let body = raw_body.trim_start().to_string();
    let from = config.require_from()?;
    let rfc822_message_id = format!("<{message_id}@afmail.local>");
    build_message(
        root,
        case_path,
        case_uid,
        &fm,
        &body,
        &from,
        &rfc822_message_id,
    )
}

fn persist_outbound(
    root: &Path,
    message_id: &str,
    raw: &[u8],
    case_uid: &str,
    workspace_status: &str,
    sent_rfc3339: Option<String>,
) -> Result<()> {
    let parsed = parse_outbound_message_with_status(
        message_id.to_string(),
        raw,
        case_uid.to_string(),
        workspace_status.to_string(),
        sent_rfc3339,
    )?;
    let messages_dir = root.join(".afmail/messages");
    fs::create_dir_all(&messages_dir).map_err(|e| AppError::io("create messages dir", &e))?;
    write_bytes_atomic(
        &messages_dir.join(format!("{message_id}.eml")),
        raw,
        "write outbound eml",
    )?;
    crate::store::Workspace::at(root).write_message_artifacts(&parsed.message)
}

fn build_message(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    fm: &DraftFrontmatter,
    body: &str,
    from: &str,
    rfc822_message_id: &str,
) -> Result<Message> {
    if fm.case_uid != case_uid {
        return Err(AppError::new(
            "draft_invalid",
            "draft case_uid does not match case",
        ));
    }
    let mut builder = Message::builder()
        .from(parse_mailbox(from, "from")?)
        .message_id(Some(rfc822_message_id.to_string()))
        .subject(
            fm.subject
                .clone()
                .ok_or_else(|| AppError::new("draft_invalid", "draft subject is required"))?,
        );
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
    let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(body.to_string()));
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
                ))
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
        SmtpTransport::builder_dangerous(&config.host).port(config.port)
    };
    if let (Some(username), Some(password)) = (&config.username, &config.password_secret) {
        builder = builder.credentials(Credentials::new(username.clone(), password.clone()));
    }
    Ok(builder.build())
}

fn parse_mailbox(value: &str, field: &str) -> Result<Mailbox> {
    value
        .parse::<Mailbox>()
        .map_err(|e| AppError::new("draft_invalid", format!("invalid {field} address: {e}")))
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

fn read_case_messages(path: &Path, case_uid: &str) -> Result<CaseMessages> {
    if !path.exists() {
        return Ok(CaseMessages::new(case_uid));
    }
    let data = fs::read_to_string(path).map_err(|e| AppError::io("read case messages", &e))?;
    let mut messages: CaseMessages =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse case messages", &e))?;
    if messages.schema_name != "case_messages" || messages.schema_version != 1 {
        return Err(AppError::new(
            "case_messages_invalid",
            "invalid case messages schema",
        ));
    }
    messages.case_uid = case_uid.to_string();
    Ok(messages)
}

fn update_case_metadata_after_append(case_path: &Path, message_count: usize) -> Result<()> {
    let path = case_path.join("data").join("case.json");
    let data = fs::read_to_string(&path).map_err(|e| AppError::io("read case metadata", &e))?;
    let mut case: CaseFrontmatter =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse case metadata", &e))?;
    case.message_count = message_count;
    case.updated_rfc3339 = Some(crate::store::now_rfc3339());
    write_json_pretty(&path, &case)
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

    #[test]
    fn builds_plain_draft_message() {
        let root = temp_root("build");
        let case_path = root.join("cases/open/c20260521001");
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
            "Me <me@example.com>",
            "<msg@example.com>",
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
    fn staged_outbound_writes_sidecar_layout() {
        let root = temp_root("staged-layout");
        let _ = fs::create_dir_all(&root);
        let raw = concat!(
            "Message-ID: <message_out@afmail.local>\r\n",
            "From: Me <me@example.com>\r\n",
            "To: Alice <alice@example.com>\r\n",
            "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
            "Subject: Hi\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n\r\n",
            "Hello\r\n"
        );
        let result = mark_staged(&root, "message_out", raw.as_bytes(), "c20260521001");
        assert!(result.is_ok(), "{result:?}");
        assert!(root.join(".afmail/messages/message_out.eml").is_file());
        assert!(root
            .join(".afmail/messages/message_out.state.json")
            .is_file());
        assert!(!root
            .join(".afmail/messages/message_out.remote.json")
            .exists());
        assert!(!root.join(".afmail/messages/message_out.json").exists());
        assert!(root.join("messages/message_out.json").is_file());
        let _ = fs::remove_dir_all(root);
    }

    fn write_parent(root: &Path, message_id: &str, rfc822_id: &str, references: &[&str]) {
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
            workspace: crate::types::WorkspaceState {
                status: "triage".to_string(),
                archive_uid: None,
                archived_rfc3339: None,
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
        let case_path = root.join("cases/open/c20260521001");
        let _ = fs::create_dir_all(case_path.join("drafts"));
        // Parent already carries a References chain (bracket-less, as stored).
        write_parent(
            &root,
            "message_p",
            "parent@example.com",
            &["root@example.com"],
        );
        let fm = draft(
            "kind: draft\ncase_uid: c20260521001\nto:\n  - a@example.com\nsubject: \"Re: Hi\"\nreply_to_message_id: message_p",
        );
        let raw = build_message(
            &root,
            &case_path,
            "c20260521001",
            &fm,
            "reply body",
            "Me <me@example.com>",
            "<reply@afmail.local>",
        )
        .map(|m| String::from_utf8(m.formatted()).unwrap_or_default())
        .unwrap_or_default();
        assert!(raw.contains("In-Reply-To: <parent@example.com>"));
        assert!(raw.contains("References: <root@example.com> <parent@example.com>"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reply_without_parent_references_falls_back_to_parent_id() {
        let root = temp_root("reply-fallback");
        let case_path = root.join("cases/open/c20260521001");
        let _ = fs::create_dir_all(case_path.join("drafts"));
        write_parent(&root, "message_p", "parent@example.com", &[]);
        let fm = draft(
            "kind: draft\ncase_uid: c20260521001\nto:\n  - a@example.com\nsubject: \"Re: Hi\"\nreply_to_message_id: message_p",
        );
        let raw = build_message(
            &root,
            &case_path,
            "c20260521001",
            &fm,
            "reply body",
            "Me <me@example.com>",
            "<reply@afmail.local>",
        )
        .map(|m| String::from_utf8(m.formatted()).unwrap_or_default())
        .unwrap_or_default();
        assert!(raw.contains("In-Reply-To: <parent@example.com>"));
        assert!(raw.contains("References: <parent@example.com>"));
        let _ = fs::remove_dir_all(root);
    }
}
