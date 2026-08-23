use crate::types::DraftCompleteness;
use std::fmt;

pub(crate) const COMPOSE_START: &str = "<!-- afmail:compose:start -->";
pub(crate) const COMPOSE_END: &str = "<!-- afmail:compose:end -->";
pub(crate) const QUOTE_START: &str = "<!-- afmail:quote:start -->";
pub(crate) const QUOTE_END: &str = "<!-- afmail:quote:end -->";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DraftContent<'a> {
    body: &'a str,
    compose_start: MarkerLine,
    compose_end: MarkerLine,
    quote: Option<(MarkerLine, MarkerLine)>,
}

impl<'a> DraftContent<'a> {
    pub(crate) fn compose(self) -> &'a str {
        &self.body[self.compose_start.after_line..self.compose_end.line_start]
    }

    #[cfg(test)]
    pub(crate) fn authored_text(self) -> &'a str {
        self.compose()
    }

    pub(crate) fn authored_text_range(self) -> std::ops::Range<usize> {
        let raw_start = self.compose_start.after_line;
        let raw_end = self.compose_end.line_start;
        let raw = &self.body[raw_start..raw_end];
        let leading = raw.len() - raw.trim_start_matches(['\r', '\n']).len();
        let trimmed = raw.trim_matches(['\r', '\n']);
        let start = raw_start + leading;
        start..start + trimmed.len()
    }

    #[cfg(test)]
    pub(crate) fn quote(self) -> Option<&'a str> {
        self.quote
            .map(|(start, end)| &self.body[start.after_line..end.line_start])
    }

    pub(crate) fn has_authored_text(self) -> bool {
        contains_authored_text(self.compose())
    }

    pub(crate) fn completeness(self) -> DraftCompleteness {
        if self.has_authored_text() {
            DraftCompleteness::Complete
        } else {
            DraftCompleteness::Incomplete
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DraftContentError {
    MissingMarker(&'static str),
    DuplicateMarker(&'static str),
    InvalidMarkerOrder {
        start: &'static str,
        end: &'static str,
    },
    QuoteBeforeComposeEnd,
}

impl fmt::Display for DraftContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMarker(marker) => {
                write!(
                    formatter,
                    "draft content is missing required marker: {marker}"
                )
            }
            Self::DuplicateMarker(marker) => {
                write!(
                    formatter,
                    "draft content contains duplicate marker: {marker}"
                )
            }
            Self::InvalidMarkerOrder { start, end } => write!(
                formatter,
                "draft content marker {start} must appear before {end}"
            ),
            Self::QuoteBeforeComposeEnd => write!(
                formatter,
                "draft quote section must appear after the compose section"
            ),
        }
    }
}

impl std::error::Error for DraftContentError {}

pub(crate) fn parse_draft_content(body: &str) -> Result<DraftContent<'_>, DraftContentError> {
    let compose_start = required_marker(body, COMPOSE_START)?;
    let compose_end = required_marker(body, COMPOSE_END)?;
    ensure_order(compose_start, compose_end, COMPOSE_START, COMPOSE_END)?;

    let quote_start = find_unique_marker(body, QUOTE_START)?;
    let quote_end = find_unique_marker(body, QUOTE_END)?;
    let quote = match (quote_start, quote_end) {
        (None, None) => None,
        (None, Some(_)) => return Err(DraftContentError::MissingMarker(QUOTE_START)),
        (Some(_), None) => return Err(DraftContentError::MissingMarker(QUOTE_END)),
        (Some(start), Some(end)) => {
            ensure_order(start, end, QUOTE_START, QUOTE_END)?;
            if start.line_start < compose_end.after_line {
                return Err(DraftContentError::QuoteBeforeComposeEnd);
            }
            Some((start, end))
        }
    };

    Ok(DraftContent {
        body,
        compose_start,
        compose_end,
        quote,
    })
}

pub(crate) fn replace_authored_text(
    body: &str,
    authored_text: &str,
) -> Result<String, DraftContentError> {
    let parsed = parse_draft_content(body)?;
    let authored_text = authored_text.trim_matches(|character| matches!(character, '\r' | '\n'));
    let line_ending = marker_line_ending(body, parsed.compose_start);
    let mut updated = String::with_capacity(
        body.len()
            .saturating_sub(parsed.compose().len())
            .saturating_add(authored_text.len())
            .saturating_add(line_ending.len()),
    );
    updated.push_str(&body[..parsed.compose_start.after_line]);
    if !authored_text.is_empty() {
        updated.push_str(authored_text);
    }
    updated.push_str(line_ending);
    updated.push_str(&body[parsed.compose_end.line_start..]);
    Ok(updated)
}

pub(crate) fn outbound_body(body: &str) -> Result<String, DraftContentError> {
    let parsed = parse_draft_content(body)?;
    let body_after_heading = &body[parsed.compose_start.after_line..];
    let without_markers = remove_schema_marker_lines(body_after_heading);
    Ok(without_markers
        .trim_matches(|character| matches!(character, '\r' | '\n'))
        .to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MarkerLine {
    line_start: usize,
    after_line: usize,
}

fn required_marker(body: &str, marker: &'static str) -> Result<MarkerLine, DraftContentError> {
    find_unique_marker(body, marker)?.ok_or(DraftContentError::MissingMarker(marker))
}

fn find_unique_marker(
    body: &str,
    marker: &'static str,
) -> Result<Option<MarkerLine>, DraftContentError> {
    let mut found = None;
    let mut line_start = 0;

    for segment in body.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == marker {
            if found.is_some() {
                return Err(DraftContentError::DuplicateMarker(marker));
            }
            found = Some(MarkerLine {
                line_start,
                after_line: line_start + segment.len(),
            });
        }
        line_start += segment.len();
    }

    Ok(found)
}

fn marker_line_ending(body: &str, marker: MarkerLine) -> &'static str {
    let marker_line = &body[marker.line_start..marker.after_line];
    if marker_line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn remove_schema_marker_lines(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for segment in input.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !matches!(line, COMPOSE_END | QUOTE_START | QUOTE_END) {
            output.push_str(segment);
        }
    }
    output
}

fn ensure_order(
    start: MarkerLine,
    end: MarkerLine,
    start_name: &'static str,
    end_name: &'static str,
) -> Result<(), DraftContentError> {
    if start.after_line > end.line_start {
        return Err(DraftContentError::InvalidMarkerOrder {
            start: start_name,
            end: end_name,
        });
    }
    Ok(())
}

fn contains_authored_text(compose: &str) -> bool {
    let mut in_comment = false;
    compose.lines().any(|line| {
        let visible = strip_html_comments(line, &mut in_comment);
        !visible.trim().is_empty()
    })
}

fn strip_html_comments(line: &str, in_comment: &mut bool) -> String {
    let mut visible = String::with_capacity(line.len());
    let mut remaining = line;

    loop {
        if *in_comment {
            let Some(comment_end) = remaining.find("-->") else {
                break;
            };
            remaining = &remaining[comment_end + 3..];
            *in_comment = false;
            continue;
        }

        let Some(comment_start) = remaining.find("<!--") else {
            visible.push_str(remaining);
            break;
        };
        visible.push_str(&remaining[..comment_start]);
        remaining = &remaining[comment_start + 4..];
        *in_comment = true;
    }

    visible
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;

    const NEW_TEMPLATES: [&str; 9] = [
        include_str!("../templates/de-DE/draft/new.md.j2"),
        include_str!("../templates/en-US/draft/new.md.j2"),
        include_str!("../templates/es-ES/draft/new.md.j2"),
        include_str!("../templates/fr-FR/draft/new.md.j2"),
        include_str!("../templates/ja-JP/draft/new.md.j2"),
        include_str!("../templates/ko-KR/draft/new.md.j2"),
        include_str!("../templates/pt-BR/draft/new.md.j2"),
        include_str!("../templates/zh-CN/draft/new.md.j2"),
        include_str!("../templates/zh-TW/draft/new.md.j2"),
    ];
    const REPLY_TEMPLATES: [&str; 9] = [
        include_str!("../templates/de-DE/draft/reply.md.j2"),
        include_str!("../templates/en-US/draft/reply.md.j2"),
        include_str!("../templates/es-ES/draft/reply.md.j2"),
        include_str!("../templates/fr-FR/draft/reply.md.j2"),
        include_str!("../templates/ja-JP/draft/reply.md.j2"),
        include_str!("../templates/ko-KR/draft/reply.md.j2"),
        include_str!("../templates/pt-BR/draft/reply.md.j2"),
        include_str!("../templates/zh-CN/draft/reply.md.j2"),
        include_str!("../templates/zh-TW/draft/reply.md.j2"),
    ];

    #[test]
    fn all_new_templates_have_an_empty_compose_section() {
        for template in NEW_TEMPLATES {
            let parsed = parse_draft_content(template).expect("parse new draft template");
            assert!(!parsed.has_authored_text());
            assert_eq!(parsed.quote(), None);
        }
    }

    #[test]
    fn all_reply_templates_have_empty_compose_and_separate_quote_sections() {
        for template in REPLY_TEMPLATES {
            let parsed = parse_draft_content(template).expect("parse reply draft template");
            assert!(!parsed.has_authored_text());
            assert_eq!(parsed.compose().trim(), "");
            assert_eq!(
                parsed.quote().map(str::trim),
                Some("{{ parent.body_text | markdown_quote }}")
            );
        }
    }

    #[test]
    fn detects_authored_text_only_inside_compose_section() {
        let body = format!(
            "# Reply\n{COMPOSE_START}\nThanks, I will investigate.\n{COMPOSE_END}\n\
             Original message\n{QUOTE_START}\nPlain original text\n{QUOTE_END}\n\
             -- \nSupport Agent\n"
        );

        assert_eq!(
            parse_draft_content(&body).map(DraftContent::has_authored_text),
            Ok(true)
        );
        assert_eq!(
            parse_draft_content(&body).map(DraftContent::completeness),
            Ok(DraftCompleteness::Complete)
        );
    }

    #[test]
    fn quote_footer_and_template_heading_outside_compose_do_not_count() {
        let body = format!(
            "# Reply\n{COMPOSE_START}\n<!-- note\nstill a comment -->\n{COMPOSE_END}\n\
             Original message\n{QUOTE_START}\nPlain original text\n{QUOTE_END}\n\
             -- \nSupport Agent\n"
        );

        assert_eq!(
            parse_draft_content(&body).map(DraftContent::has_authored_text),
            Ok(false)
        );
        assert_eq!(
            parse_draft_content(&body).map(DraftContent::completeness),
            Ok(DraftCompleteness::Incomplete)
        );
    }

    #[test]
    fn authored_markdown_heading_or_blockquote_counts_inside_compose() {
        for authored in ["# Status update", "> Confirmed"] {
            let body = format!("{COMPOSE_START}\n{authored}\n{COMPOSE_END}\n");
            assert_eq!(
                parse_draft_content(&body).map(DraftContent::has_authored_text),
                Ok(true)
            );
        }
    }

    #[test]
    fn completeness_uses_stable_snake_case_values() {
        assert_eq!(
            serde_json::to_string(&DraftCompleteness::Complete).ok(),
            Some("\"complete\"".to_string())
        );
        assert_eq!(
            serde_json::to_string(&DraftCompleteness::Incomplete).ok(),
            Some("\"incomplete\"".to_string())
        );
    }

    #[test]
    fn replaces_only_authored_text_and_preserves_quote_and_footer() {
        let body = format!(
            "# Reply\n{COMPOSE_START}\nold text\n{COMPOSE_END}\n\
             Original message\n{QUOTE_START}\n> original\n{QUOTE_END}\n\n-- \nSupport Agent\n"
        );
        let updated = replace_authored_text(&body, "\nnew text\n").expect("replace authored text");
        let parsed = parse_draft_content(&updated).expect("parse updated draft");

        assert_eq!(parsed.authored_text(), "new text\n");
        assert_eq!(parsed.quote(), Some("> original\n"));
        assert!(updated.ends_with("-- \nSupport Agent\n"));
    }

    #[test]
    fn outbound_body_removes_template_heading_and_schema_markers() {
        let body = format!(
            "# Reply\n{COMPOSE_START}\nThanks.\n{COMPOSE_END}\n\
             Original message\n{QUOTE_START}\n> original\n{QUOTE_END}\n\n-- \nSupport Agent\n"
        );

        assert_eq!(
            outbound_body(&body),
            Ok("Thanks.\nOriginal message\n> original\n\n-- \nSupport Agent".to_string())
        );
    }

    #[test]
    fn text_next_to_an_html_comment_counts() {
        let body = format!("{COMPOSE_START}\n<!-- private note --> 已处理\n{COMPOSE_END}\n");

        assert_eq!(
            parse_draft_content(&body).map(DraftContent::has_authored_text),
            Ok(true)
        );
    }

    #[test]
    fn old_draft_without_compose_schema_is_rejected() {
        assert_eq!(
            parse_draft_content("# Reply\n\nThanks").map(DraftContent::has_authored_text),
            Err(DraftContentError::MissingMarker(COMPOSE_START))
        );
    }

    #[test]
    fn duplicate_or_out_of_order_markers_are_rejected() {
        let duplicate = format!("{COMPOSE_START}\none\n{COMPOSE_START}\ntwo\n{COMPOSE_END}\n");
        assert_eq!(
            parse_draft_content(&duplicate),
            Err(DraftContentError::DuplicateMarker(COMPOSE_START))
        );

        let out_of_order = format!("{COMPOSE_END}\n{COMPOSE_START}\n");
        assert_eq!(
            parse_draft_content(&out_of_order),
            Err(DraftContentError::InvalidMarkerOrder {
                start: COMPOSE_START,
                end: COMPOSE_END,
            })
        );
    }

    #[test]
    fn quote_markers_are_a_pair_after_compose() {
        let missing_end = format!("{COMPOSE_START}\ntext\n{COMPOSE_END}\n{QUOTE_START}\n");
        assert_eq!(
            parse_draft_content(&missing_end),
            Err(DraftContentError::MissingMarker(QUOTE_END))
        );

        let quote_first =
            format!("{QUOTE_START}\noriginal\n{QUOTE_END}\n{COMPOSE_START}\ntext\n{COMPOSE_END}\n");
        assert_eq!(
            parse_draft_content(&quote_first),
            Err(DraftContentError::QuoteBeforeComposeEnd)
        );
    }

    #[test]
    fn accepts_crlf_marker_lines() {
        let body = format!("# Reply\r\n{COMPOSE_START}\r\nhello\r\n{COMPOSE_END}\r\n");
        let parsed = parse_draft_content(&body).expect("parse CRLF draft body");

        assert_eq!(parsed.compose(), "hello\r\n");
        assert!(parsed.has_authored_text());

        let updated = replace_authored_text(&body, "updated").expect("replace CRLF authored text");
        assert!(updated.contains(&format!("{COMPOSE_START}\r\nupdated\r\n{COMPOSE_END}")));
    }
}
