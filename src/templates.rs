use crate::config::TemplateLanguage;
use crate::error::{AppError, Result};
use include_dir::{Dir, include_dir};
use minijinja::Environment;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TemplateKey {
    WorkspaceAgents,
    WorkspaceDoNotEdit,
    NotesDefault,
    NotesMergeSection,
    DraftNew,
    DraftReply,
    MessageSection,
    TriageView,
    CaseDocument,
    CaseMessage,
    NotificationIndex,
    NotificationMessage,
    StatusIndex,
    StatusMessage,
}

impl TemplateKey {
    pub const ALL: [Self; 14] = [
        Self::WorkspaceAgents,
        Self::WorkspaceDoNotEdit,
        Self::NotesDefault,
        Self::NotesMergeSection,
        Self::DraftNew,
        Self::DraftReply,
        Self::MessageSection,
        Self::TriageView,
        Self::CaseDocument,
        Self::CaseMessage,
        Self::NotificationIndex,
        Self::NotificationMessage,
        Self::StatusIndex,
        Self::StatusMessage,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceAgents => "workspace/AGENTS.md.j2",
            Self::WorkspaceDoNotEdit => "workspace/DO_NOT_EDIT.txt.j2",
            Self::NotesDefault => "notes/default.md.j2",
            Self::NotesMergeSection => "notes/merge-section.md.j2",
            Self::DraftNew => "draft/new.md.j2",
            Self::DraftReply => "draft/reply.md.j2",
            Self::MessageSection => "message/section.md.j2",
            Self::TriageView => "triage/view.md.j2",
            Self::CaseDocument => "case/case.md.j2",
            Self::CaseMessage => "case/message.md.j2",
            Self::NotificationIndex => "notification/notification.md.j2",
            Self::NotificationMessage => "notification/message.md.j2",
            Self::StatusIndex => "status/index.md.j2",
            Self::StatusMessage => "status/message.md.j2",
        }
    }

    pub fn builtin_text(self, language: TemplateLanguage) -> &'static str {
        builtin_template_text(language, self)
    }
}

/// The whole `templates/` tree, embedded at compile time. Language selection is
/// data-driven: each `templates/<bcp47>/` directory is a language pack, looked up
/// by `TemplateLanguage::as_str()`. Adding a language is dropping a directory and
/// a `TemplateLanguage` variant — no per-language code lives here.
static BUILTIN_TEMPLATES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/templates");

fn lookup_builtin(language: &str, key: TemplateKey) -> Option<&'static str> {
    BUILTIN_TEMPLATES
        .get_file(format!("{language}/{}", key.as_str()))
        .and_then(|file| file.contents_utf8())
}

fn builtin_template_text(language: TemplateLanguage, key: TemplateKey) -> &'static str {
    // Fall back to en-US so a partial language pack (a key not yet translated)
    // degrades to English instead of an empty render. en-US is complete, so the
    // final `unwrap_or` is unreachable in practice; a test asserts every pack is
    // complete. Empty string keeps this panic-free for the clippy lint gate.
    lookup_builtin(language.as_str(), key)
        .or_else(|| lookup_builtin(TemplateLanguage::EnUs.as_str(), key))
        .unwrap_or("")
}

pub fn language_template_path(language: TemplateLanguage, key: TemplateKey) -> PathBuf {
    PathBuf::from(language.as_str()).join(key.as_str())
}

#[derive(Clone, Debug, Default)]
pub struct TemplateRenderStats {
    counts: BTreeMap<&'static str, TemplateSourceCounts>,
}

impl TemplateRenderStats {
    fn record(&mut self, key: TemplateKey, source: TemplateSourceKind) {
        let counts = self.counts.entry(key.as_str()).or_default();
        match source {
            TemplateSourceKind::Builtin => counts.builtin += 1,
            TemplateSourceKind::Workspace => counts.workspace += 1,
        }
    }

    pub fn to_value(&self) -> Value {
        let mut out = serde_json::Map::new();
        for key in TemplateKey::ALL {
            let counts = self.counts.get(key.as_str()).cloned().unwrap_or_default();
            out.insert(
                key.as_str().to_string(),
                json!({
                    "builtin": counts.builtin,
                    "workspace": counts.workspace,
                }),
            );
        }
        Value::Object(out)
    }
}

#[derive(Clone, Debug, Default)]
struct TemplateSourceCounts {
    builtin: usize,
    workspace: usize,
}

#[derive(Clone, Copy, Debug)]
enum TemplateSourceKind {
    Builtin,
    Workspace,
}

struct TemplateSource {
    text: String,
    kind: TemplateSourceKind,
    label: String,
}

pub struct MarkdownTemplateRenderer<'a> {
    root: Option<&'a Path>,
    language: TemplateLanguage,
    stats: TemplateRenderStats,
}

impl<'a> MarkdownTemplateRenderer<'a> {
    pub fn new(root: &'a Path, language: TemplateLanguage) -> Self {
        Self {
            root: Some(root),
            language,
            stats: TemplateRenderStats::default(),
        }
    }

    pub fn builtin(language: TemplateLanguage) -> Self {
        Self {
            root: None,
            language,
            stats: TemplateRenderStats::default(),
        }
    }

    pub fn render<T: Serialize>(&mut self, key: TemplateKey, context: &T) -> Result<String> {
        let source = self.template_source(key)?;
        let mut env = Environment::new();
        // SemiStrict: printing/iterating/accessing an undefined name is a hard
        // error (catches template typos like `{{ view.message_cont }}` instead
        // of silently rendering empty), but `{% if maybe_absent %}` still just
        // evaluates false, so intentionally-optional fields keep working.
        env.set_undefined_behavior(minijinja::UndefinedBehavior::SemiStrict);
        // Untrusted email-derived fields (subject, from/to/cc, sender, mailing
        // list, domains) are interpolated into generated views the agent reads.
        // `inline` collapses CR/LF to spaces so a crafted header cannot inject
        // new Markdown lines that impersonate afmail's own block-level labels
        // (Warnings:/Notes:/headings) outside the body fence.
        env.add_filter("inline", template_inline_filter);
        env.add_filter("plain", template_plain_filter);
        env.add_template(key.as_str(), &source.text)
            .map_err(|e| template_error(key, self.language, &source.label, e))?;
        let template = env
            .get_template(key.as_str())
            .map_err(|e| template_error(key, self.language, &source.label, e))?;
        let rendered = template
            .render(context)
            .map_err(|e| template_error(key, self.language, &source.label, e))?;
        self.stats.record(key, source.kind);
        Ok(rendered)
    }

    pub fn stats(&self) -> &TemplateRenderStats {
        &self.stats
    }

    fn template_source(&self, key: TemplateKey) -> Result<TemplateSource> {
        if let Some(root) = self.root {
            let rel = PathBuf::from("templates").join(language_template_path(self.language, key));
            let path = root.join(&rel);
            if path.exists() {
                let text =
                    fs::read_to_string(&path).map_err(|e| AppError::io("read template", &e))?;
                return Ok(TemplateSource {
                    text,
                    kind: TemplateSourceKind::Workspace,
                    label: path_to_string(&rel),
                });
            }
        }
        Ok(TemplateSource {
            text: key.builtin_text(self.language).to_string(),
            kind: TemplateSourceKind::Builtin,
            label: "builtin".to_string(),
        })
    }
}

fn template_error(
    key: TemplateKey,
    language: TemplateLanguage,
    source: &str,
    error: minijinja::Error,
) -> AppError {
    AppError::new(
        "template_render_failed",
        format!(
            "failed to render template {}/{} from {}: {}",
            language.as_str(),
            key.as_str(),
            source,
            error
        ),
    )
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// minijinja `inline` filter: collapse CR/LF to single spaces and trim, so an
/// untrusted header value stays on its own labeled line and cannot introduce
/// new Markdown block structure. Mirrors `store::render::markdown_inline`.
fn template_inline_filter(value: String) -> String {
    let inline = value.replace(['\r', '\n'], " ");
    let mut out = String::new();
    for ch in inline.trim().chars() {
        if matches!(
            ch,
            '\\' | '`' | '[' | ']' | '(' | ')' | '<' | '>' | '!' | '|'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// minijinja `plain` filter: keep an untrusted inline value readable while
/// preventing Markdown autolinks and explicit link syntax in generated views.
fn template_plain_filter(value: String) -> String {
    template_inline_filter(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_pack_ships_every_template_key() {
        // Each language pack must carry its OWN file for every key — the en-US
        // fallback in `builtin_template_text` is a safety net, not a license for
        // an incomplete pack. A missing file would silently render in English.
        for language in TemplateLanguage::ALL {
            for key in TemplateKey::ALL {
                assert!(
                    lookup_builtin(language.as_str(), key).is_some(),
                    "missing builtin template {}/{}",
                    language.as_str(),
                    key.as_str()
                );
            }
        }
    }

    #[test]
    fn workspace_agents_templates_do_not_hardcode_agent_install_target() {
        for language in TemplateLanguage::ALL {
            let text = TemplateKey::WorkspaceAgents.builtin_text(language);
            assert!(
                text.contains("`afmail skill install`"),
                "{} workspace AGENTS template should mention bare skill install",
                language.as_str()
            );
            assert!(
                !text.contains("--agent"),
                "{} workspace AGENTS template should not pin an agent",
                language.as_str()
            );
        }
    }

    #[test]
    fn generated_message_templates_only_use_sanitized_display_fields() {
        for language in TemplateLanguage::ALL {
            for key in [
                TemplateKey::TriageView,
                TemplateKey::CaseMessage,
                TemplateKey::NotificationMessage,
                TemplateKey::StatusMessage,
            ] {
                let text = key.builtin_text(language);
                for unsafe_field in ["view.title", "item.view.from", "item.view.subject"] {
                    assert!(
                        !text.contains(unsafe_field),
                        "{} {} reads unsafe field {unsafe_field}",
                        language.as_str(),
                        key.as_str()
                    );
                }
            }
        }
    }
}
