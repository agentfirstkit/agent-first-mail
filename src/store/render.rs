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

pub fn clean_body_text(input: &str) -> String {
    let normalized: String = input
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect();
    decode_common_html_entities(&normalized)
}

fn decode_common_html_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let after_amp = &rest[start + 1..];
        let Some(end) = after_amp.find(';') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let entity = &after_amp[..end];
        if let Some(decoded) = decode_html_entity(entity) {
            output.push(decoded);
        } else {
            output.push('&');
            output.push_str(entity);
            output.push(';');
        }
        rest = &after_amp[end + 1..];
    }
    output.push_str(rest);
    output
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "apos" => Some('\''),
        "gt" => Some('>'),
        "lt" => Some('<'),
        "nbsp" => Some(' '),
        "quot" => Some('"'),
        _ => decode_numeric_html_entity(entity),
    }
}

fn decode_numeric_html_entity(entity: &str) -> Option<char> {
    let value = if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        let decimal = entity.strip_prefix('#')?;
        decimal.parse::<u32>().ok()?
    };
    char::from_u32(value)
}

pub fn render_message_section(message: &MessageFile) -> Result<String> {
    render_message_section_with_options(message, TemplateLanguage::default(), None)
}

pub fn render_message_section_with_config(
    root: &Path,
    message: &MessageFile,
    config: &MailConfig,
) -> Result<String> {
    let document = build_message_render_document(
        message,
        config
            .default_identity()
            .ok()
            .map(|identity| identity.email.as_str()),
    );
    let mut renderer = TemplateRenderer::new(root, config.template_language());
    renderer.render_with_options(
        TemplateKey::MessageSection,
        &document,
        TemplateRenderOptions {
            timezone_offset: Some(config.resolved_timezone_offset()),
            output_dir: None,
        },
    )
}

pub fn render_message_section_with_options(
    message: &MessageFile,
    language: TemplateLanguage,
    account_email: Option<&str>,
) -> Result<String> {
    render_message_section_with_root(None, message, language, account_email, None, None)
}

pub(super) fn render_message_section_with_root(
    root: Option<&Path>,
    message: &MessageFile,
    language: TemplateLanguage,
    account_email: Option<&str>,
    display_offset: Option<&FixedOffset>,
    output_dir: Option<&Path>,
) -> Result<String> {
    let document = build_message_render_document(message, account_email);
    let mut renderer = root.map_or_else(
        || TemplateRenderer::builtin(language),
        |root| TemplateRenderer::new(root, language),
    );
    renderer.render_with_options(
        TemplateKey::MessageSection,
        &document,
        TemplateRenderOptions {
            timezone_offset: display_offset.copied(),
            output_dir,
        },
    )
}

pub(super) fn build_message_render_document(
    message: &MessageFile,
    account_email: Option<&str>,
) -> MessageRenderDocument {
    let canonical_message = message.clone();
    let direction = message_thread_direction(&canonical_message);
    let action = message_thread_action(&canonical_message);
    let body = body_analysis(&canonical_message.body_text);
    let link_context = crate::body_markdown::LinkContext::for_sender(
        canonical_message
            .authentication
            .authenticated_domain
            .as_deref()
            .or(canonical_message.authentication.from_domain.as_deref()),
    );
    let body_markdown = canonical_message
        .body_markdown
        .as_deref()
        .map(|markdown| markdown_body_analysis(markdown, &link_context));
    let from = canonical_message.from.as_deref().unwrap_or_default();
    let possible_bcc = possible_bcc(&canonical_message, account_email);
    let reply_to_differs = !canonical_message.reply_to.is_empty()
        && reply_to_differs_from_from(&canonical_message.reply_to, from);
    let sender_differs = canonical_message
        .sender
        .as_deref()
        .is_some_and(|sender| email_address(sender) != email_address(from));
    let authentication_check = matches!(direction, ThreadDirection::Received)
        || canonical_message.authentication.has_results();

    MessageRenderDocument {
        schema_name: message_document_schema_name(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        content_trust: ContentTrust::UntrustedRemote,
        content_provenance: ContentProvenance {
            source: if canonical_message.origin.is_some() {
                ContentSource::FileMessage
            } else {
                ContentSource::RemoteMail
            },
        },
        analysis: MessageRenderAnalysis {
            direction: match direction {
                ThreadDirection::Received => RenderMessageDirection::Inbound,
                ThreadDirection::Sent => RenderMessageDirection::Outbound,
            },
            action: match action {
                ThreadAction::Message => RenderMessageAction::Message,
                ThreadAction::Reply => RenderMessageAction::Reply,
                ThreadAction::Forward => RenderMessageAction::Forward,
            },
            body,
            body_markdown,
            security: MessageSecurityAnalysis {
                authentication_warning: canonical_message.origin.is_none()
                    && authentication_check
                    && canonical_message.authentication.is_warning(),
                possible_bcc,
                reply_to_differs,
                sender_differs,
            },
        },
        message: canonical_message,
    }
}

fn possible_bcc(message: &MessageFile, account_email: Option<&str>) -> bool {
    let Some(account) = account_email
        .map(email_address)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
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
    routed_to_me && !visible_recipients.contains(&account)
}

#[derive(Clone, Debug, Default)]
struct BodySplit {
    quote_start_bytes: Option<usize>,
}

fn markdown_body_analysis(
    markdown: &str,
    link_context: &crate::body_markdown::LinkContext,
) -> crate::render_document::MarkdownBodyAnalysis {
    let analysis = body_analysis(markdown);
    let text_of = |kind: MessageBodySegmentKind| {
        analysis
            .segments
            .iter()
            .filter(|segment| segment.kind == kind)
            .map(|segment| &markdown[segment.start_bytes..segment.end_bytes])
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let authored = crate::body_markdown::markdown_blocks(
        &text_of(MessageBodySegmentKind::Authored),
        link_context,
    );
    let quoted = crate::body_markdown::markdown_blocks(
        &text_of(MessageBodySegmentKind::Quoted),
        link_context,
    );
    crate::render_document::MarkdownBodyAnalysis {
        segments: analysis.segments,
        authored,
        quoted,
    }
}

fn body_analysis(body_text: &str) -> MessageBodyAnalysis {
    let split = split_body_text(body_text);
    let authored_source = split
        .quote_start_bytes
        .map_or(body_text, |start| &body_text[..start]);
    let mut segments = Vec::new();
    let authored_end_bytes = authored_source.trim_end().len();
    if authored_end_bytes > 0 {
        segments.push(MessageBodySegment {
            kind: MessageBodySegmentKind::Authored,
            start_bytes: 0,
            end_bytes: authored_end_bytes,
        });
    }
    if let Some(start_bytes) = split.quote_start_bytes {
        let end_bytes = start_bytes + body_text[start_bytes..].trim_end().len();
        if end_bytes > start_bytes {
            segments.push(MessageBodySegment {
                kind: MessageBodySegmentKind::Quoted,
                start_bytes,
                end_bytes,
            });
        }
    }
    MessageBodyAnalysis { segments }
}

fn split_body_text(body_text: &str) -> BodySplit {
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut start_bytes = 0usize;
    for segment in body_text.split_inclusive('\n') {
        starts.push(start_bytes);
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        lines.push(line.strip_suffix('\r').unwrap_or(line));
        start_bytes += segment.len();
    }
    for (index, line) in lines.iter().enumerate() {
        if is_apple_wrote_line(line) {
            return BodySplit {
                quote_start_bytes: starts.get(index).copied(),
            };
        }
    }
    if let Some(index) = trailing_quote_block_start(&lines) {
        return BodySplit {
            quote_start_bytes: starts.get(index).copied(),
        };
    }
    BodySplit {
        quote_start_bytes: None,
    }
}

fn is_apple_wrote_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("On ") && trimmed.ends_with(" wrote:")
}

fn trailing_quote_block_start(lines: &[&str]) -> Option<usize> {
    for index in 0..lines.len() {
        let rest = &lines[index..];
        let mut nonblank = rest.iter().filter(|line| !line.trim().is_empty());
        if nonblank
            .clone()
            .next()
            .is_some_and(|line| line.trim_start().starts_with('>'))
            && nonblank.all(|line| line.trim_start().starts_with('>'))
        {
            return Some(index);
        }
    }
    None
}

pub(super) fn collection_render_document(
    collection: MessageCollection,
    messages: Vec<MessageRenderDocument>,
    location: Option<CollectionLocation>,
) -> CollectionRenderDocument {
    CollectionRenderDocument {
        schema_name: collection_document_schema_name(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        collection,
        messages,
        location,
    }
}

pub(super) fn collection_message_render_document(
    collection: MessageCollection,
    message: MessageRenderDocument,
    location: Option<CollectionLocation>,
) -> CollectionMessageRenderDocument {
    CollectionMessageRenderDocument {
        schema_name: collection_message_document_schema_name(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        collection,
        message,
        location,
    }
}

pub(super) fn triage_render_document(
    message: MessageRenderDocument,
    related_messages: Vec<MessageRenderDocument>,
) -> TriageRenderDocument {
    TriageRenderDocument {
        schema_name: triage_document_schema_name(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        context: MessageContextDocument {
            message,
            related_messages,
            related_message_issues: Vec::new(),
        },
    }
}

pub(super) fn case_collection_location(group: String) -> CollectionLocation {
    CollectionLocation { group }
}

impl Workspace {
    pub(crate) fn case_render_document(&self, case_ref: &str) -> Result<CollectionRenderDocument> {
        self.require_workspace()?;
        let resolved = self.resolve_case_any(case_ref)?;
        let collection = read_case_file(&resolved.path)?;
        let config = MailConfig::load(&self.root)?;
        let messages = collection
            .message_ids()
            .iter()
            .map(|message_id| {
                let message = self.read_message_by_id(message_id)?;
                self.message_render_document_from_parts(&message, &config)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(collection_render_document(
            collection,
            messages,
            Some(case_collection_location(resolved.group)),
        ))
    }

    pub(crate) fn notification_render_document(
        &self,
        notification_ref: &str,
    ) -> Result<CollectionRenderDocument> {
        self.require_workspace()?;
        let (notification_uid, _) = self.resolve_notification(notification_ref)?;
        let collection = self.read_notifications(&notification_uid)?;
        let config = MailConfig::load(&self.root)?;
        let messages = collection
            .message_ids()
            .iter()
            .map(|message_id| {
                let message = self.read_message_by_id(message_id)?;
                self.message_render_document_from_parts(&message, &config)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(collection_render_document(collection, messages, None))
    }

    pub(super) fn message_render_document_from_parts(
        &self,
        message: &MessageFile,
        config: &MailConfig,
    ) -> Result<MessageRenderDocument> {
        Ok(build_message_render_document(
            message,
            Some(config.default_identity()?.email.as_str()),
        ))
    }

    pub(super) fn message_render_document_with_context(
        &self,
        message: &MessageFile,
        config: &MailConfig,
        _render_context: Option<&RenderContext>,
    ) -> Result<MessageRenderDocument> {
        self.message_render_document_from_parts(message, config)
    }

    pub(super) fn message_render_documents_with_context(
        &self,
        messages: &[MessageFile],
        config: &MailConfig,
        _render_context: Option<&RenderContext>,
    ) -> Result<Vec<MessageRenderDocument>> {
        let default_identity_email = config.default_identity()?.email.as_str();
        Ok(messages
            .iter()
            .map(|message| build_message_render_document(message, Some(default_identity_email)))
            .collect())
    }
}

pub(super) fn reply_to_differs_from_from(reply_to: &[String], from: &str) -> bool {
    let from = email_address(from);
    reply_to.len() != 1
        || reply_to
            .first()
            .is_some_and(|value| email_address(value) != from)
}

pub(super) fn render_template<T: Serialize>(
    root: &Path,
    language: TemplateLanguage,
    key: TemplateKey,
    context: &T,
) -> Result<String> {
    let mut renderer = TemplateRenderer::new(root, language);
    renderer.render(key, context)
}
