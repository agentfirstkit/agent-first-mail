use crate::error::{AppError, Result};
use crate::store::{clean_body_text, render_message_section};
use crate::types::{
    AttachmentRef, AuthAlignment, AuthVerdict, ImapRef, MessageAuthentication, MessageFile,
    RemoteLocation, RemoteState, WorkspaceState,
};
use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};

#[derive(Clone, Debug)]
pub struct ParsedMail {
    pub message: MessageFile,
    pub body_text: String,
    pub conversation: String,
}

#[derive(Clone, Debug)]
pub struct MessageParseOptions {
    pub direction: Option<String>,
    pub workspace: WorkspaceState,
    pub remote: Option<RemoteState>,
    pub received_rfc3339: Option<String>,
    pub sent_rfc3339: Option<String>,
    pub attachments: Vec<AttachmentRef>,
}

pub fn parse_inbound_message(
    message_id: String,
    raw_eml: &[u8],
    imap: ImapRef,
) -> Result<ParsedMail> {
    let remote = Some(RemoteState {
        locations: vec![RemoteLocation {
            mailbox_id: None,
            mailbox_name: imap.mailbox_name.clone(),
            uid_validity: Some(imap.uid_validity),
            uid: Some(imap.uid),
            flags: Vec::new(),
            observed_rfc3339: crate::store::now_rfc3339(),
            missing_rfc3339: None,
        }],
    });
    parse_message_with_options(
        message_id,
        raw_eml,
        MessageParseOptions {
            direction: Some("inbound".to_string()),
            workspace: WorkspaceState {
                status: "triage".to_string(),
                archive_uid: None,
                archived_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
            remote,
            received_rfc3339: None,
            sent_rfc3339: None,
            attachments: Vec::new(),
        },
    )
}

pub fn parse_message_with_options(
    message_id: String,
    raw_eml: &[u8],
    options: MessageParseOptions,
) -> Result<ParsedMail> {
    let parsed = MessageParser::default()
        .parse(raw_eml)
        .ok_or_else(|| AppError::new("mime_parse_failed", "failed to parse RFC822 message"))?;
    let body = parsed
        .body_text(0)
        .or_else(|| parsed.body_html(0))
        .map(|s| s.into_owned())
        .unwrap_or_default();
    let body_text = clean_body_text(&body);
    let mut attachments = parsed
        .attachments
        .iter()
        .filter_map(|part_id| parsed.part(*part_id).map(|part| (*part_id, part)))
        .map(|(part_id, part)| AttachmentRef {
            part_id: part_id.to_string(),
            filename: part
                .attachment_name()
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("part-{part_id}")),
            content_type: content_type_string(part.content_type()),
            size_bytes: part.len() as u64,
            fetched: false,
            file_path: None,
            source_path: None,
        })
        .collect::<Vec<_>>();
    merge_attachment_state(&mut attachments, &options.attachments);
    let parsed_date_rfc3339 = parsed.date().map(|d| d.to_rfc3339());
    let rfc822_message_id = parsed.message_id().map(ToString::to_string);
    let in_reply_to = header_id_list(parsed.in_reply_to()).into_iter().next_back();
    let references = header_id_list(parsed.references());
    let direction = options.direction.unwrap_or_else(|| "inbound".to_string());
    let is_outbound = direction.eq_ignore_ascii_case("outbound")
        || (direction.eq_ignore_ascii_case("sent")
            && options.received_rfc3339.is_none()
            && options.sent_rfc3339.is_some());
    let (received_rfc3339, sent_rfc3339) = if is_outbound {
        (
            None,
            options.sent_rfc3339.or_else(|| parsed_date_rfc3339.clone()),
        )
    } else {
        (
            options
                .received_rfc3339
                .or_else(|| parsed_date_rfc3339.clone()),
            options.sent_rfc3339,
        )
    };
    let message = MessageFile {
        schema_name: "message".to_string(),
        schema_version: 1,
        message_id: message_id.clone(),
        rfc822_message_id,
        in_reply_to,
        references,
        remote: options.remote,
        direction: Some(direction),
        subject: parsed.subject().map(ToString::to_string),
        from: parsed.from().and_then(format_first_address),
        to: parsed.to().map(format_addresses).unwrap_or_default(),
        cc: parsed.cc().map(format_addresses).unwrap_or_default(),
        bcc: parsed.bcc().map(format_addresses).unwrap_or_default(),
        reply_to: parsed.reply_to().map(format_addresses).unwrap_or_default(),
        sender: parsed.sender().and_then(format_first_address),
        delivered_to: raw_header_values(&parsed, "Delivered-To"),
        x_original_to: raw_header_values(&parsed, "X-Original-To"),
        envelope_to: raw_header_values(&parsed, "Envelope-To"),
        list_id: raw_header_values(&parsed, "List-ID").into_iter().next(),
        mailing_list_headers: mailing_list_headers(&parsed),
        authentication: parse_authentication(
            raw_header_values(&parsed, "Authentication-Results"),
            parsed.from().and_then(first_address_domain),
        ),
        received_rfc3339,
        sent_rfc3339,
        body_text: body_text.clone(),
        eml_path: Some(format!(".afmail/messages/{message_id}.eml")),
        attachments,
        workspace: options.workspace,
    };
    let conversation = render_message_section(&message, &body_text)?;
    Ok(ParsedMail {
        message,
        body_text,
        conversation,
    })
}

pub fn parse_outbound_message(
    message_id: String,
    raw_eml: &[u8],
    case_uid: String,
) -> Result<ParsedMail> {
    parse_outbound_message_with_status(
        message_id,
        raw_eml,
        case_uid,
        "case".to_string(),
        Some(crate::store::now_rfc3339()),
    )
}

pub fn parse_outbound_message_with_status(
    message_id: String,
    raw_eml: &[u8],
    _case_uid: String,
    workspace_status: String,
    sent_rfc3339: Option<String>,
) -> Result<ParsedMail> {
    parse_message_with_options(
        message_id,
        raw_eml,
        MessageParseOptions {
            direction: Some("outbound".to_string()),
            workspace: WorkspaceState {
                status: workspace_status,
                archive_uid: None,
                archived_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
            remote: None,
            received_rfc3339: None,
            sent_rfc3339,
            attachments: Vec::new(),
        },
    )
}

fn merge_attachment_state(attachments: &mut [AttachmentRef], previous: &[AttachmentRef]) {
    for attachment in attachments {
        let Some(prior) = previous.iter().find(|prior| {
            prior.part_id == attachment.part_id
                || (prior.filename == attachment.filename
                    && prior.content_type == attachment.content_type)
        }) else {
            continue;
        };
        attachment.fetched = prior.fetched;
        attachment.file_path = prior.file_path.clone();
        attachment.source_path = prior.source_path.clone();
    }
}

pub fn attachment_bytes(raw_eml: &[u8], part_id: &str) -> Result<Vec<u8>> {
    let parsed = MessageParser::default()
        .parse(raw_eml)
        .ok_or_else(|| AppError::new("mime_parse_failed", "failed to parse RFC822 message"))?;
    let id = part_id.parse::<u32>().map_err(|_| {
        AppError::new(
            "attachment_not_found",
            format!("invalid part id: {part_id}"),
        )
    })?;
    let part = parsed.part(id).ok_or_else(|| {
        AppError::new(
            "attachment_not_found",
            format!("attachment not found: part {part_id}"),
        )
    })?;
    Ok(part.contents().to_vec())
}

fn content_type_string(content_type: Option<&mail_parser::ContentType<'_>>) -> String {
    match content_type {
        Some(ct) => match ct.subtype() {
            Some(subtype) => format!("{}/{}", ct.ctype(), subtype),
            None => ct.ctype().to_string(),
        },
        None => "application/octet-stream".to_string(),
    }
}

/// Collect RFC822 message-ids from an `In-Reply-To` / `References` header,
/// which `mail-parser` exposes as a single `Text` or a `TextList` with the
/// angle brackets already stripped. Stored bracket-less to match how
/// `rfc822_message_id` is recorded; brackets are re-added when a header is
/// emitted.
fn header_id_list(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(s) => vec![s.trim().to_string()],
        HeaderValue::TextList(list) => list.iter().map(|s| s.trim().to_string()).collect(),
        _ => Vec::new(),
    }
}

fn format_first_address(address: &Address<'_>) -> Option<String> {
    address.first().and_then(|addr| {
        let email = addr.address()?;
        Some(match addr.name() {
            Some(name) if !name.is_empty() => format!("{name} <{email}>"),
            _ => email.to_string(),
        })
    })
}

fn format_addresses(address: &Address<'_>) -> Vec<String> {
    address
        .iter()
        .filter_map(|addr| {
            let email = addr.address()?;
            Some(match addr.name() {
                Some(name) if !name.is_empty() => format!("{name} <{email}>"),
                _ => email.to_string(),
            })
        })
        .collect()
}

fn raw_header_values(message: &mail_parser::Message<'_>, name: &str) -> Vec<String> {
    message
        .headers_raw()
        .filter(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .collect()
}

fn first_address_domain(address: &Address<'_>) -> Option<String> {
    address
        .first()
        .and_then(|addr| addr.address())
        .and_then(address_domain)
}

/// Registrable domain (the user-visible address domain) for an authentication
/// property value such as `user@domain`, `@domain`, or a bare `domain`.
fn address_domain(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(|c| c == '<' || c == '>').trim();
    let candidate = value.rsplit('@').next().unwrap_or(value);
    let candidate = candidate.trim().trim_end_matches('.').to_ascii_lowercase();
    if candidate.is_empty() || !candidate.contains('.') {
        return None;
    }
    Some(candidate)
}

/// Last two labels of a domain, used as a coarse organizational-domain match for
/// DMARC alignment. This intentionally over-matches multi-label public suffixes
/// (e.g. `co.uk`); afmail does not bundle a Public Suffix List.
fn registrable_suffix(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').filter(|s| !s.is_empty()).collect();
    let n = labels.len();
    if n >= 2 {
        format!("{}.{}", labels[n - 2], labels[n - 1])
    } else {
        domain.to_string()
    }
}

#[derive(Default)]
struct DomainCandidates {
    dmarc: Option<String>,
    dkim: Option<String>,
    spf: Option<String>,
}

/// Parse `Authentication-Results` header(s) into structured verdicts, the
/// authenticated domain, and DMARC/`From` alignment.
fn parse_authentication(raw: Vec<String>, from_domain: Option<String>) -> MessageAuthentication {
    let mut auth = MessageAuthentication {
        from_domain: from_domain.clone(),
        ..MessageAuthentication::default()
    };
    let mut domains = DomainCandidates::default();
    for header in &raw {
        for segment in split_top_level_semicolons(header) {
            apply_authentication_segment(&mut auth, &mut domains, &segment);
        }
    }
    auth.authenticated_domain = domains.dmarc.or(domains.dkim).or(domains.spf);
    auth.alignment = match (&auth.authenticated_domain, &from_domain) {
        (Some(authenticated), Some(from)) => {
            if registrable_suffix(authenticated) == registrable_suffix(from) {
                AuthAlignment::Aligned
            } else {
                AuthAlignment::Mismatch
            }
        }
        _ => AuthAlignment::Unknown,
    };
    auth.raw = raw;
    auth
}

/// Split on top-level `;`, leaving parenthesised comments (which may contain
/// `;` or `=`) intact within each segment.
fn split_top_level_semicolons(header: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in header.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ';' if depth == 0 => out.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Remove parenthesised comments from a segment, returning the bare segment and
/// the concatenated comment text (where DMARC carries its `p=` policy).
fn strip_comments(segment: &str) -> (String, String) {
    let mut bare = String::new();
    let mut comment = String::new();
    let mut depth = 0usize;
    for ch in segment.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth > 0 => comment.push(ch),
            _ => bare.push(ch),
        }
    }
    (bare, comment)
}

fn apply_authentication_segment(
    auth: &mut MessageAuthentication,
    domains: &mut DomainCandidates,
    segment: &str,
) {
    let (bare, comment) = strip_comments(segment);
    let mut tokens = bare.split_whitespace();
    let Some(first) = tokens.next() else {
        return;
    };
    // The first segment is the authserv-id, and any token without `method=result`
    // is ignored — only spf/dkim/dmarc results drive the structured fields.
    let Some((method_raw, result)) = split_kv(first) else {
        return;
    };
    let method = method_raw.to_ascii_lowercase();
    let verdict = parse_verdict(result);
    match method.as_str() {
        "spf" => set_verdict(&mut auth.spf, verdict),
        "dkim" => set_verdict(&mut auth.dkim, verdict),
        "dmarc" => {
            set_verdict(&mut auth.dmarc, verdict);
            if let Some(policy) = extract_policy(&comment) {
                auth.dmarc_policy.get_or_insert(policy);
            }
        }
        _ => return,
    }
    // Only a passing mechanism contributes an authenticated domain.
    if verdict != AuthVerdict::Pass {
        return;
    }
    for token in tokens {
        let Some((key, value)) = split_kv(token) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let domain = match method.as_str() {
            "spf" if matches!(key.as_str(), "smtp.mailfrom" | "envelope-from") => {
                address_domain(value)
            }
            "dkim" if matches!(key.as_str(), "header.i" | "header.d") => address_domain(value),
            "dmarc" if key == "header.from" => address_domain(value),
            _ => None,
        };
        if let Some(domain) = domain {
            match method.as_str() {
                "spf" => domains.spf.get_or_insert(domain),
                "dkim" => domains.dkim.get_or_insert(domain),
                "dmarc" => domains.dmarc.get_or_insert(domain),
                _ => continue,
            };
        }
    }
}

fn split_kv(token: &str) -> Option<(&str, &str)> {
    let (key, value) = token.split_once('=')?;
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key, value))
}

fn parse_verdict(value: &str) -> AuthVerdict {
    match value.trim().to_ascii_lowercase().as_str() {
        "pass" => AuthVerdict::Pass,
        "fail" | "hardfail" => AuthVerdict::Fail,
        "softfail" => AuthVerdict::SoftFail,
        "neutral" => AuthVerdict::Neutral,
        "none" => AuthVerdict::None,
        "temperror" => AuthVerdict::TempError,
        "permerror" => AuthVerdict::PermError,
        _ => AuthVerdict::Neutral,
    }
}

/// Combine the same mechanism seen across multiple headers/signatures: a `Pass`
/// always wins, otherwise the first concrete verdict is kept.
fn set_verdict(slot: &mut AuthVerdict, new: AuthVerdict) {
    if *slot == AuthVerdict::Missing || new == AuthVerdict::Pass {
        *slot = new;
    }
}

fn extract_policy(comment: &str) -> Option<String> {
    for token in comment.split(|c: char| c.is_whitespace() || c == ';') {
        let lower = token.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("p=") {
            let policy = rest.trim().to_string();
            if !policy.is_empty() {
                return Some(policy);
            }
        }
    }
    None
}

fn mailing_list_headers(message: &mail_parser::Message<'_>) -> Vec<String> {
    let names = [
        "List-Unsubscribe",
        "List-Post",
        "List-Help",
        "Mailing-List",
        "X-Mailing-List",
        "Precedence",
    ];
    let mut out = Vec::new();
    for name in names {
        for value in raw_header_values(message, name) {
            let clear_list_header = !name.eq_ignore_ascii_case("Precedence")
                || matches!(value.to_ascii_lowercase().as_str(), "bulk" | "list");
            if clear_list_header {
                out.push(format!("{name}: {value}"));
            }
        }
    }
    out
}

pub fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "message".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_message_and_attachment_metadata() {
        let raw = concat!(
            "Message-ID: <m1@example.com>\r\n",
            "From: Alice <alice@example.com>\r\n",
            "To: Me <me@example.com>\r\n",
            "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
            "Subject: Contract renewal\r\n",
            "Content-Type: multipart/mixed; boundary=abc\r\n\r\n",
            "--abc\r\nContent-Type: text/plain\r\n\r\nHello\r\n",
            "--abc\r\nContent-Type: text/plain; name=note.txt\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\nAttached\r\n",
            "--abc--\r\n"
        );
        let parsed = parse_inbound_message(
            "message_inbox_1_1".to_string(),
            raw.as_bytes(),
            ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 1,
            },
        );
        assert!(parsed.is_ok());
        let parsed = parsed.ok();
        assert_eq!(parsed.as_ref().map(|p| p.body_text.as_str()), Some("Hello"));
        assert_eq!(
            parsed.as_ref().map(|p| p.message.attachments.len()),
            Some(1)
        );
        assert!(parsed
            .as_ref()
            .map(|p| p.conversation.contains("```text"))
            .unwrap_or(false));
    }

    #[test]
    fn parses_passing_authentication_with_alignment() {
        let raw = vec![concat!(
            "purelymail.com; spf=pass (domain of email.apple.com designates 17.111.110.110 ",
            "as permitted sender) smtp.mailfrom=email.apple.com; dkim=pass ",
            "header.i=email.apple.com; dmarc=pass (p=reject) header.from=no_reply@email.apple.com"
        )
        .to_string()];
        let auth = parse_authentication(raw, Some("apple.com".to_string()));
        assert_eq!(auth.spf, AuthVerdict::Pass);
        assert_eq!(auth.dkim, AuthVerdict::Pass);
        assert_eq!(auth.dmarc, AuthVerdict::Pass);
        assert_eq!(auth.dmarc_policy.as_deref(), Some("reject"));
        assert_eq!(
            auth.authenticated_domain.as_deref(),
            Some("email.apple.com")
        );
        assert_eq!(auth.alignment, AuthAlignment::Aligned);
        assert!(auth.has_results());
        assert!(!auth.is_warning());
    }

    #[test]
    fn flags_spf_failure_as_warning() {
        let auth = parse_authentication(
            vec!["mx.example.com; spf=fail smtp.mailfrom=bad.test".to_string()],
            Some("example.com".to_string()),
        );
        assert_eq!(auth.spf, AuthVerdict::Fail);
        assert!(auth.is_warning());
    }

    #[test]
    fn missing_header_is_missing_not_warning() {
        let auth = parse_authentication(Vec::new(), Some("example.com".to_string()));
        assert_eq!(auth.spf, AuthVerdict::Missing);
        assert_eq!(auth.dkim, AuthVerdict::Missing);
        assert_eq!(auth.dmarc, AuthVerdict::Missing);
        assert!(!auth.has_results());
        assert!(!auth.is_warning());
    }

    #[test]
    fn soft_results_are_shown_but_not_warnings() {
        let auth = parse_authentication(
            vec![
                "mx.example.com; spf=softfail smtp.mailfrom=x.test; dmarc=none header.from=x.test"
                    .to_string(),
            ],
            Some("x.test".to_string()),
        );
        assert_eq!(auth.spf, AuthVerdict::SoftFail);
        assert_eq!(auth.dmarc, AuthVerdict::None);
        assert!(auth.has_results());
        assert!(!auth.is_warning());
    }

    #[test]
    fn passing_dmarc_on_lookalike_domain_is_mismatch_and_warning() {
        let auth = parse_authentication(
            vec!["mx; dmarc=pass (p=reject) header.from=billing@apple-billing.net".to_string()],
            Some("apple.com".to_string()),
        );
        assert_eq!(auth.dmarc, AuthVerdict::Pass);
        assert_eq!(
            auth.authenticated_domain.as_deref(),
            Some("apple-billing.net")
        );
        assert_eq!(auth.alignment, AuthAlignment::Mismatch);
        assert!(auth.is_warning());
    }

    #[test]
    fn comment_semicolons_do_not_split_segments() {
        let auth = parse_authentication(
            vec![
                "mx; spf=pass (uses ; and = inside) smtp.mailfrom=ok.test; dkim=pass header.d=ok.test"
                    .to_string(),
            ],
            Some("ok.test".to_string()),
        );
        assert_eq!(auth.spf, AuthVerdict::Pass);
        assert_eq!(auth.dkim, AuthVerdict::Pass);
        assert_eq!(auth.authenticated_domain.as_deref(), Some("ok.test"));
    }

    #[test]
    fn decodes_gb2312_encoded_subject() {
        let raw = concat!(
            "Message-ID: <gb2312@example.com>\r\n",
            "From: Apple Developer <developer@insideapple.apple.com>\r\n",
            "To: Me <me@example.com>\r\n",
            "Subject: =?gb2312?B?0rvW3LW5vMbKsQ==?=\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n\r\n",
            "Body\r\n"
        );
        let parsed = parse_inbound_message(
            "message_inbox_1_gb2312".to_string(),
            raw.as_bytes(),
            ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 1,
            },
        );
        assert!(parsed.is_ok());
        assert_eq!(
            parsed.ok().and_then(|p| p.message.subject),
            Some("一周倒计时".to_string())
        );
    }

    #[test]
    fn extracts_reply_threading_headers() {
        let raw = concat!(
            "Message-ID: <child@example.com>\r\n",
            "In-Reply-To: <parent@example.com>\r\n",
            "References: <root@example.com> <parent@example.com>\r\n",
            "From: Alice <alice@example.com>\r\n",
            "To: Me <me@example.com>\r\n",
            "Subject: Re: Hi\r\n\r\nBody\r\n"
        );
        let parsed = parse_inbound_message(
            "message_inbox_1_2".to_string(),
            raw.as_bytes(),
            ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 2,
            },
        );
        assert!(parsed.is_ok());
        if let Ok(parsed) = parsed {
            // Stored bracket-less, matching rfc822_message_id storage.
            assert_eq!(
                parsed.message.in_reply_to.as_deref(),
                Some("parent@example.com")
            );
            assert_eq!(
                parsed.message.references,
                vec![
                    "root@example.com".to_string(),
                    "parent@example.com".to_string()
                ]
            );
        }
    }

    #[test]
    fn extracts_attachment_bytes_from_raw_eml() {
        let raw = concat!(
            "Message-ID: <m2@example.com>\r\n",
            "From: Alice <alice@example.com>\r\n",
            "To: Me <me@example.com>\r\n",
            "Subject: Attachment\r\n",
            "Content-Type: multipart/mixed; boundary=abc\r\n\r\n",
            "--abc\r\nContent-Type: text/plain\r\n\r\nBody\r\n",
            "--abc\r\nContent-Type: text/plain; name=note.txt\r\nContent-Disposition: attachment; filename=note.txt\r\n\r\nAttached\r\n",
            "--abc--\r\n"
        );
        let parsed = parse_inbound_message(
            "message_inbox_1_2".to_string(),
            raw.as_bytes(),
            ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 2,
            },
        );
        assert!(parsed.is_ok());
        let part_id = parsed
            .ok()
            .and_then(|mail| mail.message.attachments.first().map(|a| a.part_id.clone()))
            .unwrap_or_default();
        let bytes = attachment_bytes(raw.as_bytes(), &part_id);
        assert_eq!(bytes, Ok(b"Attached".to_vec()));
    }

    #[test]
    fn html_only_body_contains_no_html_tags() {
        let raw = concat!(
            "Message-ID: <html-only@example.com>\r\n",
            "From: Sender <sender@example.com>\r\n",
            "To: Me <me@example.com>\r\n",
            "Date: Thu, 21 May 2026 10:00:00 +0000\r\n",
            "Subject: HTML only\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "\r\n",
            "<html><body><p>Hello <b>world</b>!</p></body></html>\r\n"
        );
        let parsed = parse_inbound_message(
            "message_inbox_1_3".to_string(),
            raw.as_bytes(),
            ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 3,
            },
        );
        assert!(parsed.is_ok());
        let body_text = parsed.map(|p| p.body_text).unwrap_or_default();
        assert!(
            !body_text.contains('<'),
            "html-only body should not contain raw HTML tags, got: {body_text:?}"
        );
        assert!(
            body_text.contains("Hello") || body_text.contains("world"),
            "body should contain text content, got: {body_text:?}"
        );
    }
}
