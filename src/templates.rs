use crate::config::TemplateLanguage;
use crate::error::{AppError, Result};
use agent_first_ui::{reject_frontend_script, reject_rendered_script};
use chrono::{DateTime, FixedOffset, SecondsFormat};
use include_dir::{Dir, include_dir};
use minijinja::value::Value as MiniValue;
use minijinja::{
    AutoEscape, Environment, Error as MiniError, ErrorKind as MiniErrorKind,
    default_auto_escape_callback,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TemplateKey {
    LocaleCatalog,
    WorkspaceDoNotEdit,
    DemoHandoff,
    ContactDefault,
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
    ReviewPage,
    ReviewMessage,
    ReviewDraftEditor,
    ReviewNotesEditor,
    ReviewStyle,
    ReviewAgentWorkbenchLayout,
    MarkdownMessageSectionLayout,
    MarkdownTriageViewLayout,
    MarkdownCaseDocumentLayout,
    MarkdownCaseMessageLayout,
    MarkdownNotificationIndexLayout,
    MarkdownNotificationMessageLayout,
    MarkdownStatusIndexLayout,
    MarkdownStatusMessageLayout,
    ReviewPageLayout,
    ReviewMessageLayout,
    ReviewDraftEditorLayout,
    ReviewNotesEditorLayout,
}

impl TemplateKey {
    pub const ALL: [Self; 34] = [
        Self::LocaleCatalog,
        Self::WorkspaceDoNotEdit,
        Self::DemoHandoff,
        Self::ContactDefault,
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
        Self::ReviewPage,
        Self::ReviewMessage,
        Self::ReviewDraftEditor,
        Self::ReviewNotesEditor,
        Self::ReviewStyle,
        Self::ReviewAgentWorkbenchLayout,
        Self::MarkdownMessageSectionLayout,
        Self::MarkdownTriageViewLayout,
        Self::MarkdownCaseDocumentLayout,
        Self::MarkdownCaseMessageLayout,
        Self::MarkdownNotificationIndexLayout,
        Self::MarkdownNotificationMessageLayout,
        Self::MarkdownStatusIndexLayout,
        Self::MarkdownStatusMessageLayout,
        Self::ReviewPageLayout,
        Self::ReviewMessageLayout,
        Self::ReviewDraftEditorLayout,
        Self::ReviewNotesEditorLayout,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocaleCatalog => "i18n/catalog.j2",
            Self::WorkspaceDoNotEdit => "workspace/DO_NOT_EDIT.txt.j2",
            Self::DemoHandoff => "demo/handoff.txt.j2",
            Self::ContactDefault => "contact/default.md.j2",
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
            Self::ReviewPage => "review/page.html.j2",
            Self::ReviewMessage => "review/message.html.j2",
            Self::ReviewDraftEditor => "review/draft-editor.html.j2",
            Self::ReviewNotesEditor => "review/notes-editor.html.j2",
            Self::ReviewStyle => "review/style.css.j2",
            Self::ReviewAgentWorkbenchLayout => "review/agent-workbench-layout.html.j2",
            Self::MarkdownMessageSectionLayout => "markdown/message-section.md.j2",
            Self::MarkdownTriageViewLayout => "markdown/triage-view.md.j2",
            Self::MarkdownCaseDocumentLayout => "markdown/case-document.md.j2",
            Self::MarkdownCaseMessageLayout => "markdown/case-message.md.j2",
            Self::MarkdownNotificationIndexLayout => "markdown/notification-index.md.j2",
            Self::MarkdownNotificationMessageLayout => "markdown/notification-message.md.j2",
            Self::MarkdownStatusIndexLayout => "markdown/status-index.md.j2",
            Self::MarkdownStatusMessageLayout => "markdown/status-message.md.j2",
            Self::ReviewPageLayout => "review/page-layout.html.j2",
            Self::ReviewMessageLayout => "review/message-layout.html.j2",
            Self::ReviewDraftEditorLayout => "review/draft-editor-layout.html.j2",
            Self::ReviewNotesEditorLayout => "review/notes-editor-layout.html.j2",
        }
    }

    pub fn builtin_text(self, language: TemplateLanguage) -> &'static str {
        builtin_template_text(language, self)
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.as_str() == name)
    }

    pub fn is_language_scoped(self) -> bool {
        !matches!(
            self,
            Self::ReviewStyle
                | Self::ReviewAgentWorkbenchLayout
                | Self::MarkdownMessageSectionLayout
                | Self::MarkdownTriageViewLayout
                | Self::MarkdownCaseDocumentLayout
                | Self::MarkdownCaseMessageLayout
                | Self::MarkdownNotificationIndexLayout
                | Self::MarkdownNotificationMessageLayout
                | Self::MarkdownStatusIndexLayout
                | Self::MarkdownStatusMessageLayout
                | Self::ReviewPageLayout
                | Self::ReviewMessageLayout
                | Self::ReviewDraftEditorLayout
                | Self::ReviewNotesEditorLayout
        )
    }

    fn is_review(self) -> bool {
        self.is_review_ui() || matches!(self, Self::LocaleCatalog)
    }

    /// The templates that are the Review UI itself.
    ///
    /// These are the only templates a user replaces through an AFUI frontend
    /// (`.afui/frontends/afmail/<ui_kind>/templates/`). Everything else is
    /// content — mail, drafts, notes, generated Markdown — and stays in the
    /// workspace `templates/` tree. The split is the point: each template has
    /// exactly one place it can be overridden from, so there is never a second
    /// entry point competing to change the same page.
    pub fn is_review_ui(self) -> bool {
        matches!(
            self,
            Self::ReviewPage
                | Self::ReviewMessage
                | Self::ReviewDraftEditor
                | Self::ReviewNotesEditor
                | Self::ReviewStyle
                | Self::ReviewAgentWorkbenchLayout
                | Self::ReviewPageLayout
                | Self::ReviewMessageLayout
                | Self::ReviewDraftEditorLayout
                | Self::ReviewNotesEditorLayout
        )
    }

    fn uses_render_document_context(self) -> bool {
        matches!(
            self,
            Self::WorkspaceDoNotEdit
                | Self::ContactDefault
                | Self::NotesDefault
                | Self::NotesMergeSection
                | Self::DraftNew
                | Self::DraftReply
                | Self::MessageSection
                | Self::TriageView
                | Self::CaseDocument
                | Self::CaseMessage
                | Self::NotificationIndex
                | Self::NotificationMessage
                | Self::StatusIndex
                | Self::StatusMessage
                | Self::MarkdownMessageSectionLayout
                | Self::MarkdownTriageViewLayout
                | Self::MarkdownCaseDocumentLayout
                | Self::MarkdownCaseMessageLayout
                | Self::MarkdownNotificationIndexLayout
                | Self::MarkdownNotificationMessageLayout
                | Self::MarkdownStatusIndexLayout
                | Self::MarkdownStatusMessageLayout
        )
    }

    pub(crate) fn required_templates(self) -> Vec<Self> {
        match self {
            Self::ReviewPage => vec![
                Self::ReviewPage,
                Self::LocaleCatalog,
                Self::ReviewPageLayout,
                Self::ReviewMessage,
                Self::ReviewMessageLayout,
                Self::ReviewDraftEditor,
                Self::ReviewDraftEditorLayout,
                Self::ReviewNotesEditor,
                Self::ReviewNotesEditorLayout,
                Self::ReviewStyle,
                Self::ReviewAgentWorkbenchLayout,
            ],
            Self::ReviewMessage => vec![
                Self::ReviewMessage,
                Self::LocaleCatalog,
                Self::ReviewMessageLayout,
            ],
            Self::ReviewDraftEditor => vec![
                Self::ReviewDraftEditor,
                Self::LocaleCatalog,
                Self::ReviewDraftEditorLayout,
            ],
            Self::ReviewNotesEditor => vec![
                Self::ReviewNotesEditor,
                Self::LocaleCatalog,
                Self::ReviewNotesEditorLayout,
            ],
            Self::MessageSection => vec![
                Self::MessageSection,
                Self::LocaleCatalog,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::TriageView => vec![
                Self::TriageView,
                Self::LocaleCatalog,
                Self::MarkdownTriageViewLayout,
                Self::MessageSection,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::CaseDocument => vec![
                Self::CaseDocument,
                Self::LocaleCatalog,
                Self::MarkdownCaseDocumentLayout,
                Self::MessageSection,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::CaseMessage => vec![
                Self::CaseMessage,
                Self::LocaleCatalog,
                Self::MarkdownCaseMessageLayout,
                Self::MessageSection,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::NotificationMessage => vec![
                Self::NotificationMessage,
                Self::LocaleCatalog,
                Self::MarkdownNotificationMessageLayout,
                Self::MessageSection,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::StatusMessage => vec![
                Self::StatusMessage,
                Self::LocaleCatalog,
                Self::MarkdownStatusMessageLayout,
                Self::MessageSection,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::NotificationIndex => vec![
                Self::NotificationIndex,
                Self::LocaleCatalog,
                Self::MarkdownNotificationIndexLayout,
            ],
            Self::StatusIndex => vec![
                Self::StatusIndex,
                Self::LocaleCatalog,
                Self::MarkdownStatusIndexLayout,
            ],
            Self::MarkdownTriageViewLayout
            | Self::MarkdownCaseDocumentLayout
            | Self::MarkdownCaseMessageLayout
            | Self::MarkdownNotificationMessageLayout
            | Self::MarkdownStatusMessageLayout => vec![
                self,
                Self::LocaleCatalog,
                Self::MessageSection,
                Self::MarkdownMessageSectionLayout,
            ],
            Self::ReviewPageLayout => vec![
                Self::ReviewPageLayout,
                Self::LocaleCatalog,
                Self::ReviewMessage,
                Self::ReviewMessageLayout,
                Self::ReviewDraftEditor,
                Self::ReviewDraftEditorLayout,
                Self::ReviewNotesEditor,
                Self::ReviewNotesEditorLayout,
                Self::ReviewStyle,
                Self::ReviewAgentWorkbenchLayout,
            ],
            Self::MarkdownMessageSectionLayout
            | Self::MarkdownNotificationIndexLayout
            | Self::MarkdownStatusIndexLayout
            | Self::ReviewMessageLayout
            | Self::ReviewDraftEditorLayout
            | Self::ReviewNotesEditorLayout
            | Self::ReviewAgentWorkbenchLayout => vec![self, Self::LocaleCatalog],
            _ => vec![self],
        }
    }
}

/// The whole `templates/` tree, embedded at compile time. Language-specific
/// templates live in `templates/<bcp47>/`; non-linguistic shared assets live
/// in `templates/shared/` and are exported once.
static BUILTIN_TEMPLATES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/templates");

fn lookup_builtin(language: &str, key: TemplateKey) -> Option<&'static str> {
    let path = if key.is_language_scoped() {
        format!("{language}/{}", key.as_str())
    } else {
        format!("shared/{}", key.as_str())
    };
    BUILTIN_TEMPLATES
        .get_file(path)
        .and_then(|file| file.contents_utf8())
}

fn builtin_template_text(language: TemplateLanguage, key: TemplateKey) -> &'static str {
    lookup_builtin(language.as_str(), key)
        .or_else(|| lookup_builtin(TemplateLanguage::EnUs.as_str(), key))
        .unwrap_or("")
}

pub fn language_template_path(language: TemplateLanguage, key: TemplateKey) -> PathBuf {
    if key.is_language_scoped() {
        PathBuf::from(language.as_str()).join(key.as_str())
    } else {
        PathBuf::from("shared").join(key.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TemplateRenderOptions<'a> {
    pub timezone_offset: Option<FixedOffset>,
    pub output_dir: Option<&'a Path>,
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
            TemplateSourceKind::Frontend => counts.frontend += 1,
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
                    "frontend": counts.frontend,
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
    frontend: usize,
    workspace: usize,
}

#[derive(Clone, Copy, Debug)]
enum TemplateSourceKind {
    Builtin,
    Frontend,
    Workspace,
}

struct TemplateSource {
    text: String,
    kind: TemplateSourceKind,
    label: String,
}

/// Which override tree a renderer reads, and therefore which templates it is
/// allowed to read from disk at all.
///
/// Content templates come from the workspace `templates/` tree; the Review UI
/// comes from the AFUI frontend a user installed for that surface. Encoding
/// that here is what stops the two from becoming two ways to change one page:
/// a workspace-rooted renderer cannot reach a Review UI template, and a
/// frontend-rooted one cannot reach a content template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateTree {
    Workspace,
    Frontend,
}

impl TemplateTree {
    fn serves(self, key: TemplateKey) -> bool {
        match self {
            Self::Workspace => !key.is_review_ui(),
            // The locale catalog is copy rather than layout, and a Review UI
            // renders without the workspace tree in reach, so a frontend that
            // rewords itself overrides the catalog in its own directory.
            Self::Frontend => key.is_review_ui() || matches!(key, TemplateKey::LocaleCatalog),
        }
    }

    fn source_kind(self) -> TemplateSourceKind {
        match self {
            Self::Workspace => TemplateSourceKind::Workspace,
            Self::Frontend => TemplateSourceKind::Frontend,
        }
    }
}

pub struct TemplateRenderer<'a> {
    root: Option<(&'a Path, TemplateTree)>,
    language: TemplateLanguage,
    stats: TemplateRenderStats,
}

impl<'a> TemplateRenderer<'a> {
    pub fn new(root: &'a Path, language: TemplateLanguage) -> Self {
        Self {
            root: Some((root, TemplateTree::Workspace)),
            language,
            stats: TemplateRenderStats::default(),
        }
    }

    /// Render from a user's own Review UI, rooted at the frontend directory
    /// AFUI resolved and trusted.
    pub fn frontend(root: &'a Path, language: TemplateLanguage) -> Self {
        Self {
            root: Some((root, TemplateTree::Frontend)),
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
        self.render_with_options(key, context, TemplateRenderOptions::default())
    }

    pub fn render_with_options<T: Serialize>(
        &mut self,
        key: TemplateKey,
        context: &T,
        options: TemplateRenderOptions<'_>,
    ) -> Result<String> {
        let sources = key
            .required_templates()
            .into_iter()
            .map(|required| {
                self.template_source(required)
                    .map(|source| (required, source))
            })
            .collect::<Result<Vec<_>>>()?;
        let entry_source = sources
            .iter()
            .find(|(source_key, _)| *source_key == key)
            .map(|(_, source)| source)
            .ok_or_else(|| AppError::new("internal", "template entry source was not loaded"))?;

        let mut env = Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::SemiStrict);
        if key.is_review() {
            env.set_auto_escape_callback(|_| AutoEscape::Html);
            env.remove_filter("safe");
        } else {
            env.set_auto_escape_callback(default_auto_escape_callback);
        }
        add_template_filters(&mut env, self.root.map(|(root, _)| root), options);
        add_template_loader(&mut env, self.root, self.language, key.is_review());
        for (source_key, source) in &sources {
            env.add_template_owned(source_key.as_str().to_string(), source.text.clone())
                .map_err(|error| {
                    template_error(*source_key, self.language, &source.label, error)
                })?;
        }
        let template = env
            .get_template(key.as_str())
            .map_err(|error| template_error(key, self.language, &entry_source.label, error))?;
        let json = serde_json::to_value(context)
            .map_err(|error| AppError::json("build template context", &error))?;
        let json = if key.uses_render_document_context() {
            json!({"document": json})
        } else {
            json
        };
        let context = json_value_to_minijinja(&json);
        let rendered = template
            .render(context)
            .map_err(|error| template_error(key, self.language, &entry_source.label, error))?;
        if key.is_review() {
            validate_rendered_review_html("rendered Review output", &rendered)?;
        }
        for (source_key, source) in &sources {
            self.stats.record(*source_key, source.kind);
        }
        Ok(rendered)
    }

    pub fn stats(&self) -> &TemplateRenderStats {
        &self.stats
    }

    fn template_source(&self, key: TemplateKey) -> Result<TemplateSource> {
        if let Some((root, tree)) = self.root
            && tree.serves(key)
        {
            let rel = PathBuf::from("templates").join(language_template_path(self.language, key));
            let path = root.join(&rel);
            ensure_template_path_safe(&root.join("templates"), &path)?;
            if path.exists() {
                let text = fs::read_to_string(&path)
                    .map_err(|error| AppError::io("read template", &error))?;
                if key.is_review() {
                    validate_review_template_text(key.as_str(), &text)?;
                }
                return Ok(TemplateSource {
                    text,
                    kind: tree.source_kind(),
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

fn add_template_filters(
    env: &mut Environment<'_>,
    root: Option<&Path>,
    options: TemplateRenderOptions<'_>,
) {
    env.add_filter("inline", template_inline_filter);
    env.add_filter("plain", template_inline_filter);
    env.add_filter("markdown_code", template_markdown_code_filter);
    env.add_filter("markdown_alt", template_markdown_alt_filter);
    env.add_filter("markdown_fence", template_markdown_fence_filter);
    env.add_filter("markdown_table", template_markdown_table_filter);
    env.add_filter("markdown_quote", template_markdown_quote_filter);
    env.add_filter("yaml", template_yaml_filter);
    env.add_filter("is_image", template_is_image_filter);
    env.add_filter("is_safe_review_image", template_is_safe_review_image_filter);
    env.add_filter("bytes", template_bytes_filter);
    env.add_filter("display_name", template_display_name_filter);
    env.add_filter("initials", template_initials_filter);
    env.add_filter("message_item", template_message_item_filter);
    env.add_filter("message_status_key", template_message_status_key_filter);
    env.add_filter("text_range", template_text_range_filter);
    env.add_filter("chronological", template_chronological_filter);
    env.add_filter(
        "notification_date_desc",
        template_notification_date_desc_filter,
    );

    let timezone_offset = options.timezone_offset;
    env.add_filter("datetime", move |value: String| {
        template_datetime_filter(&value, timezone_offset)
    });

    let root = root.map(Path::to_path_buf);
    let output_dir = options.output_dir.map(Path::to_path_buf);
    env.add_filter("asset_path", move |value: String| {
        template_asset_path_filter(root.as_deref(), output_dir.as_deref(), &value)
    });
}

/// Register the same value-only filters on AFUI's restricted page
/// environment.  The registrar deliberately exposes no loader or escaping
/// controls; Review template sources and policy stay owned by `UiPage`.
#[cfg(feature = "review")]
pub(crate) fn add_review_page_filters(
    env: &mut agent_first_ui::UiPageEnvironment<'_>,
    root: Option<&Path>,
    options: TemplateRenderOptions<'_>,
) -> agent_first_ui::Result<()> {
    env.add_filter("inline", template_inline_filter)?;
    env.add_filter("plain", template_inline_filter)?;
    env.add_filter("markdown_code", template_markdown_code_filter)?;
    env.add_filter("markdown_alt", template_markdown_alt_filter)?;
    env.add_filter("markdown_fence", template_markdown_fence_filter)?;
    env.add_filter("markdown_table", template_markdown_table_filter)?;
    env.add_filter("markdown_quote", template_markdown_quote_filter)?;
    env.add_filter("yaml", template_yaml_filter)?;
    env.add_filter("is_image", template_is_image_filter)?;
    env.add_filter("is_safe_review_image", template_is_safe_review_image_filter)?;
    env.add_filter("bytes", template_bytes_filter)?;
    env.add_filter("display_name", template_display_name_filter)?;
    env.add_filter("initials", template_initials_filter)?;
    env.add_filter("message_item", review_message_item_filter)?;
    env.add_filter("message_status_key", template_message_status_key_filter)?;
    env.add_filter("text_range", template_text_range_filter)?;
    env.add_filter("chronological", review_chronological_filter)?;
    env.add_filter(
        "notification_date_desc",
        review_notification_date_desc_filter,
    )?;

    let timezone_offset = options.timezone_offset;
    env.add_filter("datetime", move |value: String| {
        template_datetime_filter(&value, timezone_offset)
    })?;

    let root = root.map(Path::to_path_buf);
    let output_dir = options.output_dir.map(Path::to_path_buf);
    env.add_filter("asset_path", move |value: String| {
        template_asset_path_filter(root.as_deref(), output_dir.as_deref(), &value)
    })?;
    Ok(())
}

#[cfg(feature = "review")]
fn review_message_item_filter(
    items: MiniValue,
    message_id: String,
) -> std::result::Result<agent_first_ui::UiPageSerializedValue, MiniError> {
    let value = template_message_item_filter(items, message_id);
    if value.is_undefined() {
        Ok(agent_first_ui::UiPageSerializedValue::undefined())
    } else {
        agent_first_ui::UiPageSerializedValue::from_serialize(value)
    }
}

#[cfg(feature = "review")]
fn review_chronological_filter(
    items: MiniValue,
) -> std::result::Result<agent_first_ui::UiPageSerializedValue, MiniError> {
    agent_first_ui::UiPageSerializedValue::from_serialize(template_chronological_filter(items))
}

#[cfg(feature = "review")]
fn review_notification_date_desc_filter(
    documents: MiniValue,
    memberships: MiniValue,
) -> std::result::Result<agent_first_ui::UiPageSerializedValue, MiniError> {
    agent_first_ui::UiPageSerializedValue::from_serialize(template_notification_date_desc_filter(
        documents,
        memberships,
    ))
}

fn add_template_loader(
    env: &mut Environment<'_>,
    root: Option<(&Path, TemplateTree)>,
    language: TemplateLanguage,
    review_render: bool,
) {
    let Some((templates_root, tree)) = root.map(|(root, tree)| (root.join("templates"), tree))
    else {
        return;
    };
    env.set_loader(move |name| {
        let relative = safe_template_name(name).ok_or_else(|| {
            MiniError::new(
                MiniErrorKind::InvalidOperation,
                format!("template include path is not safe: {name}"),
            )
        })?;
        if let Some(key) = TemplateKey::from_name(name) {
            // The same split as `template_source`: an include cannot reach a
            // template this tree is not allowed to serve, or a workspace could
            // change the Review UI through a partial after being told it could
            // not change it directly.
            if !tree.serves(key) {
                return Ok(Some(key.builtin_text(language).to_string()));
            }
            let path = templates_root.join(language_template_path(language, key));
            ensure_template_path_safe(&templates_root, &path).map_err(template_loader_error)?;
            if path.is_file() {
                let text = fs::read_to_string(&path).map_err(|error| {
                    MiniError::new(
                        MiniErrorKind::InvalidOperation,
                        format!("could not read template include {name}"),
                    )
                    .with_source(error)
                })?;
                if review_render || key.is_review() {
                    validate_review_template_text(name, &text).map_err(template_loader_error)?;
                }
                return Ok(Some(text));
            }
            return Ok(Some(key.builtin_text(language).to_string()));
        }

        let paths = if let Ok(shared) = relative.strip_prefix("shared") {
            vec![templates_root.join("shared").join(shared)]
        } else {
            vec![
                templates_root.join(language.as_str()).join(&relative),
                templates_root.join("shared").join(&relative),
            ]
        };
        for path in paths {
            ensure_template_path_safe(&templates_root, &path).map_err(template_loader_error)?;
            if !path.is_file() {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|error| {
                MiniError::new(
                    MiniErrorKind::InvalidOperation,
                    format!("could not read template include {name}"),
                )
                .with_source(error)
            })?;
            if review_render {
                validate_review_template_text(name, &text).map_err(template_loader_error)?;
            }
            return Ok(Some(text));
        }
        Ok(None)
    });
}

fn safe_template_name(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('\\') {
        return None;
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

pub(crate) fn ensure_template_path_safe(templates_root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(templates_root).map_err(|_| {
        AppError::new(
            "template_path_unsafe",
            format!(
                "template path {} is outside {}",
                path.display(),
                templates_root.display()
            ),
        )
    })?;
    let mut current = templates_root.to_path_buf();
    reject_template_symlink(&current)?;
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(AppError::new(
                "template_path_unsafe",
                format!("template path {} is not safe", path.display()),
            ));
        };
        current.push(component);
        reject_template_symlink(&current)?;
    }
    Ok(())
}

fn reject_template_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::new(
            "template_path_unsafe",
            format!("template path {} is a symlink", path.display()),
        )
        .with_hint("Use regular files and directories inside templates/; symlinks are rejected.")),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io("inspect template path", &error)),
    }
}

fn template_loader_error(error: AppError) -> MiniError {
    MiniError::new(MiniErrorKind::InvalidOperation, error.message)
}

/// Refuse a Review template that could run code.
///
/// The rule lives in AFUI, not here. afmail wrote it first, but afpsql and
/// afterminal need the identical rule for the identical reason — each one binds
/// its own behaviour to its own page under a nonce, which is worth nothing if
/// the page can ship code beside it — and a security check with four
/// implementations is a security check that drifts. This is the call, and the
/// hint afmail attaches to it is the only part that is afmail's.
///
/// It covers markup (`<script>`, `<iframe>`, an inline `onclick=`, a
/// `javascript:` URL through the character references that hide one) and the
/// MiniJinja directives that switch escaping off, which is the other way a
/// template turns a message body into markup.
fn validate_review_template_text(label: &str, text: &str) -> Result<()> {
    reject_frontend_script(label, text).map_err(unsafe_review_template_error)
}

/// The same guard on what the template produced, deliberately narrower.
///
/// A rendered Review page carries subject lines and message bodies, so
/// `data:` inside one of those is text a person is meant to read rather than a
/// URL a browser will fetch. AFUI draws that line; afmail only says where.
fn validate_rendered_review_html(label: &str, text: &str) -> Result<()> {
    reject_rendered_script(label, text).map_err(unsafe_review_template_error)
}

pub(crate) fn unsafe_review_template_error(error: agent_first_ui::Error) -> AppError {
    AppError::new("review_template_unsafe", error.to_string()).with_hint(
        "Keep Review templates to MiniJinja, HTML, and CSS. afmail supplies the behavior-only JavaScript runtime.",
    )
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
            "failed to render template {}/{} from {}: {error:#?}",
            language.as_str(),
            key.as_str(),
            source
        ),
    )
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A JSON document as MiniJinja values.
///
/// Numbers are the reason this exists: afdata turns on serde_json's
/// `arbitrary_precision` for the whole build, so a `serde_json::Number` handed
/// straight to a template arrives as a map with an internal key inside it, and
/// anything that indexes or formats it fails. A document that was typed all
/// the way down never meets this; one that passed through `serde_json::Value`
/// on the way — a smoke fixture, an already-serialized envelope — always does.
pub(crate) fn json_value_to_minijinja(value: &Value) -> MiniValue {
    match value {
        Value::Null => MiniValue::from(()),
        Value::Bool(value) => MiniValue::from(*value),
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                MiniValue::from(value)
            } else if let Some(value) = number.as_i64() {
                MiniValue::from(value)
            } else if let Some(value) = number.as_f64() {
                MiniValue::from(value)
            } else {
                MiniValue::from(number.to_string())
            }
        }
        Value::String(value) => MiniValue::from(value.as_str()),
        Value::Array(items) => items.iter().map(json_value_to_minijinja).collect(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.as_str(), json_value_to_minijinja(value)))
            .collect(),
    }
}

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

fn template_markdown_code_filter(value: String) -> String {
    value.replace(['\r', '\n'], " ").trim().replace('`', "'")
}

fn template_markdown_alt_filter(value: String) -> String {
    template_inline_filter(value)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn template_markdown_fence_filter(value: String) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in value.chars() {
        if ch == '`' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(max_run.max(2) + 1)
}

fn template_markdown_table_filter(value: String) -> String {
    value
        .replace(['\r', '\n'], " ")
        .replace('|', "\\|")
        .trim()
        .to_string()
}

fn template_markdown_quote_filter(value: String) -> String {
    value
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn template_yaml_filter(value: String) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push(' '),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn template_datetime_filter(value: &str, offset: Option<FixedOffset>) -> String {
    let Some(offset) = offset else {
        return value.to_string();
    };
    DateTime::parse_from_rfc3339(value)
        .map(|datetime| {
            datetime
                .with_timezone(&offset)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        })
        .unwrap_or_else(|_| value.to_string())
}

fn template_asset_path_filter(
    root: Option<&Path>,
    output_dir: Option<&Path>,
    file_path: &str,
) -> String {
    let (Some(root), Some(output_dir)) = (root, output_dir) else {
        return file_path.to_string();
    };
    let Ok(from) = output_dir.strip_prefix(root) else {
        return file_path.to_string();
    };
    let up_count = from
        .components()
        .filter(|component| matches!(component, std::path::Component::Normal(_)))
        .count();
    let mut parts = Vec::new();
    parts.extend(std::iter::repeat_n("..", up_count));
    parts.extend(file_path.split('/').filter(|part| !part.is_empty()));
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn template_is_image_filter(content_type: String) -> bool {
    content_type
        .split_once(';')
        .map(|(mime, _)| mime)
        .unwrap_or(&content_type)
        .trim()
        .to_ascii_lowercase()
        .starts_with("image/")
}

fn template_is_safe_review_image_filter(content_type: String) -> bool {
    safe_review_image_content_type(&content_type).is_some()
}

pub(crate) fn safe_review_image_content_type(content_type: &str) -> Option<&'static str> {
    let content_type = content_type
        .split_once(';')
        .map(|(mime, _)| mime)
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match content_type.as_str() {
        "image/avif" => Some("image/avif"),
        "image/bmp" => Some("image/bmp"),
        "image/gif" => Some("image/gif"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/webp" => Some("image/webp"),
        "image/vnd.microsoft.icon" | "image/x-icon" => Some("image/x-icon"),
        _ => None,
    }
}

fn template_bytes_filter(value: u64) -> String {
    if value < 1024 {
        format!("{value} B")
    } else if value < 1024 * 1024 {
        format!("{} KiB", value.div_ceil(1024))
    } else {
        format!("{:.1} MiB", value as f64 / (1024.0 * 1024.0))
    }
}

fn template_display_name_filter(value: String) -> String {
    let value = value.trim();
    if let Some((name, _)) = value.split_once('<') {
        let name = name.trim().trim_matches('"');
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Some((local, _)) = value.split_once('@')
        && !local.is_empty()
    {
        return local.to_string();
    }
    value.to_string()
}

fn template_initials_filter(value: String) -> String {
    let name = template_display_name_filter(value);
    let initials = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>();
    if initials.is_empty() {
        "?".to_string()
    } else {
        initials
    }
}

fn template_message_item_filter(items: MiniValue, message_id: String) -> MiniValue {
    let Ok(iter) = items.try_iter() else {
        return MiniValue::UNDEFINED;
    };
    for item in iter {
        let Ok(value) = item.get_attr("message_id") else {
            continue;
        };
        if value.as_str() == Some(message_id.as_str()) {
            return item;
        }
    }
    MiniValue::UNDEFINED
}

fn template_message_status_key_filter(status: String) -> String {
    match status.as_str() {
        "trashed" => "trash".to_string(),
        "deleted_remote" => "remote_deleted".to_string(),
        other => other.to_string(),
    }
}

fn template_text_range_filter(
    value: String,
    start_bytes: usize,
    end_bytes: usize,
) -> std::result::Result<String, MiniError> {
    value
        .get(start_bytes..end_bytes)
        .map(ToString::to_string)
        .ok_or_else(|| {
            MiniError::new(
                MiniErrorKind::InvalidOperation,
                format!(
                    "text_range requires a valid UTF-8 byte range; got {start_bytes}..{end_bytes} for {} bytes",
                    value.len()
                ),
            )
        })
}

fn template_chronological_filter(items: MiniValue) -> Vec<MiniValue> {
    let Ok(iter) = items.try_iter() else {
        return Vec::new();
    };
    let mut items = iter.collect::<Vec<_>>();
    items.sort_by(|left, right| {
        template_message_time(left)
            .cmp(&template_message_time(right))
            .then_with(|| template_message_id(left).cmp(&template_message_id(right)))
    });
    items
}

fn template_notification_date_desc_filter(
    documents: MiniValue,
    memberships: MiniValue,
) -> Vec<MiniValue> {
    let membership_times = memberships
        .try_iter()
        .map(|items| {
            items
                .filter_map(|item| {
                    let message_id = item
                        .get_attr("message_id")
                        .ok()?
                        .as_str()
                        .map(ToString::to_string)?;
                    let added_rfc3339 = item
                        .get_attr("added_rfc3339")
                        .ok()?
                        .as_str()
                        .map(ToString::to_string)?;
                    let time = DateTime::parse_from_rfc3339(&added_rfc3339).ok()?;
                    Some((message_id, time))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let Ok(iter) = documents.try_iter() else {
        return Vec::new();
    };
    let mut documents = iter.collect::<Vec<_>>();
    documents.sort_by(|left, right| {
        let time = |document: &MiniValue| {
            template_message_time(document).or_else(|| {
                membership_times
                    .get(&template_message_id(document))
                    .cloned()
            })
        };
        time(right)
            .cmp(&time(left))
            .then_with(|| template_message_id(left).cmp(&template_message_id(right)))
    });
    documents
}

fn template_message_time(document: &MiniValue) -> Option<DateTime<FixedOffset>> {
    let analysis = document.get_attr("analysis").ok()?;
    let message = document.get_attr("message").ok()?;
    let direction = analysis.get_attr("direction").ok()?;
    let primary = if direction.as_str() == Some("outbound") {
        "sent_rfc3339"
    } else {
        "received_rfc3339"
    };
    let fallback = if primary == "sent_rfc3339" {
        "received_rfc3339"
    } else {
        "sent_rfc3339"
    };
    [primary, fallback].into_iter().find_map(|field| {
        message
            .get_attr(field)
            .ok()
            .and_then(|value| value.as_str().map(ToString::to_string))
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
    })
}

fn template_message_id(document: &MiniValue) -> String {
    document
        .get_attr("message")
        .ok()
        .and_then(|message| message.get_attr("message_id").ok())
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_template_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("afmail-template-{name}-{stamp}"))
    }

    fn catalog_keys(text: &str) -> BTreeSet<&str> {
        text.lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix('"')
                    .and_then(|line| line.split_once("\":"))
                    .map(|(key, _)| key)
            })
            .collect()
    }

    #[test]
    fn every_language_pack_ships_every_language_scoped_template() {
        for language in TemplateLanguage::ALL {
            for key in TemplateKey::ALL
                .into_iter()
                .filter(|key| key.is_language_scoped())
            {
                assert!(
                    lookup_builtin(language.as_str(), key).is_some(),
                    "missing builtin template {}/{}",
                    language.as_str(),
                    key.as_str()
                );
            }
        }
        for key in TemplateKey::ALL
            .into_iter()
            .filter(|key| !key.is_language_scoped())
        {
            assert!(
                lookup_builtin(TemplateLanguage::EnUs.as_str(), key).is_some(),
                "missing shared builtin template {}",
                key.as_str()
            );
        }
    }

    #[test]
    fn every_language_catalog_has_the_same_copy_keys_and_macros() {
        let expected =
            catalog_keys(TemplateKey::LocaleCatalog.builtin_text(TemplateLanguage::EnUs));
        assert!(!expected.is_empty());
        for language in TemplateLanguage::ALL {
            let catalog = TemplateKey::LocaleCatalog.builtin_text(language);
            assert_eq!(
                catalog_keys(catalog),
                expected,
                "{} locale catalog keys differ from en-US",
                language.as_str()
            );
            assert!(
                catalog.contains("macro collection_ref"),
                "{} locale catalog omitted collection_ref",
                language.as_str()
            );
            assert!(
                catalog.contains("macro message_count"),
                "{} locale catalog omitted message_count",
                language.as_str()
            );
        }
    }

    #[test]
    fn generated_templates_do_not_read_legacy_presentation_projections() {
        for language in TemplateLanguage::ALL {
            for key in TemplateKey::ALL {
                let text = key.builtin_text(language);
                for forbidden in [
                    " view.",
                    "display_time",
                    "display_title",
                    "display_contact",
                    "direction_symbol",
                    "body_text_block",
                    "body_text_fence",
                    "preview_path",
                ] {
                    assert!(
                        !text.contains(forbidden),
                        "{} {} reads legacy presentation field {forbidden}",
                        language.as_str(),
                        key.as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn byte_filter_uses_binary_unit_names() {
        assert_eq!(template_bytes_filter(1023), "1023 B");
        assert_eq!(template_bytes_filter(1024), "1 KiB");
        assert_eq!(template_bytes_filter(1025), "2 KiB");
        assert_eq!(template_bytes_filter(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn review_image_filter_allows_raster_formats_but_not_active_content() {
        assert_eq!(
            safe_review_image_content_type("IMAGE/JPEG; name=photo.jpg"),
            Some("image/jpeg")
        );
        assert_eq!(
            safe_review_image_content_type("image/png"),
            Some("image/png")
        );
        assert_eq!(safe_review_image_content_type("image/svg+xml"), None);
        assert_eq!(safe_review_image_content_type("text/html"), None);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_template_entry_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let root = temp_template_root("entry-symlink");
        let message_root = root.join("templates/en-US/message");
        fs::create_dir_all(&message_root).expect("message template directory");
        let outside = root.join("outside.md.j2");
        fs::write(&outside, "outside").expect("outside template");
        symlink(&outside, message_root.join("section.md.j2")).expect("template symlink");
        let error = TemplateRenderer::new(&root, TemplateLanguage::EnUs)
            .render(TemplateKey::MessageSection, &json!({}))
            .expect_err("workspace template symlinks must fail closed");
        assert_eq!(error.error_code, "template_path_unsafe");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_ui_templates_reject_javascript() {
        let error = validate_review_template_text(
            TemplateKey::ReviewPage.as_str(),
            "<button onclick=\"run()\">x</button>",
        )
        .expect_err("inline JavaScript must be rejected");
        assert_eq!(error.error_code, "review_template_unsafe");
        assert!(
            validate_review_template_text(
                TemplateKey::ReviewPage.as_str(),
                "<svg/onload=\"run()\"></svg>",
            )
            .is_err()
        );
        assert!(
            validate_review_template_text(TemplateKey::ReviewPage.as_str(), "{{ value |   safe }}",)
                .is_err()
        );
        assert!(
            validate_review_template_text(
                TemplateKey::ReviewPage.as_str(),
                "{% filter safe %}{{ value }}{% endfilter %}",
            )
            .is_err()
        );
        assert!(
            validate_review_template_text(
                TemplateKey::ReviewPage.as_str(),
                "{% autoescape\nfalse %}unsafe{% endautoescape %}",
            )
            .is_err()
        );
        assert!(
            validate_review_template_text(
                TemplateKey::ReviewPage.as_str(),
                "<main>{{ review.contents }}</main>",
            )
            .is_ok()
        );
        for encoded_url in [
            "<a href=\"java&#x73;cript:alert(1)\">x</a>",
            "<a href=\"java&#115;cript:alert(1)\">x</a>",
            "<a href=\"javascript&colon;alert(1)\">x</a>",
            "<a href=\"data&colon;text/html,<script>alert(1)</script>\">x</a>",
            "<a href=\"vbscript&colon;msgbox(1)\">x</a>",
            "<a href=\"data&colon;application/xhtml+xml;base64,AAAA\">x</a>",
        ] {
            assert!(
                validate_review_template_text(TemplateKey::ReviewPage.as_str(), encoded_url,)
                    .is_err(),
                "encoded executable URL must be rejected: {encoded_url}"
            );
        }

        let root = temp_template_root("rendered-review-javascript");
        let review_root = root.join("templates/en-US/review");
        fs::create_dir_all(&review_root).expect("Review template directory");
        fs::write(
            review_root.join("page.html.j2"),
            "<scr{% if true %}ipt{% endif %}>run()</scr{% if true %}ipt{% endif %}>",
        )
        .expect("split script Review template");
        let error = TemplateRenderer::frontend(&root, TemplateLanguage::EnUs)
            .render(TemplateKey::ReviewPage, &json!({}))
            .expect_err("rendered script element must be rejected");
        assert_eq!(error.error_code, "review_template_unsafe");
        fs::write(review_root.join("page.html.j2"), "<main>{{ body }}</main>")
            .expect("plain-text Review template");
        let rendered = TemplateRenderer::frontend(&root, TemplateLanguage::EnUs)
            .render(
                TemplateKey::ReviewPage,
                &json!({"body": "plain javascript: text onclick=example data:text/html"}),
            )
            .expect("dangerous-looking plain text is not executable markup");
        assert!(rendered.contains("plain javascript: text"));
        fs::write(
            review_root.join("page.html.j2"),
            "<a href=\"java{% if true %}&#x73;{% endif %}cript:alert(1)\">x</a>",
        )
        .expect("split URL scheme Review template");
        let error = TemplateRenderer::frontend(&root, TemplateLanguage::EnUs)
            .render(TemplateKey::ReviewPage, &json!({}))
            .expect_err("rendered entity-obfuscated script URL must be rejected");
        assert_eq!(error.error_code, "review_template_unsafe");
        fs::write(
            review_root.join("page.html.j2"),
            "{% set filter_name = 'sa' ~ 'fe' %}{{ values | map(filter_name) | join }}",
        )
        .expect("dynamic safe-filter Review template");
        let error = TemplateRenderer::frontend(&root, TemplateLanguage::EnUs)
            .render(
                TemplateKey::ReviewPage,
                &json!({"values": ["<script>alert(1)</script>"]}),
            )
            .expect_err("Review environment must not expose the safe filter by alias");
        assert_eq!(error.error_code, "template_render_failed");
        let partial_root = root.join("templates/en-US/partials");
        fs::create_dir_all(&partial_root).expect("Review partial directory");
        fs::write(
            review_root.join("page.html.j2"),
            "{% include \"partials/body.md.j2\" %}",
        )
        .expect("cross-suffix Review include");
        fs::write(partial_root.join("body.md.j2"), "<p>{{ body }}</p>")
            .expect("Markdown-suffixed Review partial");
        let rendered = TemplateRenderer::frontend(&root, TemplateLanguage::EnUs)
            .render(
                TemplateKey::ReviewPage,
                &json!({"body": "<b>untrusted</b>"}),
            )
            .expect("all Review partial values are HTML-escaped");
        assert!(rendered.contains("&lt;b&gt;untrusted&lt;&#x2f;b&gt;"));
        assert!(!rendered.contains("<b>untrusted</b>"));
        let _ = fs::remove_dir_all(root);
    }

    /// A frontend supplies the Review UI and nothing else. Letting it reach a
    /// content template would make `.afui/frontends/` a second way to change
    /// mail, drafts and Markdown — which is the duplication the split exists to
    /// remove.
    #[test]
    fn a_frontend_cannot_replace_content_templates() {
        let root = temp_template_root("frontend-content");
        let content_root = root.join("templates/en-US/case");
        fs::create_dir_all(&content_root).expect("content template directory");
        fs::write(content_root.join("case.md.j2"), "FRONTEND CONTENT")
            .expect("content template in a frontend");
        let mut renderer = TemplateRenderer::frontend(&root, TemplateLanguage::EnUs);
        let rendered = renderer
            .render(TemplateKey::CaseDocument, &json!({}))
            .unwrap_or_default();
        assert!(!rendered.contains("FRONTEND CONTENT"), "{rendered}");
        let _ = fs::remove_dir_all(root);
    }

    /// …and the reverse. The workspace `templates/` tree used to be able to
    /// replace the Review UI; now `.afui/frontends/` is the only entry point,
    /// so a file left behind in the old place changes nothing.
    #[test]
    fn the_workspace_content_tree_can_no_longer_replace_the_review_ui() {
        let root = temp_template_root("workspace-review");
        let review_root = root.join("templates/en-US/review");
        fs::create_dir_all(&review_root).expect("Review template directory");
        fs::write(
            review_root.join("page.html.j2"),
            "<main>WORKSPACE REVIEW</main>",
        )
        .expect("Review template in the content tree");
        let mut renderer = TemplateRenderer::new(&root, TemplateLanguage::EnUs);
        let rendered = renderer
            .render(TemplateKey::ReviewPage, &json!({}))
            .unwrap_or_default();
        assert!(!rendered.contains("WORKSPACE REVIEW"), "{rendered}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_templates_can_add_partials_but_review_partials_cannot_add_javascript() {
        let root = temp_template_root("partials");
        let language_root = root.join("templates/en-US");
        fs::create_dir_all(language_root.join("message")).expect("message template directory");
        fs::create_dir_all(language_root.join("partials")).expect("partial template directory");
        fs::create_dir_all(language_root.join("review")).expect("Review template directory");
        fs::write(
            language_root.join("message/section.md.j2"),
            "{% include \"partials/custom.md.j2\" %}",
        )
        .expect("message entry template");
        fs::write(
            language_root.join("partials/custom.md.j2"),
            "custom {{ document.message.message_id }}",
        )
        .expect("custom Markdown partial");
        let mut renderer = TemplateRenderer::new(&root, TemplateLanguage::EnUs);
        let rendered = renderer
            .render(
                TemplateKey::MessageSection,
                &json!({"message":{"message_id":"message_custom"}}),
            )
            .expect("custom partial must render");
        assert_eq!(rendered, "custom message_custom");

        fs::write(
            language_root.join("review/page.html.j2"),
            "{% include \"partials/unsafe.html.j2\" %}",
        )
        .expect("Review entry template");
        fs::write(
            language_root.join("partials/unsafe.html.j2"),
            "<script>alert(1)</script>",
        )
        .expect("unsafe Review partial");
        let mut renderer = TemplateRenderer::new(&root, TemplateLanguage::EnUs);
        let error = renderer
            .render(TemplateKey::ReviewPage, &json!({}))
            .expect_err("custom Review JavaScript must be rejected");
        assert_eq!(error.error_code, "template_render_failed");
        let _ = fs::remove_dir_all(root);
    }
}
