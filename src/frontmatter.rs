//! Typed metadata for markdown frontmatter and adjacent JSON state.
//!
//! Draft and triage markdown use `markdown::read_doc::<T>` and
//! `markdown::render_frontmatter`; cases store the same typed metadata in
//! `data/case.json`. Body content (conversation blocks, notes sections) is never
//! modeled here; it stays raw markdown.

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a sequence that may be written as YAML `null` (a bare `key:`
/// with no value) into an empty `Vec`. Agents hand-write drafts and frequently
/// leave `attachments:` / `cc:` empty.
fn de_null_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

fn default_active() -> String {
    "active".to_string()
}

/// Frontmatter of an agent-authored `drafts/*.md`. Shared by draft validation
/// and outbound message building so both agree on one schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DraftFrontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub case_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub to: Vec<String>,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub cc: Vec<String>,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub attachments: Vec<String>,
}

/// Canonical case metadata stored in `data/case.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaseFrontmatter {
    pub kind: String,
    pub case_uid: String,
    pub case_name: String,
    #[serde(default = "default_active")]
    pub status: String,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_rfc3339: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_rfc3339: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_rfc3339: Option<String>,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub thread_count: usize,
    #[serde(default)]
    pub attachment_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_rfc3339: Option<String>,
}

/// Frontmatter of a generated `triage/message_*.md` view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TriageFrontmatter {
    pub kind: String,
    pub message_id: String,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub message_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_rfc3339: Option<String>,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub attachment_count: usize,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub suggested_case_uids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_reason: Option<String>,
}

/// Frontmatter of generated `cases/<group>/<case-uid>/views/messages/<message-id>.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CaseMessageFrontmatter {
    pub kind: String,
    pub case_uid: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_rfc3339: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{read_doc, render_frontmatter};

    #[test]
    fn draft_tolerates_empty_arrays() {
        let text = "---\nkind: draft\ncase_uid: c20260521001\nsubject: Hello\nto:\n  - a@example.com\ncc:\nattachments:\n---\nbody";
        let parsed = read_doc::<DraftFrontmatter>(text);
        assert!(parsed.is_ok());
        if let Ok((fm, body)) = parsed {
            assert_eq!(fm.case_uid, "c20260521001");
            assert_eq!(fm.subject.as_deref(), Some("Hello"));
            assert_eq!(fm.to, vec!["a@example.com".to_string()]);
            assert!(fm.cc.is_empty());
            assert!(fm.attachments.is_empty());
            assert_eq!(body.trim(), "body");
        }
    }

    #[test]
    fn case_round_trips_through_render() {
        let fm = CaseFrontmatter {
            kind: "case".to_string(),
            case_uid: "c20260521001".to_string(),
            case_name: "Acme".to_string(),
            status: "active".to_string(),
            tags: vec!["legal".to_string()],
            created_rfc3339: Some("2026-05-30T00:00:00Z".to_string()),
            updated_rfc3339: Some("2026-05-30T00:00:00Z".to_string()),
            archived_rfc3339: None,
            message_count: 2,
            thread_count: 0,
            attachment_count: 1,
            last_message_rfc3339: None,
        };
        let body = "\n# Title\n\n<!-- afmail:conversation:start -->\nhi\n<!-- afmail:conversation:end -->\n";
        let rendered = render_frontmatter(&fm, body);
        assert!(rendered.is_ok());
        if let Ok(rendered) = rendered {
            let reparsed = read_doc::<CaseFrontmatter>(&rendered);
            assert!(reparsed.is_ok());
            if let Ok((parsed, parsed_body)) = reparsed {
                assert_eq!(parsed, fm);
                assert_eq!(parsed_body, body.trim_start());
            }
        }
    }

    #[test]
    fn case_defaults_status_active_when_missing() {
        let text = "---\nkind: case\ncase_uid: c20260530001\ncase_name: Bare\n---\n";
        let parsed = read_doc::<CaseFrontmatter>(text);
        assert!(parsed.is_ok());
        if let Ok((fm, _)) = parsed {
            assert_eq!(fm.status, "active");
        }
    }
}
