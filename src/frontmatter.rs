//! Typed metadata for canonical user-authored Markdown frontmatter.
//!
//! Draft Markdown uses `markdown::read_doc::<T>` and
//! `markdown::render_frontmatter`. Body content stays raw Markdown.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::types::DraftSyncIntent;

pub use crate::types::MessageCollection as CaseFrontmatter;

/// Deserialize a sequence that may be written as YAML `null` (a bare `key:`
/// with no value) into an empty `Vec`. Agents hand-write drafts and frequently
/// leave `attachments:` / `cc:` empty.
fn de_null_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

/// Frontmatter of an agent-authored `drafts/*.md`. Shared by draft validation
/// and outbound message building so both agree on one schema.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct DraftFrontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub case_uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_intent: Option<DraftSyncIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
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
    pub bcc: Vec<String>,
    #[serde(
        default,
        deserialize_with = "de_null_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub attachments: Vec<String>,
}

/// Optional `identities/<identity>.md` persona override.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct IdentityFileFrontmatter {
    pub kind: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{read_doc, render_frontmatter};

    #[test]
    fn draft_tolerates_empty_arrays() {
        let text = "---\nkind: draft\ncase_uid: c20260521001\nsubject: Hello\nto:\n  - a@example.com\ncc:\nbcc:\nattachments:\n---\nbody";
        let parsed = read_doc::<DraftFrontmatter>(text);
        assert!(parsed.is_ok(), "{parsed:?}");
        if let Ok((fm, body)) = parsed {
            assert_eq!(fm.case_uid, "c20260521001");
            assert_eq!(fm.subject.as_deref(), Some("Hello"));
            assert_eq!(fm.to, vec!["a@example.com".to_string()]);
            assert!(fm.cc.is_empty());
            assert!(fm.bcc.is_empty());
            assert!(fm.attachments.is_empty());
            assert_eq!(body.trim(), "body");
        }
    }

    #[test]
    fn draft_sync_intent_requires_the_canonical_one_time_upgrade_value() {
        let legacy = "---\nkind: draft\ncase_uid: c20260521001\nsync_intent: save\n---\nbody";
        assert!(read_doc::<DraftFrontmatter>(legacy).is_err());

        let canonical =
            "---\nkind: draft\ncase_uid: c20260521001\nsync_intent: save_draft\n---\nbody";
        let parsed = read_doc::<DraftFrontmatter>(canonical);
        assert!(parsed.is_ok());
        if let Ok((frontmatter, _)) = parsed {
            assert_eq!(frontmatter.sync_intent, Some(DraftSyncIntent::SaveDraft));
        }
    }

    #[test]
    fn case_round_trips_through_render() {
        let mut fm = CaseFrontmatter::new_case("c20260521001", "Acme", "2026-05-30T00:00:00Z");
        fm.tags = vec!["legal".to_string()];
        let body = "\n# Title\n\nhi\n";
        let rendered = render_frontmatter(&fm, body);
        assert!(rendered.is_ok());
        if let Ok(rendered) = rendered {
            let reparsed = read_doc::<CaseFrontmatter>(&rendered);
            assert!(reparsed.is_ok(), "{reparsed:?}");
            if let Ok((parsed, parsed_body)) = reparsed {
                assert_eq!(parsed, fm);
                assert_eq!(parsed_body, body.trim_start());
            }
        }
    }

    #[test]
    fn case_collection_uses_shared_schema() {
        let fm = CaseFrontmatter::new_case("c20260530001", "Bare", "2026-05-30T00:00:00Z");
        assert_eq!(fm.schema_name, crate::types::CASE_SCHEMA_NAME);
        assert!(fm.status.is_empty());
    }
}
