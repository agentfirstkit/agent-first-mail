use crate::config::TemplateLanguage;
use crate::error::{AppError, Result};
use minijinja::Environment;
use serde::Serialize;
use serde_json::{json, Value};
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
    ArchiveMessageIndex,
    ArchiveMessage,
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
        Self::ArchiveMessageIndex,
        Self::ArchiveMessage,
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
            Self::ArchiveMessageIndex => "archive-message/archive.md.j2",
            Self::ArchiveMessage => "archive-message/message.md.j2",
            Self::StatusIndex => "status/index.md.j2",
            Self::StatusMessage => "status/message.md.j2",
        }
    }

    pub fn builtin_text(self, language: TemplateLanguage) -> &'static str {
        match (language, self) {
            (TemplateLanguage::EnUs, Self::WorkspaceAgents) => {
                include_str!("../templates/en-US/workspace/AGENTS.md.j2")
            }
            (TemplateLanguage::EnUs, Self::WorkspaceDoNotEdit) => {
                include_str!("../templates/en-US/workspace/DO_NOT_EDIT.txt.j2")
            }
            (TemplateLanguage::EnUs, Self::NotesDefault) => {
                include_str!("../templates/en-US/notes/default.md.j2")
            }
            (TemplateLanguage::EnUs, Self::NotesMergeSection) => {
                include_str!("../templates/en-US/notes/merge-section.md.j2")
            }
            (TemplateLanguage::EnUs, Self::DraftNew) => {
                include_str!("../templates/en-US/draft/new.md.j2")
            }
            (TemplateLanguage::EnUs, Self::DraftReply) => {
                include_str!("../templates/en-US/draft/reply.md.j2")
            }
            (TemplateLanguage::EnUs, Self::MessageSection) => {
                include_str!("../templates/en-US/message/section.md.j2")
            }
            (TemplateLanguage::EnUs, Self::TriageView) => {
                include_str!("../templates/en-US/triage/view.md.j2")
            }
            (TemplateLanguage::EnUs, Self::CaseDocument) => {
                include_str!("../templates/en-US/case/case.md.j2")
            }
            (TemplateLanguage::EnUs, Self::CaseMessage) => {
                include_str!("../templates/en-US/case/message.md.j2")
            }
            (TemplateLanguage::EnUs, Self::ArchiveMessageIndex) => {
                include_str!("../templates/en-US/archive-message/archive.md.j2")
            }
            (TemplateLanguage::EnUs, Self::ArchiveMessage) => {
                include_str!("../templates/en-US/archive-message/message.md.j2")
            }
            (TemplateLanguage::EnUs, Self::StatusIndex) => {
                include_str!("../templates/en-US/status/index.md.j2")
            }
            (TemplateLanguage::EnUs, Self::StatusMessage) => {
                include_str!("../templates/en-US/status/message.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::WorkspaceAgents) => {
                include_str!("../templates/zh-CN/workspace/AGENTS.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::WorkspaceDoNotEdit) => {
                include_str!("../templates/zh-CN/workspace/DO_NOT_EDIT.txt.j2")
            }
            (TemplateLanguage::ZhCn, Self::NotesDefault) => {
                include_str!("../templates/zh-CN/notes/default.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::NotesMergeSection) => {
                include_str!("../templates/zh-CN/notes/merge-section.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::DraftNew) => {
                include_str!("../templates/zh-CN/draft/new.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::DraftReply) => {
                include_str!("../templates/zh-CN/draft/reply.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::MessageSection) => {
                include_str!("../templates/zh-CN/message/section.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::TriageView) => {
                include_str!("../templates/zh-CN/triage/view.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::CaseDocument) => {
                include_str!("../templates/zh-CN/case/case.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::CaseMessage) => {
                include_str!("../templates/zh-CN/case/message.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::ArchiveMessageIndex) => {
                include_str!("../templates/zh-CN/archive-message/archive.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::ArchiveMessage) => {
                include_str!("../templates/zh-CN/archive-message/message.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::StatusIndex) => {
                include_str!("../templates/zh-CN/status/index.md.j2")
            }
            (TemplateLanguage::ZhCn, Self::StatusMessage) => {
                include_str!("../templates/zh-CN/status/message.md.j2")
            }
        }
    }
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
            let rel = PathBuf::from(".afmail")
                .join("templates")
                .join(language_template_path(self.language, key));
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
