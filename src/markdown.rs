use crate::error::{AppError, Result};
use agent_first_data::document::{Addressing, Document, Format, Value as AfValue, get_path};
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
    // The trailing newline is load-bearing. A final key with no value —
    // `attachments:`, which afmail's own draft scaffolding writes — is a null
    // only when its line is terminated; unterminated it reads as the start of
    // something the parser never finds. `split_frontmatter` joins lines without
    // one, so the block always arrives here missing it. The predecessor instead
    // rewrote every bare key in the block to an explicit ` null`: forty lines
    // of line-classification standing in for a newline.
    let terminated = format!("{}\n", frontmatter.trim_end_matches(['\r', '\n']));
    from_str_with_config(&terminated, &parser_config)
        .map_err(|e| AppError::new("invalid_request", format!("invalid frontmatter: {e}")))
}

/// Split a markdown document and deserialize its frontmatter, returning the
/// parsed struct and the raw body (untouched markdown).
pub fn read_doc<T: DeserializeOwned + 'static>(input: &str) -> Result<(T, String)> {
    let doc = split_frontmatter(input)?;
    let parsed = parse_frontmatter::<T>(&doc.frontmatter)?;
    Ok((parsed, doc.body))
}

/// Write `frontmatter` back into `source`, touching only the keys whose values
/// actually changed.
///
/// [`render_frontmatter`] rebuilds the whole block from the struct, which
/// normalizes everything the author did: comments disappear, key order becomes
/// field-declaration order, quote styles are rewritten, and blank lines at the
/// top of the body are trimmed. That is right for a file being created and
/// wrong for one being edited — these files are meant to be edited by hand as
/// well as by afmail. Here the frontmatter is edited through afdata's document
/// layer, so unrelated bytes survive, and only a caller that also changes the
/// body pays for re-rendering it.
///
/// Falls back to a full render when the source has no frontmatter to preserve,
/// or when a key would be read as a dot-path rather than a name.
pub fn edit_frontmatter<T: Serialize>(source: &str, frontmatter: &T, body: &str) -> Result<String> {
    let desired = serde_json::to_value(frontmatter)
        .map_err(|e| AppError::new("internal", format!("serialize frontmatter: {e}")))?;
    let Some(desired) = desired.as_object() else {
        return render_frontmatter(frontmatter, body);
    };
    if desired
        .keys()
        .any(|key| key.contains('.') || key.contains('\\'))
    {
        return render_frontmatter(frontmatter, body);
    }
    let Ok(mut document) = Document::parse(source, Format::YamlFrontmatter) else {
        return render_frontmatter(frontmatter, body);
    };
    for (key, want) in desired {
        let want = AfValue::from(want.clone());
        let current = get_path(document.value(), key, Addressing::INDEX_ONLY).ok();
        if current.as_ref() == Some(&want) {
            continue;
        }
        document
            .set(key, want)
            .map_err(|e| AppError::new("internal", format!("edit frontmatter `{key}`: {e}")))?;
    }
    let edited = document.source();
    // The body is the caller's to replace, but most callers hand back the one
    // they read. Keeping the edited source whole in that case preserves the
    // blank lines around it that a re-render would collapse.
    match split_frontmatter(edited) {
        Ok(split) if split.body == body => Ok(edited.to_string()),
        _ => render_frontmatter(frontmatter, body),
    }
}

/// Re-attach a serialized frontmatter struct to an untouched markdown body.
///
/// For a file that already exists, prefer [`edit_frontmatter`]: this rebuilds
/// the block from the struct and drops whatever the struct does not model.
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
    fn a_key_with_no_value_reads_as_null() {
        // afmail's own draft scaffolding writes `cc:` and `attachments:` with
        // nothing after them, and authors write them that way by hand. This
        // used to need a pre-pass that spelled the nulls out, because the YAML
        // parser of the day rejected consecutive ones. The parser handles it
        // now — but the promise is afmail's, not the parser's, so it is pinned
        // here rather than left to whichever library is underneath.
        let (parsed, _) = read_doc::<serde_json::Value>(
            "---\nto:\n  - a@example.com\ncc:\nattachments:\n---\n\nBody\n",
        )
        .expect("a bare key is a null value");
        assert_eq!(parsed["cc"], serde_json::Value::Null);
        assert_eq!(parsed["attachments"], serde_json::Value::Null);
        assert_eq!(parsed["to"][0], "a@example.com");
    }
}
