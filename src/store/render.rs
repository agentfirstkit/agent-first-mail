use super::*;

pub(super) fn message_time(message: &MessageFile) -> Option<String> {
    message_time_raw(message).map(ToString::to_string)
}

pub(super) fn message_time_raw(message: &MessageFile) -> Option<&str> {
    message
        .received_rfc3339
        .as_deref()
        .or(message.sent_rfc3339.as_deref())
}

pub(super) fn message_time_utc(message: &MessageFile) -> Option<DateTime<Utc>> {
    message_time_raw(message)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub(super) fn compare_message_time_asc(a: &MessageFile, b: &MessageFile) -> std::cmp::Ordering {
    match (message_time_utc(a), message_time_utc(b)) {
        (Some(a_time), Some(b_time)) => a_time.cmp(&b_time),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => message_time(a)
            .unwrap_or_default()
            .cmp(&message_time(b).unwrap_or_default()),
    }
    .then_with(|| a.message_id.cmp(&b.message_id))
}

pub(super) fn compare_rfc3339_asc(a: &str, b: &str) -> std::cmp::Ordering {
    let a_time = DateTime::parse_from_rfc3339(a)
        .ok()
        .map(|value| value.with_timezone(&Utc));
    let b_time = DateTime::parse_from_rfc3339(b)
        .ok()
        .map(|value| value.with_timezone(&Utc));
    match (a_time, b_time) {
        (Some(a_time), Some(b_time)) => a_time.cmp(&b_time),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

pub(super) fn message_time_datetime(message: &MessageFile, offset: &FixedOffset) -> Option<String> {
    message_time_raw(message)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| {
            value
                .with_timezone(offset)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
}

pub(super) fn message_time_context(message: &MessageFile, offset: &FixedOffset) -> Value {
    time_context(message_time_raw(message).unwrap_or_default(), offset)
}

pub(super) fn time_context(original: &str, offset: &FixedOffset) -> Value {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(original) {
        let local = parsed.with_timezone(offset);
        json!({
            "original_rfc3339": original,
            "local_rfc3339": local.to_rfc3339_opts(SecondsFormat::Secs, true),
            "date": local.format("%Y-%m-%d").to_string(),
            "time": local.format("%H:%M").to_string(),
            "datetime": local.format("%Y-%m-%d %H:%M").to_string(),
            "year": local.year(),
            "month": local.month(),
            "day": local.day(),
            "hour": local.hour(),
            "minute": local.minute(),
        })
    } else {
        json!({
            "original_rfc3339": original,
            "local_rfc3339": "",
            "date": "",
            "time": "",
            "datetime": "",
            "year": null,
            "month": null,
            "day": null,
            "hour": null,
            "minute": null,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThreadDirection {
    Received,
    Sent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThreadAction {
    Message,
    Reply,
    Forward,
}

pub(super) fn message_thread_direction(message: &MessageFile) -> ThreadDirection {
    match message.direction.as_deref().map(str::trim) {
        Some(direction)
            if MailDirection::parse(direction)
                .ok()
                .is_some_and(|direction| direction == MailDirection::Outbound) =>
        {
            ThreadDirection::Sent
        }
        _ if message.received_rfc3339.is_none() && message.sent_rfc3339.is_some() => {
            ThreadDirection::Sent
        }
        _ => ThreadDirection::Received,
    }
}

pub(super) fn message_thread_action(message: &MessageFile) -> ThreadAction {
    let subject = message.subject.as_deref().unwrap_or_default();
    if subject_has_prefix(subject, &["fwd:", "fw:", "转发:", "轉發:", "fwd：", "fw："]) {
        return ThreadAction::Forward;
    }
    if message
        .in_reply_to
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || !message.references.is_empty()
        || subject_has_prefix(
            subject,
            &["re:", "回复:", "回覆:", "答复:", "答覆:", "re："],
        )
    {
        return ThreadAction::Reply;
    }
    ThreadAction::Message
}

pub(super) fn subject_has_prefix(subject: &str, prefixes: &[&str]) -> bool {
    let lower = subject.trim_start().to_ascii_lowercase();
    prefixes
        .iter()
        .any(|prefix| lower.starts_with(&prefix.to_ascii_lowercase()))
}

pub(super) fn thread_label(
    direction: ThreadDirection,
    action: ThreadAction,
    language: TemplateLanguage,
) -> &'static str {
    match language {
        TemplateLanguage::EnUs => match (direction, action) {
            (ThreadDirection::Received, ThreadAction::Message) => "\u{2190} Received",
            (ThreadDirection::Received, ThreadAction::Reply) => "\u{2190} Received reply",
            (ThreadDirection::Received, ThreadAction::Forward) => "\u{2190} Received forward",
            (ThreadDirection::Sent, ThreadAction::Message) => "\u{2192} Sent",
            (ThreadDirection::Sent, ThreadAction::Reply) => "\u{2192} Sent reply",
            (ThreadDirection::Sent, ThreadAction::Forward) => "\u{2192} Sent forward",
        },
        TemplateLanguage::ZhCn => match (direction, action) {
            (ThreadDirection::Received, ThreadAction::Message) => "\u{2190} 收到",
            (ThreadDirection::Received, ThreadAction::Reply) => "\u{2190} 收到回复",
            (ThreadDirection::Received, ThreadAction::Forward) => "\u{2190} 收到转发",
            (ThreadDirection::Sent, ThreadAction::Message) => "\u{2192} 发送",
            (ThreadDirection::Sent, ThreadAction::Reply) => "\u{2192} 发送回复",
            (ThreadDirection::Sent, ThreadAction::Forward) => "\u{2192} 发送转发",
        },
    }
}

pub(super) fn thread_action_kind(direction: ThreadDirection, action: ThreadAction) -> &'static str {
    match (direction, action) {
        (ThreadDirection::Received, ThreadAction::Message) => "received",
        (ThreadDirection::Received, ThreadAction::Reply) => "received_reply",
        (ThreadDirection::Received, ThreadAction::Forward) => "received_forward",
        (ThreadDirection::Sent, ThreadAction::Message) => "sent",
        (ThreadDirection::Sent, ThreadAction::Reply) => "sent_reply",
        (ThreadDirection::Sent, ThreadAction::Forward) => "sent_forward",
    }
}

pub(super) fn thread_contact(
    message: &MessageFile,
    direction: ThreadDirection,
    language: TemplateLanguage,
) -> (&'static str, &'static str, String) {
    match (direction, language) {
        (ThreadDirection::Received, TemplateLanguage::EnUs) => {
            ("from", "From", message.from.clone().unwrap_or_default())
        }
        (ThreadDirection::Received, TemplateLanguage::ZhCn) => {
            ("from", "发件人", message.from.clone().unwrap_or_default())
        }
        (ThreadDirection::Sent, TemplateLanguage::EnUs) => ("to", "To", message.to.join(", ")),
        (ThreadDirection::Sent, TemplateLanguage::ZhCn) => ("to", "收件人", message.to.join(", ")),
    }
}

pub(super) fn thread_item_common(
    message: &MessageFile,
    offset: &FixedOffset,
    language: TemplateLanguage,
    link: String,
    title: String,
) -> Result<Value> {
    let direction = message_thread_direction(message);
    let action = message_thread_action(message);
    let (contact_kind, contact_label, contact) = thread_contact(message, direction, language);
    let time = message_time_context(message, offset);
    let display_time = time
        .get("datetime")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let direction_kind = match direction {
        ThreadDirection::Received => "received",
        ThreadDirection::Sent => "sent",
    };
    let action_kind = match action {
        ThreadAction::Message => "message",
        ThreadAction::Reply => "reply",
        ThreadAction::Forward => "forward",
    };
    Ok(json!({
        "message_id": message.message_id.as_str(),
        "time": time,
        "time_rfc3339": message_time(message).unwrap_or_default(),
        "display_time": display_time,
        "direction": match direction {
            ThreadDirection::Received => "inbound",
            ThreadDirection::Sent => "outbound",
        },
        "direction_kind": direction_kind,
        "direction_symbol": match direction {
            ThreadDirection::Received => "\u{2190}",
            ThreadDirection::Sent => "\u{2192}",
        },
        "action": action_kind,
        "action_kind": thread_action_kind(direction, action),
        "action_label": thread_label(direction, action, language),
        "is_reply": action == ThreadAction::Reply,
        "is_forward": action == ThreadAction::Forward,
        "contact_kind": contact_kind,
        "contact_label": contact_label,
        "contact": contact.as_str(),
        "display_contact": markdown_inline(&contact),
        "from": message.from.as_deref().unwrap_or(""),
        "to": message.to.join(", "),
        "subject": message.subject.as_deref().unwrap_or(""),
        "display_subject": message
            .subject
            .as_deref()
            .map(markdown_inline)
            .unwrap_or_default(),
        "title": title.as_str(),
        "display_title": markdown_inline(&title),
        "status": message.workspace.status.as_str(),
        "display_status": markdown_inline(&message.workspace.status),
        "link": link,
        "message": message_template_value(message)?,
    }))
}

pub fn clean_body_text(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect()
}

pub fn render_message_section(message: &MessageFile, body_text: &str) -> Result<String> {
    render_message_section_with_options(message, body_text, TemplateLanguage::default(), None)
}

pub fn render_message_section_with_config(
    root: &Path,
    message: &MessageFile,
    body_text: &str,
    config: &MailConfig,
) -> Result<String> {
    render_message_section_with_root(
        Some(root),
        message,
        body_text,
        config.template_language(),
        config
            .smtp
            .from
            .as_deref()
            .or(config.imap.username.as_deref()),
        None,
    )
}

pub fn render_message_section_with_options(
    message: &MessageFile,
    body_text: &str,
    language: TemplateLanguage,
    account_email: Option<&str>,
) -> Result<String> {
    render_message_section_with_root(None, message, body_text, language, account_email, None)
}

pub(super) fn render_message_section_with_root(
    root: Option<&Path>,
    message: &MessageFile,
    body_text: &str,
    language: TemplateLanguage,
    account_email: Option<&str>,
    output_dir: Option<&Path>,
) -> Result<String> {
    let mut renderer = root.map_or_else(
        || MarkdownTemplateRenderer::builtin(language),
        |root| MarkdownTemplateRenderer::new(root, language),
    );
    renderer.render(
        TemplateKey::MessageSection,
        &message_section_context(
            root,
            message,
            body_text,
            language,
            account_email,
            output_dir,
        )?,
    )
}

pub(super) fn message_section_context(
    root: Option<&Path>,
    message: &MessageFile,
    body_text: &str,
    language: TemplateLanguage,
    account_email: Option<&str>,
    output_dir: Option<&Path>,
) -> Result<Value> {
    let timestamp_rfc3339 = message
        .received_rfc3339
        .as_deref()
        .or(message.sent_rfc3339.as_deref())
        .unwrap_or("");
    let direction = message_thread_direction(message);
    let display_time = message
        .received_rfc3339
        .as_deref()
        .or(message.sent_rfc3339.as_deref())
        .unwrap_or("unknown-time");
    let counterparty = match direction {
        ThreadDirection::Received => message.from.clone().unwrap_or_default(),
        ThreadDirection::Sent => message.to.join(", "),
    };
    let display_counterparty = markdown_inline(if counterparty.trim().is_empty() {
        match language {
            TemplateLanguage::EnUs => "unknown",
            TemplateLanguage::ZhCn => "未知",
        }
    } else {
        &counterparty
    });
    let (message_action, display_heading) = match (direction, language) {
        (ThreadDirection::Received, TemplateLanguage::EnUs) => (
            "received",
            format!("Received from {display_counterparty} - {display_time}"),
        ),
        (ThreadDirection::Sent, TemplateLanguage::EnUs) => (
            "sent",
            format!("Sent to {display_counterparty} - {display_time}"),
        ),
        (ThreadDirection::Received, TemplateLanguage::ZhCn) => (
            "received",
            format!("收到自 {display_counterparty} - {display_time}"),
        ),
        (ThreadDirection::Sent, TemplateLanguage::ZhCn) => (
            "sent",
            format!("发送给 {display_counterparty} - {display_time}"),
        ),
    };
    let from = message.from.as_deref().unwrap_or("");
    let mut hints = Vec::new();
    let mut possible_bcc = false;
    if let Some(account) = account_email
        .map(email_address)
        .filter(|value| !value.is_empty())
    {
        let visible_recipients = message
            .to
            .iter()
            .chain(message.cc.iter())
            .map(|value| email_address(value))
            .collect::<BTreeSet<_>>();
        let routed_to_me = message
            .delivered_to
            .iter()
            .chain(message.x_original_to.iter())
            .chain(message.envelope_to.iter())
            .any(|value| email_address(value) == account);
        if routed_to_me && !visible_recipients.contains(&account) {
            possible_bcc = true;
            hints.push(json!({"kind": "possible_bcc"}));
        }
    }
    let reply_to_differs =
        !message.reply_to.is_empty() && reply_to_differs_from_from(&message.reply_to, from);
    let reply_to_recipients = message.reply_to.join(", ");
    if reply_to_differs {
        hints.push(json!({
            "kind": "reply_to_differs",
            "recipients": reply_to_recipients.as_str(),
        }));
    }
    let mut sender_differs = false;
    let sender = message.sender.as_deref().unwrap_or("");
    if let Some(sender) = &message.sender {
        if email_address(sender) != email_address(from) {
            sender_differs = true;
            hints.push(json!({"kind": "sender_differs", "sender": sender}));
        }
    }
    let mailing_list = message.list_id.as_deref().unwrap_or("");
    let mailing_list_headers = message.mailing_list_headers.join(" | ");
    if let Some(list_id) = &message.list_id {
        hints.push(json!({"kind": "mailing_list", "list_id": list_id}));
    } else if !message.mailing_list_headers.is_empty() {
        hints.push(json!({
            "kind": "mailing_list_headers",
            "headers": mailing_list_headers.as_str(),
        }));
    }
    let auth = &message.authentication;
    let authentication_check = matches!(direction, ThreadDirection::Received) || auth.has_results();
    let security = json!({
        "authentication": {
            "check": authentication_check,
            "has_results": auth.has_results(),
            "spf": auth.spf.as_str(),
            "dkim": auth.dkim.as_str(),
            "dmarc": auth.dmarc.as_str(),
            "dmarc_policy": auth.dmarc_policy.clone(),
            "authenticated_domain": auth.authenticated_domain.clone(),
            "from_domain": auth.from_domain.clone(),
            "alignment": auth.alignment.as_str(),
        },
        "possible_bcc": possible_bcc,
        "reply_to_differs": reply_to_differs,
        "reply_to_recipients": reply_to_recipients,
        "sender_differs": sender_differs,
        "sender": sender,
        "mailing_list": mailing_list,
        "mailing_list_headers": mailing_list_headers,
    });
    let mut body_text_block = body_text.to_string();
    if !body_text_block.ends_with('\n') {
        body_text_block.push('\n');
    }
    let visible_body = body_text_visible(body_text);
    let mut body_text_visible_block = visible_body.visible.clone();
    if !body_text_visible_block.ends_with('\n') {
        body_text_visible_block.push('\n');
    }
    let body_text_fence = markdown_fence_for(&body_text_visible_block);
    let quoted_message_id = if visible_body.has_quoted_reply {
        quoted_local_message_id(root, message)?.unwrap_or_default()
    } else {
        String::new()
    };
    let attachments = message
        .attachments
        .iter()
        .map(|attachment| {
            let file_path = attachment.file_path.as_deref().unwrap_or("");
            let preview_path = if attachment.fetched
                && !file_path.is_empty()
                && is_image_content_type(&attachment.content_type)
            {
                attachment_markdown_path(root, output_dir, file_path)
            } else {
                String::new()
            };
            json!({
                "part_id": attachment.part_id.as_str(),
                "filename": attachment.filename.as_str(),
                "display_filename": markdown_inline(&attachment.filename),
                "image_alt": markdown_image_alt(&attachment.filename),
                "content_type": attachment.content_type.as_str(),
                "size_bytes": attachment.size_bytes,
                "file_path": file_path,
                "saved_filename": saved_filename_for_attachment(attachment),
                "source_path": attachment.source_path.as_deref().unwrap_or(""),
                "fetched": attachment.fetched,
                "is_image": is_image_content_type(&attachment.content_type),
                "preview_path": preview_path,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "language": language.as_str(),
        "message_id": message.message_id.as_str(),
        "timestamp_rfc3339": timestamp_rfc3339,
        "display_heading": display_heading,
        "message_action": message_action,
        "display_counterparty": display_counterparty,
        "from": from,
        "subject": message.subject.as_deref().unwrap_or(""),
        "to": &message.to,
        "cc": &message.cc,
        "bcc": &message.bcc,
        "body_text": body_text,
        "body_text_block": body_text_block,
        "body_text_visible": visible_body.visible,
        "body_text_visible_block": body_text_visible_block,
        "body_text_fence": body_text_fence,
        "has_quoted_reply": visible_body.has_quoted_reply,
        "quoted_message_id": quoted_message_id,
        "quoted_from": visible_body.quoted_from.unwrap_or_default(),
        "quoted_at": visible_body.quoted_at.unwrap_or_default(),
        "security": security,
        "hints": hints,
        "attachments": attachments,
        "message": message_template_value(message)?,
    }))
}

#[derive(Clone, Debug, Default)]
struct VisibleBodyText {
    visible: String,
    has_quoted_reply: bool,
    quoted_from: Option<String>,
    quoted_at: Option<String>,
}

fn body_text_visible(body_text: &str) -> VisibleBodyText {
    let lines = body_text.lines().collect::<Vec<_>>();
    for (idx, line) in lines.iter().enumerate() {
        if let Some((quoted_at, quoted_from)) = parse_apple_wrote_line(line) {
            return VisibleBodyText {
                visible: lines[..idx].join("\n").trim_end().to_string(),
                has_quoted_reply: true,
                quoted_from,
                quoted_at,
            };
        }
    }
    if let Some(idx) = trailing_quote_block_start(&lines) {
        return VisibleBodyText {
            visible: lines[..idx].join("\n").trim_end().to_string(),
            has_quoted_reply: true,
            quoted_from: None,
            quoted_at: None,
        };
    }
    VisibleBodyText {
        visible: body_text.trim_end().to_string(),
        has_quoted_reply: false,
        quoted_from: None,
        quoted_at: None,
    }
}

fn parse_apple_wrote_line(line: &str) -> Option<(Option<String>, Option<String>)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("On ") || !trimmed.ends_with(" wrote:") {
        return None;
    }
    let inner = trimmed.strip_prefix("On ")?.strip_suffix(" wrote:")?.trim();
    let (quoted_at, quoted_from) = inner
        .rsplit_once(',')
        .map(|(at, from)| {
            (
                Some(at.trim().to_string()).filter(|value| !value.is_empty()),
                Some(from.trim().to_string()).filter(|value| !value.is_empty()),
            )
        })
        .unwrap_or_else(|| {
            (
                Some(inner.to_string()).filter(|value| !value.is_empty()),
                None,
            )
        });
    Some((quoted_at, quoted_from))
}

fn trailing_quote_block_start(lines: &[&str]) -> Option<usize> {
    for idx in 0..lines.len() {
        let rest = &lines[idx..];
        let mut nonblank = rest.iter().filter(|line| !line.trim().is_empty());
        if nonblank
            .clone()
            .next()
            .is_some_and(|line| line.trim_start().starts_with('>'))
            && nonblank.all(|line| line.trim_start().starts_with('>'))
        {
            return Some(idx);
        }
    }
    None
}

fn quoted_local_message_id(root: Option<&Path>, message: &MessageFile) -> Result<Option<String>> {
    let Some(root) = root else {
        return Ok(None);
    };
    let candidates = message_reply_header_ids(message);
    if candidates.is_empty() {
        return Ok(None);
    }
    let index = Workspace::at(root).rfc822_message_id_index()?;
    for candidate in candidates.into_iter().rev() {
        if let Some(message_id) = index.get(&candidate) {
            return Ok(Some(message_id.clone()));
        }
    }
    Ok(None)
}

pub(super) fn message_template_value(message: &MessageFile) -> Result<Value> {
    serde_json::to_value(message).map_err(|e| AppError::json("serialize message", &e))
}

pub(super) fn markdown_inline(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}

pub(super) fn markdown_image_alt(value: &str) -> String {
    markdown_inline(value)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

pub(super) fn markdown_fence_for(value: &str) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in value.chars() {
        if ch == '`' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(max_run.max(2) + 1)
}

pub(super) fn reply_to_differs_from_from(reply_to: &[String], from: &str) -> bool {
    let from = email_address(from);
    reply_to.len() != 1
        || reply_to
            .first()
            .is_some_and(|value| email_address(value) != from)
}

pub(super) fn render_template(
    root: &Path,
    language: TemplateLanguage,
    key: TemplateKey,
    context: &Value,
) -> Result<String> {
    let mut renderer = MarkdownTemplateRenderer::new(root, language);
    renderer.render(key, context)
}

pub(super) fn markdown_table_cell(value: &str) -> String {
    value
        .replace(['\r', '\n'], " ")
        .replace('|', "\\|")
        .trim()
        .to_string()
}
