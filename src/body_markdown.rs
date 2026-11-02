//! The reading copy of an HTML mail body.
//!
//! `body_text` is what the sender put in the `text/plain` part, kept verbatim
//! because hashes and quote segmentation are computed over it. Newsletters
//! write that part as link text on one line and the full tracking URL on the
//! next, and a mail with only an HTML part gets a tag-stripped text where
//! every link has lost its target. Neither is something a person wants to
//! read. When a message carries an HTML part, this module turns it into
//! Markdown — `<a>` keeps its text and target, structure becomes paragraphs,
//! lists and quotes — and the result travels beside `body_text` as
//! `body_markdown`, purely for reading surfaces.

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::options::{BulletListMarker, Options};
use htmd::{Element, HtmlToMarkdown};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Elements whose content is never mail a person reads: code, styling, metadata,
/// embedded or interactive objects. Their subtrees are dropped whole.
const SKIPPED_TAGS: &[&str] = &[
    "script", "style", "head", "title", "meta", "link", "noscript", "template", "iframe", "frame",
    "frameset", "object", "embed", "applet", "svg", "math", "canvas", "map", "form", "input",
    "button", "select", "option", "textarea", "video", "audio", "source", "track",
];

/// Characters that carry no text: joiners, zero-width marks, the braille blank
/// and soft hyphen that newsletters pad "preheader" lines with so a mail
/// client's preview shows nothing. They are stripped rather than escaped.
fn is_invisible(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}' | '\u{034F}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' | '\u{2800}'
    ) || (ch.is_control() && ch != '\n' && ch != '\t')
}

/// Convert an HTML body to Markdown for reading. `None` when nothing readable
/// remains, so callers fall back to `body_text`.
pub(crate) fn html_to_markdown(html: &str) -> Option<String> {
    let converter = HtmlToMarkdown::builder()
        .options(Options {
            bullet_list_marker: BulletListMarker::Dash,
            ul_bullet_spacing: 1,
            ..Options::default()
        })
        .skip_tags(SKIPPED_TAGS.to_vec())
        // Images become their alt text. A tracking pixel has none and vanishes;
        // a product picture keeps the description the sender wrote for it.
        .add_handler(vec!["img"], |_: &dyn Handlers, element: Element| {
            let alt = element
                .attrs
                .iter()
                .find(|attribute| &attribute.name.local == "alt")
                .map(|attribute| attribute.value.trim().to_string())
                .filter(|alt| !alt.is_empty())?;
            Some(HandlerResult {
                content: alt,
                markdown_translated: true,
            })
        })
        .build();
    let markdown = converter.convert(html).ok()?;
    let cleaned = tidy_markdown(&markdown);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// One block of a Markdown body, as data a template can lay out.
///
/// The Review page is rendered inside AFUI's restricted template environment,
/// where no filter or value may carry pre-rendered markup. So the body is not
/// turned into HTML here; it is turned into this tree, and the template writes
/// the `<p>` and `<a href>` itself with ordinary autoescaping.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarkdownBlock {
    pub kind: MarkdownBlockKind,
    /// A list item's bullet or number, e.g. `•` or `3.`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    pub runs: Vec<MarkdownRun>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MarkdownBlockKind {
    Paragraph,
    Heading,
    ListItem,
    Quote,
    Code,
    Rule,
}

/// A run of text with one style. `href` is present only for `http`, `https`
/// and `mailto` targets; any other link scheme is shown as its text alone.
///
/// The last run of a link also carries what a person needs to judge it
/// without following it: `host`, the registrable domain the link really goes
/// to, and whether that is the sender's own domain.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarkdownRun {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub emphasis: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strong: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
    /// Registrable domain (or bare host) of `href`, on the link's last run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Whether `host` is the sender's registrable domain. One rule, no
    /// heuristics: equal or not. Absent when the sender's domain is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_domain_match: Option<bool>,
}

/// What links are compared against: the sender's registrable domain, from the
/// authenticated domain when there is one, else the `From` domain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkContext {
    pub sender_domain: Option<String>,
}

impl LinkContext {
    pub(crate) fn for_sender(domain: Option<&str>) -> Self {
        Self {
            sender_domain: domain.and_then(registrable_domain),
        }
    }
}

/// The host part of an `http`, `https` or `mailto` URL, lowercased, without
/// userinfo, port or trailing dot.
fn url_host(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let authority = if let Some(rest) = lower.strip_prefix("mailto:") {
        let address = rest.split(['?', '&']).next().unwrap_or_default();
        address.rsplit_once('@')?.1.to_string()
    } else {
        let rest = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if let Some(bracketed) = authority.strip_prefix('[') {
            format!("[{}]", bracketed.split(']').next().unwrap_or_default())
        } else {
            authority
                .rsplit_once(':')
                .filter(|(_, port)| {
                    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
                })
                .map_or(authority, |(host, _)| host)
                .to_string()
        }
    };
    let host = authority.trim_end_matches('.').to_string();
    (!host.is_empty()).then_some(host)
}

/// The registrable domain (`apple.com` for `c.apple.com`, `bbc.co.uk` for
/// `news.bbc.co.uk`) in ASCII form, so a Unicode host and its punycode twin
/// compare equal. Hosts without a public suffix — an IP address, a bare
/// label — come back as they are.
pub(crate) fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    if host.starts_with('[')
        || host
            .split('.')
            .all(|label| label.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Some(host.to_ascii_lowercase());
    }
    let ascii = idna::domain_to_ascii(host).ok()?.to_ascii_lowercase();
    Some(psl::domain_str(&ascii).unwrap_or(&ascii).to_owned())
}

fn safe_link_target(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
        .then(|| url.to_string())
}

#[derive(Default)]
struct BlockBuilder {
    blocks: Vec<MarkdownBlock>,
    current: Option<MarkdownBlock>,
    quote_depth: usize,
    /// One counter per open list: `None` for bullets, `Some(next)` for numbers.
    lists: Vec<Option<u64>>,
    href: Option<String>,
    link_depth: usize,
    emphasis: usize,
    strong: usize,
    image_depth: usize,
    context: LinkContext,
}

impl BlockBuilder {
    /// Close the link that just ended: name its destination on its last run.
    fn finish_link(&mut self) {
        let Some(href) = self.href.take() else {
            return;
        };
        let host = url_host(&href).and_then(|host| registrable_domain(&host));
        let sender_domain_match = match (&host, &self.context.sender_domain) {
            (Some(host), Some(sender)) => Some(host == sender),
            _ => None,
        };
        if let Some(block) = self.current.as_mut()
            && let Some(last) = block
                .runs
                .iter_mut()
                .rev()
                .find(|run| run.href.as_deref() == Some(href.as_str()))
        {
            last.host = host;
            last.sender_domain_match = sender_domain_match;
        }
    }

    fn open(&mut self, kind: MarkdownBlockKind, marker: Option<String>) {
        self.close();
        let kind = if self.quote_depth > 0 && kind == MarkdownBlockKind::Paragraph {
            MarkdownBlockKind::Quote
        } else {
            kind
        };
        self.current = Some(MarkdownBlock {
            kind,
            marker,
            runs: Vec::new(),
        });
    }

    fn close(&mut self) {
        if let Some(block) = self.current.take()
            && (!block.runs.is_empty() || block.kind == MarkdownBlockKind::Rule)
        {
            self.blocks.push(block);
        }
    }

    fn push_text(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        if self.current.is_none() {
            self.open(MarkdownBlockKind::Paragraph, None);
        }
        let href = self.href.clone();
        let emphasis = self.emphasis > 0;
        let strong = self.strong > 0;
        let Some(block) = self.current.as_mut() else {
            return;
        };
        if let Some(last) = block.runs.last_mut()
            && last.href == href
            && last.emphasis == emphasis
            && last.strong == strong
            && last.code == code
        {
            last.text.push_str(text);
            return;
        }
        block.runs.push(MarkdownRun {
            text: text.to_string(),
            href,
            emphasis,
            strong,
            code,
            host: None,
            sender_domain_match: None,
        });
    }
}

/// Lay a Markdown body out as blocks. Raw HTML in the source is text; images
/// contribute their alt text; headings are kept as a kind for the template to
/// style, never as document structure.
pub(crate) fn markdown_blocks(markdown: &str, context: &LinkContext) -> Vec<MarkdownBlock> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut builder = BlockBuilder {
        context: context.clone(),
        ..BlockBuilder::default()
    };
    for event in Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => builder.open(MarkdownBlockKind::Paragraph, None),
                Tag::Heading { .. } => builder.open(MarkdownBlockKind::Heading, None),
                Tag::BlockQuote(_) => {
                    builder.close();
                    builder.quote_depth += 1;
                }
                Tag::CodeBlock(_) => builder.open(MarkdownBlockKind::Code, None),
                Tag::List(start) => {
                    builder.close();
                    builder.lists.push(start);
                }
                Tag::Item => {
                    let marker = match builder.lists.last_mut() {
                        Some(Some(next)) => {
                            let marker = format!("{next}.");
                            *next += 1;
                            marker
                        }
                        _ => "•".to_string(),
                    };
                    builder.open(MarkdownBlockKind::ListItem, Some(marker));
                }
                Tag::Emphasis => builder.emphasis += 1,
                Tag::Strong => builder.strong += 1,
                Tag::Link { dest_url, .. } => {
                    builder.link_depth += 1;
                    if builder.link_depth == 1 {
                        builder.href = safe_link_target(&dest_url);
                    }
                }
                Tag::Image { .. } => builder.image_depth += 1,
                Tag::Table(_) | Tag::TableHead | Tag::TableRow => {
                    builder.open(MarkdownBlockKind::Paragraph, None)
                }
                Tag::TableCell => builder.push_text(" ", false),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::Table
                | TagEnd::TableHead
                | TagEnd::TableRow => builder.close(),
                TagEnd::BlockQuote(_) => {
                    builder.close();
                    builder.quote_depth = builder.quote_depth.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    builder.close();
                    builder.lists.pop();
                }
                TagEnd::Emphasis => builder.emphasis = builder.emphasis.saturating_sub(1),
                TagEnd::Strong => builder.strong = builder.strong.saturating_sub(1),
                TagEnd::Link => {
                    builder.link_depth = builder.link_depth.saturating_sub(1);
                    if builder.link_depth == 0 {
                        builder.finish_link();
                    }
                }
                TagEnd::Image => builder.image_depth = builder.image_depth.saturating_sub(1),
                _ => {}
            },
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                builder.push_text(&text, false)
            }
            Event::Code(code) => builder.push_text(&code, true),
            Event::SoftBreak => builder.push_text(" ", false),
            Event::HardBreak => builder.push_text("\n", false),
            Event::Rule => {
                builder.open(MarkdownBlockKind::Rule, None);
                builder.close();
            }
            Event::FootnoteReference(text) => builder.push_text(&text, false),
            Event::TaskListMarker(checked) => {
                builder.push_text(if checked { "☑ " } else { "☐ " }, false)
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => builder.push_text(&text, false),
        }
    }
    builder.close();
    builder.blocks
}

/// Strip invisible characters, trailing whitespace, and runs of blank lines.
fn tidy_markdown(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut blank_run = 0usize;
    for raw_line in markdown.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let line: String = raw_line.chars().filter(|ch| !is_invisible(*ch)).collect();
        let line = line.trim_end();
        if line.chars().all(char::is_whitespace) {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank_run > 0 {
                out.push('\n');
            }
        }
        blank_run = 0;
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::html_to_markdown;

    #[test]
    fn links_keep_their_text_and_target() {
        let html = r#"<p>前往<a href="https://c.apple.com/r?v=2&amp;a=abc">教育商店选购</a>。</p>"#;
        let markdown = html_to_markdown(html).expect("markdown");
        assert_eq!(
            markdown,
            "前往[教育商店选购](https://c.apple.com/r?v=2&a=abc)。"
        );
    }

    #[test]
    fn images_become_alt_text_and_pixels_vanish() {
        let html = concat!(
            r#"<img src="https://x/pixel.gif" width="1" height="1">"#,
            r#"<p><img src="https://x/hero.jpg" alt="银色 Mac mini 的正面图"></p>"#,
            r#"<p>Text</p>"#
        );
        assert_eq!(
            html_to_markdown(html).expect("markdown"),
            "银色 Mac mini 的正面图\n\nText"
        );
    }

    #[test]
    fn code_styling_and_interactive_content_are_dropped() {
        let html = concat!(
            "<html><head><title>T</title><style>p{color:red}</style></head><body>",
            "<script>alert(1)</script><p>Hello <b>world</b></p>",
            "<form><input value=\"x\"><button>Go</button></form></body></html>"
        );
        assert_eq!(html_to_markdown(html).expect("markdown"), "Hello **world**");
    }

    #[test]
    fn preheader_padding_and_blank_runs_collapse() {
        let html = concat!(
            "<div>\u{034F}\u{2800}\u{2800}\u{2800}\u{200B}</div>",
            "<p>First</p><br><br><br><p>&nbsp;</p><p>Second</p>"
        );
        assert_eq!(html_to_markdown(html).expect("markdown"), "First\n\nSecond");
    }

    #[test]
    fn nothing_readable_yields_none() {
        assert_eq!(
            html_to_markdown("<style>a{}</style><img src=\"p.gif\">"),
            None
        );
        assert_eq!(html_to_markdown(""), None);
    }

    #[test]
    fn blocks_carry_links_styles_and_kinds() {
        let blocks = super::markdown_blocks(
            "# Title\n\n前往[选购](https://c.apple.com/r?a=1)，**now** `x`\n\n- One\n- Two\n\n> Quoted\n\n---\n\n[js](javascript:alert(1)) <b>raw</b>",
            &super::LinkContext::default(),
        );
        let kinds = blocks.iter().map(|block| block.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                super::MarkdownBlockKind::Heading,
                super::MarkdownBlockKind::Paragraph,
                super::MarkdownBlockKind::ListItem,
                super::MarkdownBlockKind::ListItem,
                super::MarkdownBlockKind::Quote,
                super::MarkdownBlockKind::Rule,
                super::MarkdownBlockKind::Paragraph,
            ]
        );
        let paragraph = &blocks[1];
        assert_eq!(paragraph.runs[0].text, "前往");
        assert_eq!(paragraph.runs[1].text, "选购");
        assert_eq!(
            paragraph.runs[1].href.as_deref(),
            Some("https://c.apple.com/r?a=1")
        );
        assert!(
            paragraph
                .runs
                .iter()
                .any(|run| run.strong && run.text == "now")
        );
        assert!(paragraph.runs.iter().any(|run| run.code && run.text == "x"));
        assert_eq!(blocks[2].marker.as_deref(), Some("•"));
        let last = &blocks[6];
        assert!(last.runs.iter().all(|run| run.href.is_none()), "{last:?}");
        assert!(last.runs.iter().any(|run| run.text.contains("<b>raw</b>")));
    }

    #[test]
    fn link_runs_name_their_destination_and_whether_it_is_the_senders() {
        use super::{LinkContext, markdown_blocks};
        let context = LinkContext::for_sender(Some("mail.paypal.com"));
        let blocks = markdown_blocks(
            concat!(
                "[选购](https://c.paypal.com/r?a=1) ",
                "[paypal.com](https://secure-paypa1.com/login) ",
                "[news](https://news.bbc.co.uk:8443/x) ",
                "[support](mailto:help@paypa1.com?subject=hi) ",
                "[ip](http://user@192.168.0.1/x)"
            ),
            &context,
        );
        let runs: Vec<_> = blocks[0]
            .runs
            .iter()
            .filter(|run| run.href.is_some())
            .collect();
        assert_eq!(runs.len(), 5, "{runs:?}");
        assert_eq!(runs[0].host.as_deref(), Some("paypal.com"));
        assert_eq!(runs[0].sender_domain_match, Some(true));
        assert_eq!(runs[1].host.as_deref(), Some("secure-paypa1.com"));
        assert_eq!(runs[1].sender_domain_match, Some(false));
        assert_eq!(runs[2].host.as_deref(), Some("bbc.co.uk"));
        assert_eq!(runs[2].sender_domain_match, Some(false));
        assert_eq!(runs[3].host.as_deref(), Some("paypa1.com"));
        assert_eq!(runs[3].sender_domain_match, Some(false));
        assert_eq!(runs[4].host.as_deref(), Some("192.168.0.1"));

        // Without a sender there is a destination but no verdict.
        let blocks = markdown_blocks("[x](https://example.com/)", &LinkContext::default());
        assert_eq!(blocks[0].runs[0].host.as_deref(), Some("example.com"));
        assert_eq!(blocks[0].runs[0].sender_domain_match, None);
    }

    #[test]
    fn registrable_domains_compare_in_ascii_form() {
        use super::registrable_domain;
        assert_eq!(
            registrable_domain("C.Apple.com."),
            Some("apple.com".to_string())
        );
        assert_eq!(
            registrable_domain("аpple.com"),
            Some("xn--pple-43d.com".to_string())
        );
        assert_ne!(
            registrable_domain("аpple.com"),
            registrable_domain("apple.com")
        );
        assert_eq!(
            registrable_domain("192.168.0.1"),
            Some("192.168.0.1".to_string())
        );
        assert_eq!(
            registrable_domain("localhost"),
            Some("localhost".to_string())
        );
        assert_eq!(registrable_domain(""), None);
    }

    #[test]
    fn quotes_and_lists_keep_their_shape() {
        let html = "<ul><li>One</li><li>Two</li></ul><blockquote><p>Quoted</p></blockquote>";
        assert_eq!(
            html_to_markdown(html).expect("markdown"),
            "- One\n- Two\n\n> Quoted"
        );
    }
}
