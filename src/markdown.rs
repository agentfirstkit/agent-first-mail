use crate::error::{AppError, Result};
use noyalib::{
    DuplicateKeyPolicy, ParserConfig, from_str_with_config, to_string as yaml_to_string,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_only_bare_null_mapping_values() {
        let input = "to:\n  - a@example.com\ncc:\nmetadata:\n  author: example\nattachments:";
        assert_eq!(
            normalize_bare_null_values(input),
            "to:\n  - a@example.com\ncc: null\nmetadata:\n  author: example\nattachments: null"
        );
    }
}
