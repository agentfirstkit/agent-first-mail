use crate::error::{AppError, Result};
use noyalib::{
    DuplicateKeyPolicy, ParserConfig, from_str_with_config, to_string as yaml_to_string,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub const CONVERSATION_START: &str = "<!-- afmail:conversation:start -->";
pub const CONVERSATION_END: &str = "<!-- afmail:conversation:end -->";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownDoc {
    pub frontmatter: String,
    pub body: String,
}

pub fn split_frontmatter(input: &str) -> Result<MarkdownDoc> {
    let mut lines = input.lines();
    if lines.next() != Some("---") {
        return Err(AppError::new(
            "invalid_request",
            "markdown file is missing frontmatter",
        ));
    }
    let mut frontmatter_lines = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter && line == "---" {
            in_frontmatter = false;
            continue;
        }
        if in_frontmatter {
            frontmatter_lines.push(line);
        } else {
            body_lines.push(line);
        }
    }
    if in_frontmatter {
        return Err(AppError::new(
            "invalid_request",
            "markdown frontmatter is not closed",
        ));
    }
    Ok(MarkdownDoc {
        frontmatter: frontmatter_lines.join("\n"),
        body: body_lines.join("\n"),
    })
}

/// Deserialize a YAML frontmatter block into a typed struct.
pub fn parse_frontmatter<T: DeserializeOwned + 'static>(frontmatter: &str) -> Result<T> {
    let parser_config = ParserConfig::new().duplicate_key_policy(DuplicateKeyPolicy::Error);
    let normalized = normalize_bare_null_values(frontmatter);
    from_str_with_config(&normalized, &parser_config)
        .map_err(|e| AppError::new("invalid_request", format!("invalid frontmatter: {e}")))
}

// noyalib 0.0.13 rejects consecutive bare null mapping values (`cc:` followed
// by `attachments:`), while afmail has always accepted that author-friendly
// spelling. Make those nulls explicit without touching keys that introduce an
// indented mapping or sequence.
fn normalize_bare_null_values(frontmatter: &str) -> String {
    let lines = frontmatter.lines().collect::<Vec<_>>();
    let mut normalized = String::with_capacity(frontmatter.len());
    for (index, line) in lines.iter().enumerate() {
        normalized.push_str(line);
        if is_bare_mapping_value(line) && !has_indented_child(&lines, index) {
            normalized.push_str(" null");
        }
        if index + 1 < lines.len() {
            normalized.push('\n');
        }
    }
    normalized
}

fn is_bare_mapping_value(line: &str) -> bool {
    let content = line.trim_start();
    if content.is_empty() || content.starts_with(['#', '-']) {
        return false;
    }
    content
        .split_once(':')
        .is_some_and(|(key, value)| !key.trim().is_empty() && value.trim().is_empty())
}

fn has_indented_child(lines: &[&str], parent_index: usize) -> bool {
    let parent_indent = line_indent(lines[parent_index]);
    lines
        .iter()
        .skip(parent_index + 1)
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .is_some_and(|line| line_indent(line) > parent_indent)
}

fn line_indent(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

/// Split a markdown document and deserialize its frontmatter, returning the
/// parsed struct and the raw body (untouched markdown).
pub fn read_doc<T: DeserializeOwned + 'static>(input: &str) -> Result<(T, String)> {
    let doc = split_frontmatter(input)?;
    let parsed = parse_frontmatter::<T>(&doc.frontmatter)?;
    Ok((parsed, doc.body))
}

/// Re-attach a serialized frontmatter struct to an untouched markdown body.
pub fn render_frontmatter<T: Serialize>(frontmatter: &T, body: &str) -> Result<String> {
    let yaml = yaml_to_string(frontmatter)
        .map_err(|e| AppError::new("internal", format!("serialize frontmatter: {e}")))?;
    Ok(format!(
        "---\n{}\n---\n{}\n",
        yaml.trim_end_matches(['\r', '\n']),
        body.trim_start()
    ))
}

pub fn extract_conversation(input: &str) -> Result<String> {
    let start = input.find(CONVERSATION_START).ok_or_else(|| {
        AppError::new("invalid_request", "conversation start marker was not found")
    })?;
    let after_start = start + CONVERSATION_START.len();
    let end_rel = input[after_start..]
        .find(CONVERSATION_END)
        .ok_or_else(|| AppError::new("invalid_request", "conversation end marker was not found"))?;
    let end = after_start + end_rel;
    Ok(input[after_start..end].trim_matches('\n').to_string())
}

pub fn append_conversation(case_md: &str, conversation: &str) -> Result<String> {
    let end = case_md.find(CONVERSATION_END).ok_or_else(|| {
        AppError::new(
            "invalid_request",
            "case conversation end marker was not found",
        )
    })?;
    let before = case_md[..end].trim_end();
    let after = &case_md[end..];
    Ok(format!("{before}\n\n{}\n\n{after}", conversation.trim()))
}

pub fn replace_conversation(markdown: &str, conversation: &str) -> Result<String> {
    let start = markdown.find(CONVERSATION_START).ok_or_else(|| {
        AppError::new("invalid_request", "conversation start marker was not found")
    })?;
    let after_start = start + CONVERSATION_START.len();
    let end_rel = markdown[after_start..]
        .find(CONVERSATION_END)
        .ok_or_else(|| AppError::new("invalid_request", "conversation end marker was not found"))?;
    let end = after_start + end_rel;
    let before = markdown[..after_start].trim_end();
    let after = markdown[end..].trim_start_matches('\n');
    Ok(format!("{before}\n\n{}\n\n{after}", conversation.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_appends_conversation() {
        let triage = format!("a\n{CONVERSATION_START}\nhello\n{CONVERSATION_END}\nz");
        let conversation = extract_conversation(&triage);
        assert_eq!(conversation, Ok("hello".to_string()));
        let case_md = format!("a\n{CONVERSATION_START}\nold\n{CONVERSATION_END}\nz");
        let updated = append_conversation(&case_md, "new");
        assert!(updated.is_ok());
        assert!(
            updated
                .as_ref()
                .map(|s| s.contains("old\n\nnew"))
                .unwrap_or(false)
        );
        let replaced = replace_conversation(&case_md, "new");
        assert_eq!(
            replaced,
            Ok(format!(
                "a\n{CONVERSATION_START}\n\nnew\n\n{CONVERSATION_END}\nz"
            ))
        );
    }

    #[test]
    fn normalizes_only_bare_null_mapping_values() {
        let input = "to:\n  - a@example.com\ncc:\nmetadata:\n  author: example\nattachments:";
        assert_eq!(
            normalize_bare_null_values(input),
            "to:\n  - a@example.com\ncc: null\nmetadata:\n  author: example\nattachments: null"
        );
    }
}
