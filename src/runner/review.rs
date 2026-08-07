//! Local, blocking review sessions. The page is deliberately a thin renderer:
//! it receives a fixed typed Review document and can only save notes or a draft
//! locally.
//!
//! This module owns the review domain: the typed documents a surface shows, the
//! hash-checked saves it allows, and the decision it returns. AFUI owns the
//! delivery — the listener, the credential, expiry, the isolated window — and
//! [`host`] is the only place the two meet.

mod host;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::Ipv4Addr;
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::host::run_session;
use super::output::progress_event;
use crate::cli::{
    CaseCommand, CaseDraftAction, Command, NotificationCommand, ReviewMode, TriageAction,
};
use crate::config::MailConfig;
use crate::error::{AppError, Result};
use crate::render_document::{
    CollectionRenderDocument, DraftRenderDocument, NotesRenderDocument,
    RENDER_DOCUMENT_SCHEMA_VERSION, TriageRenderDocument,
};
use crate::review_frontend::review_frontend;
use crate::store::{
    CaseHumanSuggestionInput, DraftChange, MessageHumanSuggestionInput,
    NotificationHumanSuggestionInput, Workspace,
};
use crate::templates::{TemplateKey, TemplateRenderOptions, TemplateRenderer};
use crate::types::{
    CaseHumanSuggestion, CollectionAgentSuggestion, CommandResult, HumanSuggestionOutcome,
    MessageHumanSuggestion, MessageSuggestedAction, NotificationHumanSuggestion,
    NotificationSuggestedAction,
};
use crate::workspace_lock::{LockMode, WorkspaceLock};

pub(super) fn is_review_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Triage {
            action: TriageAction::Review { .. },
        } | Command::Case {
            action: CaseCommand::Review { .. }
                | CaseCommand::Draft {
                    action: CaseDraftAction::Review { .. },
                },
        } | Command::Notification {
            action: NotificationCommand::Review { .. },
        }
    )
}

pub(super) fn run_review_command<W: Write>(
    command: &Command,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<CommandResult> {
    let result = match command {
        Command::Triage {
            action: TriageAction::Review { message_id, mode },
        } => run_triage_review(message_id, *mode, emitter, started),
        Command::Case {
            action: CaseCommand::Review { case_ref, mode },
        } => run_case_review(case_ref, *mode, emitter, started),
        Command::Case {
            action:
                CaseCommand::Draft {
                    action:
                        CaseDraftAction::Review {
                            case_ref,
                            draft_name,
                            mode,
                        },
                },
        } => run_draft_review(case_ref, draft_name, *mode, emitter, started),
        Command::Notification {
            action:
                NotificationCommand::Review {
                    notification_ref,
                    mode,
                },
        } => run_notification_review(notification_ref, *mode, emitter, started),
        _ => Err(AppError::new(
            "review_dispatch_error",
            "run_review_command requires a review command",
        )
        .into()),
    };
    result.map(CommandResult::succeeded)
}

pub(super) enum ReviewRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for ReviewRunError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<CliEmitterError> for ReviewRunError {
    fn from(error: CliEmitterError) -> Self {
        Self::Output(error)
    }
}

type ReviewRunResult<T> = std::result::Result<T, ReviewRunError>;

const LAN_REVIEW_EXPIRES_IN_S: u64 = 15 * 60;
const TRUSTED_RUNTIME_MARKER: &str = "<!-- afmail:trusted-runtime -->";

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReviewSurfaceKind {
    #[serde(rename = "triage_review")]
    Triage,
    #[serde(rename = "case_review")]
    Case,
    #[serde(rename = "notification_review")]
    Notification,
    #[serde(rename = "draft_review")]
    Draft,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewRenderDocument {
    schema_name: String,
    schema_version: u64,
    #[serde(skip)]
    interaction_id: String,
    #[serde(skip)]
    workspace_uid: String,
    surface_kind: ReviewSurfaceKind,
    #[serde(skip)]
    subject_refs: Vec<String>,
    #[serde(skip)]
    base_hash: String,
    contents: ReviewContents,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum ReviewContents {
    Triage(TriageReviewContents),
    Case(CaseReviewContents),
    Notification(NotificationReviewContents),
    Draft(DraftReviewContents),
}

#[derive(Clone, Debug, Serialize)]
struct TriageReviewContents {
    items: Vec<TriageRenderDocument>,
}

#[derive(Clone, Debug, Serialize)]
struct CaseReviewContents {
    collection: CollectionRenderDocument,
    notes: EditableReviewDocument<NotesRenderDocument>,
    drafts: Vec<EditableReviewDocument<DraftRenderDocument>>,
}

#[derive(Clone, Debug, Serialize)]
struct NotificationReviewContents {
    collection: CollectionRenderDocument,
    notes: EditableReviewDocument<NotesRenderDocument>,
}

#[derive(Clone, Debug, Serialize)]
struct DraftReviewContents {
    draft: EditableReviewDocument<DraftRenderDocument>,
}

#[derive(Clone, Debug, Serialize)]
struct EditableReviewDocument<T> {
    document: T,
    base_hash: String,
}

fn run_triage_review<W: Write>(
    message_ids: &[String],
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let selected = if message_ids.is_empty() {
        workspace.triage_suggested_message_ids()?
    } else {
        let mut unique = HashSet::new();
        for message_id in message_ids {
            if !unique.insert(message_id.as_str()) {
                return Err(AppError::new(
                    "invalid_review_request",
                    "each message_id may appear only once",
                )
                .into());
            }
        }
        message_ids.to_vec()
    };
    if selected.is_empty() {
        return Err(triage_suggestion_not_found_error(None).into());
    }
    let mut items = Vec::new();
    for message_id in &selected {
        let document = workspace.triage_template_document(message_id)?;
        let suggestion = document
            .context
            .message
            .message
            .agent_suggestion
            .as_ref()
            .ok_or_else(|| triage_suggestion_not_found_error(Some(message_id)))?;
        let current_base_hash = workspace.message_review_base_hash(message_id)?;
        if suggestion.message_review_base_hash != current_base_hash {
            return Err(AppError::new(
                "review_stale",
                format!("agent_suggestion for {message_id} no longer matches local mail"),
            )
            .with_hint("Read the message again and replace its agent_suggestion before review.")
            .with_details(json!({"message_id":message_id,"expected_message_review_base_hash":suggestion.message_review_base_hash,"current_message_review_base_hash":current_base_hash}))
            .into());
        }
        items.push(document);
    }
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Triage,
        ReviewContents::Triage(TriageReviewContents { items }),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Triage,
        model,
        mode,
        emitter,
        started,
    )
}

fn run_case_review<W: Write>(
    case_ref: &str,
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let collection = workspace.case_render_document(case_ref)?;
    let current_base_hash = workspace.case_review_base_hash(case_ref)?;
    if let Some(CollectionAgentSuggestion::Case(suggestion)) =
        collection.collection.agent_suggestion.as_ref()
        && suggestion.case_review_base_hash != current_base_hash
    {
        return Err(AppError::new(
            "review_stale",
            format!("agent_suggestion for {case_ref} no longer matches the case"),
        )
        .with_hint("Read the case again and replace its agent_suggestion before review.")
        .with_details(json!({
            "case_ref": case_ref,
            "expected_case_review_base_hash": suggestion.case_review_base_hash,
            "current_case_review_base_hash": current_base_hash,
        }))
        .into());
    }
    let case_uid = collection.collection.collection_uid.clone();
    let notes = workspace.case_notes_render_document(&case_uid)?;
    let notes = EditableReviewDocument {
        base_hash: value_hash(&notes)?,
        document: notes,
    };
    let archived = !collection.collection.status.is_empty();
    let mut drafts = Vec::new();
    if !archived {
        for document in workspace.draft_render_documents(&case_uid)? {
            drafts.push(EditableReviewDocument {
                base_hash: value_hash(&document)?,
                document,
            });
        }
    }
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Case,
        ReviewContents::Case(CaseReviewContents {
            collection,
            notes,
            drafts,
        }),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Case { case_ref: case_uid },
        model,
        mode,
        emitter,
        started,
    )
}

fn run_notification_review<W: Write>(
    notification_ref: &str,
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let collection = workspace.notification_render_document(notification_ref)?;
    let current_base_hash = workspace.notification_review_base_hash(notification_ref)?;
    if let Some(CollectionAgentSuggestion::Notification(suggestion)) =
        collection.collection.agent_suggestion.as_ref()
        && suggestion.notification_review_base_hash != current_base_hash
    {
        return Err(AppError::new(
            "review_stale",
            format!("agent_suggestion for {notification_ref} no longer matches the notification"),
        )
        .with_hint("Read the notification again and replace its agent_suggestion before review.")
        .with_details(json!({
            "notification_ref": notification_ref,
            "expected_notification_review_base_hash": suggestion.notification_review_base_hash,
            "current_notification_review_base_hash": current_base_hash,
        }))
        .into());
    }
    let notification_uid = collection.collection.collection_uid.clone();
    let notes = workspace.notification_notes_render_document(&notification_uid)?;
    let notes = EditableReviewDocument {
        base_hash: value_hash(&notes)?,
        document: notes,
    };
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Notification,
        ReviewContents::Notification(NotificationReviewContents { collection, notes }),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Notification {
            notification_ref: notification_uid,
        },
        model,
        mode,
        emitter,
        started,
    )
}

fn run_draft_review<W: Write>(
    case_ref: &str,
    draft_name: &str,
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let document = workspace.draft_render_document(case_ref, draft_name)?;
    let case_uid = document.draft.frontmatter.case_uid.clone();
    let draft_name = document.locator.draft_name.clone();
    let contents = DraftReviewContents {
        draft: EditableReviewDocument {
            base_hash: value_hash(&document)?,
            document,
        },
    };
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Draft,
        ReviewContents::Draft(contents),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Draft {
            case_ref: case_uid,
            draft_name,
        },
        model,
        mode,
        emitter,
        started,
    )
}

enum ReviewSurface {
    Triage,
    Case {
        case_ref: String,
    },
    Notification {
        notification_ref: String,
    },
    Draft {
        case_ref: String,
        draft_name: String,
    },
}

/// One notes-editor save, exactly as the review page sends it.
#[derive(Debug, Default, Deserialize)]
struct NotesEdit {
    #[serde(default)]
    base_hash: String,
    #[serde(default)]
    text: String,
}

/// One draft-editor save.
///
/// `draft_name` names which of a case review's listed drafts this is; the draft
/// surface shows exactly one draft and ignores it. `subject` and `body` are
/// optional because an absent field leaves that part of the draft alone, while
/// an empty `cc`/`bcc` clears that recipient list.
#[derive(Debug, Default, Deserialize)]
struct DraftEdit {
    #[serde(default)]
    base_hash: String,
    #[serde(default)]
    draft_name: String,
    subject: Option<String>,
    #[serde(default)]
    to: String,
    #[serde(default)]
    cc: String,
    #[serde(default)]
    bcc: String,
    body: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SavedFile {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    case_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notification_ref: Option<String>,
    draft_name: Option<String>,
    path: String,
    base_hash: String,
}

#[derive(Default)]
struct ReviewSessionState {
    saved_files: Vec<SavedFile>,
}

#[derive(Serialize)]
struct ReviewEnvelope<'a> {
    interaction_id: &'a str,
    workspace_uid: &'a str,
    surface_kind: ReviewSurfaceKind,
    subject_refs: &'a [String],
    base_hash: &'a str,
    outcome: &'a str,
    saved_count: usize,
    saved_files: &'a [SavedFile],
    #[serde(flatten)]
    payload: ReviewPayload,
}

#[derive(Serialize)]
#[serde(tag = "result_kind", rename_all = "snake_case")]
enum ReviewPayload {
    Empty,
    Triage {
        human_suggestions: Vec<MessageHumanSuggestionResult>,
    },
    Case {
        case_ref: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        human_suggestion: Option<CaseHumanSuggestion>,
    },
    Notification {
        notification_ref: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        human_suggestion: Option<NotificationHumanSuggestion>,
    },
    Draft {
        case_ref: String,
        draft_name: String,
    },
}

#[derive(Serialize)]
struct MessageHumanSuggestionResult {
    message_id: String,
    human_suggestion: MessageHumanSuggestion,
}

fn review_envelope(
    model: &ReviewRenderDocument,
    outcome: &str,
    state: &ReviewSessionState,
    payload: ReviewPayload,
) -> Result<Value> {
    serde_json::to_value(ReviewEnvelope {
        interaction_id: &model.interaction_id,
        workspace_uid: &model.workspace_uid,
        surface_kind: model.surface_kind,
        subject_refs: &model.subject_refs,
        base_hash: &model.base_hash,
        outcome,
        saved_count: state.saved_files.len(),
        saved_files: &state.saved_files,
        payload,
    })
    .map_err(|error| AppError::json("serialize review result", &error))
}

/// Announce the open review, and say whose UI is serving it.
///
/// `review_frontend_id` is absent when the built-in UI is serving. An Agent
/// that installed a frontend can tell from this alone whether the page it is
/// pointing a person at is the one they wrote or the one afmail ships — which
/// is the only way an untrusted frontend, the one case that is deliberately
/// silent, becomes visible without opening the window.
fn emit_review_ready<W: Write>(
    emitter: &mut CliEmitter<W>,
    model: &ReviewRenderDocument,
    mode: ReviewMode,
    review_url: Option<&str>,
    frontend_id: Option<&str>,
    started: Instant,
) -> std::result::Result<(), CliEmitterError> {
    let mut progress = serde_json::Map::new();
    progress.insert("phase".to_string(), json!("review_ready"));
    match review_url {
        Some(review_url) => {
            progress.insert(
                "message".to_string(),
                json!(
                    "Open this short-lived review URL from a device on the trusted local network."
                ),
            );
            progress.insert("review_url".to_string(), json!(review_url));
            progress.insert("expires_in_s".to_string(), json!(LAN_REVIEW_EXPIRES_IN_S));
        }
        None => {
            progress.insert(
                "message".to_string(),
                json!("The local review window is open."),
            );
        }
    }
    progress.insert("review_mode".to_string(), json!(review_mode_name(mode)));
    progress.insert("surface_kind".to_string(), json!(model.surface_kind));
    progress.insert("subject_refs".to_string(), json!(&model.subject_refs));
    if let Some(frontend_id) = frontend_id {
        progress.insert("review_frontend_id".to_string(), json!(frontend_id));
    }
    let progress = Value::Object(progress);
    emitter.emit(
        agent_first_data::json_progress(progress_event(progress))
            .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
            .build(),
    )
}

fn review_mode_name(mode: ReviewMode) -> &'static str {
    match mode {
        ReviewMode::Local => "local",
        ReviewMode::Lan => "lan",
    }
}

fn triage_suggestion_not_found_error(message_id: Option<&str>) -> AppError {
    let message_ref = message_id.unwrap_or("MESSAGE_ID");
    let message = match message_id {
        Some(message_id) => format!("message has no current agent_suggestion: {message_id}"),
        None => "no triage messages have a current agent_suggestion".to_string(),
    };
    AppError::new("suggestion_not_found", message)
        .with_hint(format!(
            "Run `afmail message show {message_ref}`, copy its `review_base_hash`, then run `afmail triage suggestion set {message_ref} --message-review-base-hash HASH --reason TEXT ACTION...` before review."
        ))
        .with_details(json!({
            "message_id": message_id,
            "suggested_commands": [
                format!("afmail message show {message_ref}"),
                format!(
                    "afmail triage suggestion set {message_ref} --message-review-base-hash HASH --reason TEXT ACTION..."
                ),
            ],
        }))
}

fn primary_lan_ipv4() -> Result<Ipv4Addr> {
    crate::lan::primary_lan_ipv4().map_err(|error| {
        AppError::new(
            "review_lan_unavailable",
            format!("could not select an IPv4 address for LAN review: {error}"),
        )
        .with_hint("Connect this machine to a trusted IPv4 local network and retry.")
    })
}

fn review_input_error(error: &AppError) -> bool {
    matches!(
        error.error_code,
        "invalid_request" | "draft_invalid" | "draft_incomplete" | "config_invalid"
    )
}

fn save_notes(
    surface: &ReviewSurface,
    edit: &NotesEdit,
) -> Result<std::result::Result<SavedFile, String>> {
    let workspace = current_workspace()?;
    match surface {
        ReviewSurface::Case { case_ref } => {
            save_notes_in_workspace(&workspace, case_ref, &edit.text, &edit.base_hash)
        }
        ReviewSurface::Notification { notification_ref } => save_notification_notes_in_workspace(
            &workspace,
            notification_ref,
            &edit.text,
            &edit.base_hash,
        ),
        ReviewSurface::Triage | ReviewSurface::Draft { .. } => Err(AppError::new(
            "review_save_invalid",
            "this review surface does not allow notes saves",
        )),
    }
}

fn save_notes_in_workspace(
    workspace: &Workspace,
    case_ref: &str,
    text: &str,
    base_hash: &str,
) -> Result<std::result::Result<SavedFile, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.case_notes_render_document(case_ref)?;
    let current_hash = value_hash(&current)?;
    if current_hash != base_hash {
        return Ok(Err(current_hash));
    }
    let changed = workspace.case_notes_replace(case_ref, text)?;
    let saved = workspace.case_notes_render_document(case_ref)?;
    Ok(Ok(SavedFile {
        kind: "notes",
        case_ref: Some(case_ref.to_string()),
        notification_ref: None,
        draft_name: None,
        path: saved_path(&changed, "notes_path")?,
        base_hash: value_hash(&saved)?,
    }))
}

fn save_notification_notes_in_workspace(
    workspace: &Workspace,
    notification_ref: &str,
    text: &str,
    base_hash: &str,
) -> Result<std::result::Result<SavedFile, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.notification_notes_render_document(notification_ref)?;
    let current_hash = value_hash(&current)?;
    if current_hash != base_hash {
        return Ok(Err(current_hash));
    }
    let changed = workspace.notification_notes_replace(notification_ref, text)?;
    let saved = workspace.notification_notes_render_document(notification_ref)?;
    Ok(Ok(SavedFile {
        kind: "notes",
        case_ref: None,
        notification_ref: Some(notification_ref.to_string()),
        draft_name: None,
        path: saved_path(&changed, "notes_path")?,
        base_hash: value_hash(&saved)?,
    }))
}

fn save_draft(
    case_ref: &str,
    draft_name: &str,
    edit: &DraftEdit,
) -> Result<std::result::Result<SavedFile, String>> {
    let workspace = current_workspace()?;
    save_draft_in_workspace(&workspace, case_ref, draft_name, edit)
}

fn save_draft_in_workspace(
    workspace: &Workspace,
    case_ref: &str,
    draft_name: &str,
    edit: &DraftEdit,
) -> Result<std::result::Result<SavedFile, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.draft_render_document(case_ref, draft_name)?;
    let current_hash = value_hash(&current)?;
    if current_hash != edit.base_hash {
        return Ok(Err(current_hash));
    }
    if edit.to.trim().is_empty() && edit.bcc.trim().is_empty() {
        return Err(AppError::new(
            "draft_invalid",
            "draft requires at least one to or bcc recipient",
        ));
    }
    let to = crate::smtp_send::normalize_recipient_list_text(&edit.to, "to")?;
    let cc = crate::smtp_send::normalize_recipient_list_text(&edit.cc, "cc")?;
    let bcc = crate::smtp_send::normalize_recipient_list_text(&edit.bcc, "bcc")?;
    let changed = workspace.change_draft(
        case_ref,
        draft_name,
        DraftChange {
            subject: edit.subject.as_deref(),
            to: &to,
            cc: &cc,
            clear_cc: edit.cc.trim().is_empty(),
            bcc: &bcc,
            clear_bcc: edit.bcc.trim().is_empty(),
            identity: None,
            body: edit.body.as_deref(),
            body_file: None,
        },
    )?;
    Ok(Ok(SavedFile {
        kind: "draft",
        case_ref: Some(case_ref.to_string()),
        notification_ref: None,
        draft_name: Some(draft_name.to_string()),
        path: saved_path(&changed, "draft_path")?,
        base_hash: value_hash(&workspace.draft_render_document(case_ref, draft_name)?)?,
    }))
}

fn current_workspace() -> Result<Workspace> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    Workspace::discover(&cwd)
}

fn saved_path(value: &Value, field: &str) -> Result<String> {
    value[field].as_str().map(str::to_string).ok_or_else(|| {
        AppError::new(
            "review_save_invalid",
            format!("local save result omitted {field}"),
        )
    })
}

fn saved_draft_target(
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    requested: &str,
) -> Result<(String, String)> {
    match surface {
        ReviewSurface::Draft {
            case_ref,
            draft_name,
        } => Ok((case_ref.clone(), draft_name.clone())),
        ReviewSurface::Case { case_ref } => {
            let is_listed = match &model.contents {
                ReviewContents::Case(contents) => contents
                    .drafts
                    .iter()
                    .any(|draft| draft.document.locator.draft_name == requested),
                _ => false,
            };
            if !is_listed {
                return Err(AppError::new(
                    "review_draft_not_allowed",
                    "only drafts listed in this case review can be saved",
                ));
            }
            Ok((case_ref.clone(), requested.to_string()))
        }
        ReviewSurface::Triage | ReviewSurface::Notification { .. } => Err(AppError::new(
            "review_draft_not_allowed",
            "this review surface does not allow draft saves",
        )),
    }
}

fn content_hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}
fn value_hash<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map(|value| content_hash(&value))
        .map_err(|error| AppError::json("serialize review hash", &error))
}

/// The decision fields the review page submits.
///
/// Triage names one decision and one reason per message, so the field names are
/// the message ids themselves and no fixed struct can describe the payload.
type SubmittedFields = HashMap<String, String>;

fn submitted_result(
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    state: &ReviewSessionState,
    values: &SubmittedFields,
) -> Result<Value> {
    match surface {
        ReviewSurface::Triage => {
            let ReviewContents::Triage(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            let mut inputs = Vec::new();
            for item in &contents.items {
                let message_id = &item.context.message.message.message_id;
                let agent_suggestion = item
                    .context
                    .message
                    .message
                    .agent_suggestion
                    .as_ref()
                    .ok_or_else(|| triage_suggestion_not_found_error(Some(message_id)))?;
                let choice = values
                    .get(&format!("decision:{message_id}"))
                    .map(String::as_str)
                    .unwrap_or("accept");
                let reason = values
                    .get(&format!("reason:{message_id}"))
                    .map(String::as_str)
                    .unwrap_or_default()
                    .trim();
                let (outcome, suggested_action, reason) = match choice {
                    "" => {
                        return Err(AppError::new(
                            "invalid_triage_decision",
                            "choose a decision for every message",
                        ));
                    }
                    "accept" => (
                        HumanSuggestionOutcome::Accepted,
                        Some(agent_suggestion.suggested_action.clone()),
                        None,
                    ),
                    "reject" if !reason.is_empty() => (
                        HumanSuggestionOutcome::Rejected,
                        None,
                        Some(reason.to_string()),
                    ),
                    "defer" => (HumanSuggestionOutcome::Deferred, None, None),
                    "override_spam" if !reason.is_empty() => (
                        HumanSuggestionOutcome::Changed,
                        Some(MessageSuggestedAction::Spam),
                        Some(reason.to_string()),
                    ),
                    "override_trash" if !reason.is_empty() => (
                        HumanSuggestionOutcome::Changed,
                        Some(MessageSuggestedAction::Trash),
                        Some(reason.to_string()),
                    ),
                    _ => {
                        return Err(AppError::new(
                            "invalid_triage_decision",
                            "reject and override decisions require a reason; override must be spam or trash",
                        ));
                    }
                };
                inputs.push(MessageHumanSuggestionInput {
                    message_id: message_id.clone(),
                    agent_suggestion_uid: agent_suggestion.agent_suggestion_uid.clone(),
                    message_review_base_hash: agent_suggestion.message_review_base_hash.clone(),
                    outcome,
                    suggested_action,
                    reason,
                });
            }
            let workspace = current_workspace()?;
            let _lock = acquire_review_write_lock(&workspace)?;
            let human_suggestions = workspace
                .set_message_human_suggestions(&inputs)?
                .into_iter()
                .map(
                    |(message_id, human_suggestion)| MessageHumanSuggestionResult {
                        message_id,
                        human_suggestion,
                    },
                )
                .collect();
            review_envelope(
                model,
                "submitted",
                state,
                ReviewPayload::Triage { human_suggestions },
            )
        }
        ReviewSurface::Case { case_ref } => {
            let ReviewContents::Case(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            let agent = match contents.collection.collection.agent_suggestion.as_ref() {
                None => None,
                Some(CollectionAgentSuggestion::Case(agent)) => Some(agent),
                Some(CollectionAgentSuggestion::Notification(_)) => {
                    return Err(review_contents_mismatch());
                }
            };
            let human_suggestion = if let Some(agent) = agent {
                let choice = values
                    .get("case_decision")
                    .map(String::as_str)
                    .unwrap_or("accept");
                if choice == "accept"
                    && matches!(
                        &agent.suggested_action,
                        crate::types::CaseSuggestedAction::CaseDelete { allow_notes: true }
                    )
                    && values
                        .get("case_delete_allow_notes_confirmed")
                        .map(String::as_str)
                        != Some("true")
                {
                    return Err(AppError::new(
                        "invalid_case_decision",
                        "accepting case deletion with allow_notes requires explicit notes deletion confirmation",
                    ));
                }
                let reason = values
                    .get("case_reason")
                    .map(String::as_str)
                    .unwrap_or_default()
                    .trim();
                let (outcome, suggested_action, reason) = match choice {
                    "accept" => (
                        HumanSuggestionOutcome::Accepted,
                        Some(agent.suggested_action.clone()),
                        None,
                    ),
                    "reject" if !reason.is_empty() => (
                        HumanSuggestionOutcome::Rejected,
                        None,
                        Some(reason.to_string()),
                    ),
                    "defer" => (HumanSuggestionOutcome::Deferred, None, None),
                    _ => {
                        return Err(AppError::new(
                            "invalid_case_decision",
                            "case rejection requires a reason",
                        ));
                    }
                };
                let workspace = current_workspace()?;
                let _lock = acquire_review_write_lock(&workspace)?;
                Some(
                    workspace.set_case_human_suggestion(&CaseHumanSuggestionInput {
                        case_ref: case_ref.clone(),
                        agent_suggestion_uid: agent.agent_suggestion_uid.clone(),
                        case_review_base_hash: agent.case_review_base_hash.clone(),
                        outcome,
                        suggested_action,
                        reason,
                    })?,
                )
            } else {
                None
            };
            review_envelope(
                model,
                "submitted",
                state,
                ReviewPayload::Case {
                    case_ref: case_ref.clone(),
                    human_suggestion,
                },
            )
        }
        ReviewSurface::Notification { notification_ref } => {
            let ReviewContents::Notification(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            let agent = match contents.collection.collection.agent_suggestion.as_ref() {
                None => None,
                Some(CollectionAgentSuggestion::Notification(agent)) => Some(agent),
                Some(CollectionAgentSuggestion::Case(_)) => {
                    return Err(review_contents_mismatch());
                }
            };
            let human_suggestion = if let Some(agent) = agent {
                let choice = values
                    .get("notification_decision")
                    .map(String::as_str)
                    .unwrap_or("accept");
                if choice == "accept"
                    && matches!(
                        &agent.suggested_action,
                        NotificationSuggestedAction::NotificationDelete { allow_notes: true }
                    )
                    && values
                        .get("notification_delete_allow_notes_confirmed")
                        .map(String::as_str)
                        != Some("true")
                {
                    return Err(AppError::new(
                        "invalid_notification_decision",
                        "accepting notification deletion with allow_notes requires explicit notes deletion confirmation",
                    ));
                }
                let reason = values
                    .get("notification_reason")
                    .map(String::as_str)
                    .unwrap_or_default()
                    .trim();
                let (outcome, suggested_action, reason) = match choice {
                    "accept" => (
                        HumanSuggestionOutcome::Accepted,
                        Some(agent.suggested_action.clone()),
                        None,
                    ),
                    "reject" if !reason.is_empty() => (
                        HumanSuggestionOutcome::Rejected,
                        None,
                        Some(reason.to_string()),
                    ),
                    "defer" => (HumanSuggestionOutcome::Deferred, None, None),
                    _ => {
                        return Err(AppError::new(
                            "invalid_notification_decision",
                            "notification rejection requires a reason",
                        ));
                    }
                };
                let workspace = current_workspace()?;
                let _lock = acquire_review_write_lock(&workspace)?;
                Some(workspace.set_notification_human_suggestion(
                    &NotificationHumanSuggestionInput {
                        notification_ref: notification_ref.clone(),
                        agent_suggestion_uid: agent.agent_suggestion_uid.clone(),
                        notification_review_base_hash: agent.notification_review_base_hash.clone(),
                        outcome,
                        suggested_action,
                        reason,
                    },
                )?)
            } else {
                None
            };
            review_envelope(
                model,
                "submitted",
                state,
                ReviewPayload::Notification {
                    notification_ref: notification_ref.clone(),
                    human_suggestion,
                },
            )
        }
        ReviewSurface::Draft {
            case_ref,
            draft_name,
        } => review_envelope(
            model,
            "submitted",
            state,
            ReviewPayload::Draft {
                case_ref: case_ref.clone(),
                draft_name: draft_name.clone(),
            },
        ),
    }
}

fn review_contents_mismatch() -> AppError {
    AppError::new(
        "review_document_invalid",
        "review surface and typed contents do not match",
    )
}

fn acquire_review_write_lock(workspace: &Workspace) -> Result<WorkspaceLock> {
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Exclusive)?;
    workspace.ensure_no_incomplete_transactions()?;
    Ok(lock)
}

fn review_render_document(
    workspace: &Workspace,
    surface_kind: ReviewSurfaceKind,
    contents: ReviewContents,
) -> Result<ReviewRenderDocument> {
    let identity = crate::workspace_identity::load_workspace_identity(workspace.root())?;
    let base_hash = value_hash(&contents)?;
    let subject_refs = subject_refs(&contents);
    Ok(ReviewRenderDocument {
        schema_name: "afmail_review_render_document".to_string(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        interaction_id: interaction_id()?,
        workspace_uid: identity.workspace_uid,
        surface_kind,
        subject_refs,
        base_hash,
        contents,
    })
}

fn subject_refs(contents: &ReviewContents) -> Vec<String> {
    match contents {
        ReviewContents::Triage(contents) => contents
            .items
            .iter()
            .map(|item| item.context.message.message.message_id.clone())
            .collect(),
        ReviewContents::Case(contents) => {
            std::iter::once(contents.collection.collection.collection_uid.clone())
                .chain(
                    contents
                        .drafts
                        .iter()
                        .map(|draft| draft.document.locator.draft_name.clone()),
                )
                .collect()
        }
        ReviewContents::Notification(contents) => {
            vec![contents.collection.collection.collection_uid.clone()]
        }
        ReviewContents::Draft(contents) => vec![
            contents.draft.document.draft.frontmatter.case_uid.clone(),
            contents.draft.document.locator.draft_name.clone(),
        ],
    }
}

/// Mint the identifier the Agent correlates this review's result with.
///
/// This is a domain fact carried in the result envelope, not a credential:
/// AFUI mints the session's access credential and its CSP nonce, and neither
/// is ever this value.
fn interaction_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        AppError::new(
            "review_random_failed",
            "could not generate a review interaction id",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Render the review page, binding the trusted runtime to the session's nonce.
///
/// The nonce comes from the host because the header that authorizes it does:
/// a page rendered against any other nonce would have its own script refused.
///
/// The templates come from the person's own frontend when they installed one
/// for this surface, and from afmail's built-ins otherwise. The choice is made
/// per request rather than once per session, because trust is a fact about the
/// exact bytes on disk: editing a trusted frontend mid-review revokes it, and
/// the next page must already be the built-in one.
fn render_page(workspace: &Workspace, model: &ReviewRenderDocument, nonce: &str) -> Result<String> {
    let frontend = review_frontend(workspace.root(), host::review_ui_kind(model.surface_kind))?;
    render_page_from(workspace, model, nonce, frontend.as_ref())
}

/// Render one review page from a Review UI that has already been resolved.
fn render_page_from(
    workspace: &Workspace,
    model: &ReviewRenderDocument,
    nonce: &str,
    frontend: Option<&agent_first_ui::FrontendLocation>,
) -> Result<String> {
    let config = MailConfig::load(workspace.root())?;
    let language = config.template_language();
    let mut renderer = match frontend {
        Some(location) => TemplateRenderer::frontend(&location.root, language),
        None => TemplateRenderer::builtin(language),
    };
    let mut html = renderer.render_with_options(
        TemplateKey::ReviewPage,
        model,
        TemplateRenderOptions {
            timezone_offset: Some(config.resolved_timezone_offset()),
            output_dir: None,
        },
    )?;
    let runtime = format!(
        "<script nonce=\"{nonce}\">{}</script>",
        include_str!("review_runtime.js")
    );
    if html.matches(TRUSTED_RUNTIME_MARKER).count() != 1 {
        return Err(AppError::new(
            "review_template_invalid",
            "Review page must contain exactly one <!-- afmail:trusted-runtime --> marker",
        )
        .with_hint(
            "Place the trusted runtime marker where afmail should inject its behavior-only script, normally immediately before </body>.",
        ));
    }
    html = html.replacen(TRUSTED_RUNTIME_MARKER, &runtime, 1);
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewDraft;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Stands in for the per-session nonce AFUI mints for the live host.
    const TEST_NONCE: &str = "0123456789abcdef0123456789abcdef";

    fn temp_workspace(name: &str) -> (PathBuf, Workspace, String, String) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("afmail-review-{name}-{stamp}"));
        let workspace = Workspace::at(&root);
        workspace.init().expect("test workspace must initialize");
        let case = workspace
            .create_case("Review", Some("support"), None, None, None)
            .expect("test case must create");
        let case_ref = case["case_uid"]
            .as_str()
            .expect("created case must have a uid")
            .to_string();
        let draft = workspace
            .create_draft(
                &case_ref,
                NewDraft {
                    to: &["customer@example.com".to_string()],
                    cc: &[],
                    bcc: &[],
                    subject: "Review draft",
                    identity: None,
                    body: Some("Initial authored text"),
                    body_file: None,
                },
            )
            .expect("test draft must create");
        let draft_name = draft["draft_name"]
            .as_str()
            .expect("created draft must have a name")
            .to_string();
        (root, workspace, case_ref, draft_name)
    }

    fn case_review_document(workspace: &Workspace, case_ref: &str) -> ReviewRenderDocument {
        let collection = workspace
            .case_render_document(case_ref)
            .expect("case render document");
        let notes_document = workspace
            .case_notes_render_document(case_ref)
            .expect("case notes render document");
        let notes = EditableReviewDocument {
            base_hash: value_hash(&notes_document).expect("notes document hash"),
            document: notes_document,
        };
        let drafts = workspace
            .draft_render_documents(case_ref)
            .expect("draft render documents")
            .into_iter()
            .map(|document| EditableReviewDocument {
                base_hash: value_hash(&document).expect("draft document hash"),
                document,
            })
            .collect();
        review_render_document(
            workspace,
            ReviewSurfaceKind::Case,
            ReviewContents::Case(CaseReviewContents {
                collection,
                notes,
                drafts,
            }),
        )
        .expect("case Review document")
    }

    #[test]
    fn review_envelope_keeps_all_saved_files_for_every_outcome() {
        let (root, workspace, case_ref, _) = temp_workspace("envelope");
        let model = case_review_document(&workspace, &case_ref);
        let state = ReviewSessionState {
            saved_files: vec![
                SavedFile {
                    kind: "notes",
                    case_ref: Some("c1".to_string()),
                    notification_ref: None,
                    draft_name: None,
                    path: "cases/c1/notes.md".to_string(),
                    base_hash: "n1".to_string(),
                },
                SavedFile {
                    kind: "notes",
                    case_ref: Some("c1".to_string()),
                    notification_ref: None,
                    draft_name: None,
                    path: "cases/c1/notes.md".to_string(),
                    base_hash: "n2".to_string(),
                },
                SavedFile {
                    kind: "draft",
                    case_ref: Some("c1".to_string()),
                    notification_ref: None,
                    draft_name: Some("reply.md".to_string()),
                    path: "cases/c1/drafts/reply.md".to_string(),
                    base_hash: "d1".to_string(),
                },
            ],
        };
        for outcome in ["submitted", "cancelled", "dismissed", "expired"] {
            let result = review_envelope(&model, outcome, &state, ReviewPayload::Empty)
                .expect("test review envelope must serialize");
            assert_eq!(result["outcome"], outcome);
            assert_eq!(result["saved_count"], 3);
            assert_eq!(result["saved_files"][1]["base_hash"], "n2");
            assert_eq!(result["saved_files"][2]["path"], "cases/c1/drafts/reply.md");
            assert_eq!(result["saved_files"][2]["base_hash"], "d1");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// A closed window and a lapsed URL are different answers.
    ///
    /// AFUI's contract says so — `Closed` is someone declining to act and
    /// `Expired` is nobody having looked — and the Agent reads the difference:
    /// a dismissal should not be re-proposed at once, a lapse can be.
    #[test]
    fn a_closed_window_and_a_lapsed_url_do_not_report_the_same_outcome() {
        let (root, workspace, case_ref, _) = temp_workspace("ended-outcomes");
        let model = case_review_document(&workspace, &case_ref);
        let state = ReviewSessionState {
            saved_files: vec![SavedFile {
                kind: "notes",
                case_ref: Some("c1".to_string()),
                notification_ref: None,
                draft_name: None,
                path: "cases/c1/notes.md".to_string(),
                base_hash: "n1".to_string(),
            }],
        };
        let closed = host::ended_envelope(&model, &state, agent_first_ui::Outcome::Closed)
            .expect("closed envelope must serialize");
        assert_eq!(closed["outcome"], "dismissed");
        let expired = host::ended_envelope(&model, &state, agent_first_ui::Outcome::Expired)
            .expect("expired envelope must serialize");
        assert_eq!(expired["outcome"], "expired");
        // Whatever ended the session, the files it wrote are still reported.
        for ended in [&closed, &expired] {
            assert_eq!(ended["saved_count"], 1);
            assert_eq!(ended["saved_files"][0]["path"], "cases/c1/notes.md");
        }
        // A decision the page reached is passed through untouched.
        let submitted = review_envelope(&model, "submitted", &state, ReviewPayload::Empty)
            .expect("submitted envelope must serialize");
        assert_eq!(
            host::ended_envelope(
                &model,
                &ReviewSessionState::default(),
                agent_first_ui::Outcome::Completed(submitted.clone()),
            )
            .expect("completed envelope passes through"),
            submitted
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn case_review_contains_the_exact_markdown_collection_document() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("shared-document");
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case review hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                crate::types::CaseSuggestedAction::CaseArchive,
                "This case is complete",
                None,
            )
            .expect("case suggestion");
        let markdown_document = workspace
            .case_render_document(&case_ref)
            .expect("Markdown collection render document");
        let notes_document = workspace
            .case_notes_render_document(&case_ref)
            .expect("notes render document");
        let draft_document = workspace
            .draft_render_document(&case_ref, &draft_name)
            .expect("draft render document");
        let model = case_review_document(&workspace, &case_ref);
        let render_value = serde_json::to_value(&model).expect("Review render document serializes");
        assert!(render_value.get("interaction_id").is_none());
        assert!(render_value.get("workspace_uid").is_none());
        assert!(render_value.get("subject_refs").is_none());
        assert!(render_value.get("base_hash").is_none());
        let page = render_page(&workspace, &model, TEST_NONCE).expect("case Review page renders");
        assert!(page.contains("This case is complete"));
        let ReviewContents::Case(contents) = model.contents else {
            panic!("case Review must contain case contents");
        };
        assert_eq!(contents.collection, markdown_document);
        assert_eq!(contents.notes.document, notes_document);
        assert_eq!(
            contents
                .drafts
                .iter()
                .map(|draft| &draft.document)
                .collect::<Vec<_>>(),
            vec![&draft_document]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn draft_review_contains_the_exact_draft_render_document() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("shared-draft-document");
        let document = workspace
            .draft_render_document(&case_ref, &draft_name)
            .expect("draft render document");
        let review_document = review_render_document(
            &workspace,
            ReviewSurfaceKind::Draft,
            ReviewContents::Draft(DraftReviewContents {
                draft: EditableReviewDocument {
                    base_hash: value_hash(&document).expect("draft document hash"),
                    document: document.clone(),
                },
            }),
        )
        .expect("draft Review document");
        let page = render_page(&workspace, &review_document, TEST_NONCE)
            .expect("draft Review page renders");
        assert!(page.contains("Review draft"));
        assert!(page.contains("Autosaves to this workspace."));
        assert!(page.contains("Quoted history and attachments remain unchanged."));
        assert!(!page.contains("Save draft locally"));
        assert!(!page.contains("<!-- afmail:compose:start -->"));
        let ReviewContents::Draft(contents) = review_document.contents else {
            panic!("draft Review must contain draft contents");
        };
        assert_eq!(contents.draft.document, document);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn triage_review_contains_the_exact_markdown_triage_document() {
        let (root, workspace, _, _) = temp_workspace("shared-triage-document");
        let raw = b"From: Alice <alice@example.com>\r\nTo: me@example.com\r\nSubject: Shared triage document\r\nDate: Mon, 20 Jul 2026 08:00:00 +0000\r\nMessage-ID: <shared-triage@example.com>\r\n\r\nShared body\r\n";
        let parsed = crate::mail::parse_inbound_message(
            "message_shared_triage".to_string(),
            raw,
            crate::types::ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 2,
            },
        )
        .expect("test message parses");
        workspace
            .write_message_artifacts(&parsed.message)
            .expect("test message artifacts write");
        let message_hash = workspace
            .message_review_base_hash(&parsed.message.message_id)
            .expect("message review hash");
        workspace
            .set_message_agent_suggestion(
                &parsed.message.message_id,
                &message_hash,
                MessageSuggestedAction::NotificationCreate {
                    notification_name: "contract-follow-up".to_string(),
                    summary: "Follow up on contract renewal".to_string(),
                },
                "A reply is needed",
                Some(90),
            )
            .expect("message suggestion");
        let markdown_document = workspace
            .triage_template_document(&parsed.message.message_id)
            .expect("Markdown triage render document");
        let review_document = review_render_document(
            &workspace,
            ReviewSurfaceKind::Triage,
            ReviewContents::Triage(TriageReviewContents {
                items: vec![markdown_document.clone()],
            }),
        )
        .expect("triage Review document");
        let page = render_page(&workspace, &review_document, TEST_NONCE)
            .expect("triage Review page renders");
        assert!(page.contains("Create notification"));
        assert!(page.contains("contract-follow-up"));
        assert!(page.contains("A reply is needed"));
        let ReviewContents::Triage(contents) = review_document.contents else {
            panic!("triage Review must contain triage contents");
        };
        assert_eq!(contents.items, vec![markdown_document]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn notification_review_contains_the_exact_markdown_collection_document() {
        let (root, workspace, _, _) = temp_workspace("shared-notification-document");
        let created = workspace
            .create_notification("Shared notification", None, None, None)
            .expect("notification creates");
        let notification_ref = created["notification_uid"]
            .as_str()
            .expect("notification uid");
        let notes_document = workspace
            .notification_notes_render_document(notification_ref)
            .expect("notification notes render document");
        let notification_hash = workspace
            .notification_review_base_hash(notification_ref)
            .expect("notification hash");
        workspace
            .set_notification_agent_suggestion(
                notification_ref,
                &notification_hash,
                NotificationSuggestedAction::NotificationRename {
                    notification_name: "Service notices".to_string(),
                },
                "Use a clearer collection name",
                None,
            )
            .expect("notification suggestion");
        let markdown_document = workspace
            .notification_render_document(notification_ref)
            .expect("updated Markdown notification render document");
        let review_document = review_render_document(
            &workspace,
            ReviewSurfaceKind::Notification,
            ReviewContents::Notification(NotificationReviewContents {
                collection: markdown_document.clone(),
                notes: EditableReviewDocument {
                    base_hash: value_hash(&notes_document).expect("notes document hash"),
                    document: notes_document,
                },
            }),
        )
        .expect("notification Review document");
        let page = render_page(&workspace, &review_document, TEST_NONCE)
            .expect("notification Review page renders");
        assert!(page.contains("Shared notification"));
        assert!(page.contains("Use a clearer collection name"));
        assert!(page.contains("Notification review"));
        let ReviewContents::Notification(contents) = review_document.contents else {
            panic!("notification Review must contain notification contents");
        };
        assert_eq!(contents.collection, markdown_document);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn case_page_is_server_rendered_from_typed_documents_and_only_accepts_listed_drafts() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("typed-page");
        let mut model = case_review_document(&workspace, &case_ref);
        let raw = b"From: Alice <alice@example.com>\r\nTo: me@example.com\r\nSubject: Readable <img src=x onerror=alert(1)>\r\nDate: Mon, 20 Jul 2026 08:00:00 +0000\r\nMessage-ID: <review@example.com>\r\n\r\nReadable human message <script>alert(1)</script>\r\n";
        let parsed = crate::mail::parse_inbound_message(
            "message_review_test".to_string(),
            raw,
            crate::types::ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 1,
            },
        )
        .expect("test message parses");
        std::fs::write(
            workspace
                .root()
                .join(".afmail/messages/message_review_test.eml"),
            raw,
        )
        .expect("raw test message writes");
        workspace
            .write_message_artifacts(&parsed.message)
            .expect("test message artifacts write");
        let mut message_document = workspace
            .triage_template_document(&parsed.message.message_id)
            .expect("triage render document")
            .context
            .message;
        let unsafe_body = "Readable human message <script>alert(1)</script>";
        message_document.message.subject =
            Some("Readable <img src=x onerror=alert(1)>".to_string());
        message_document.message.body_text = unsafe_body.to_string();
        message_document.analysis.body.segments =
            vec![crate::render_document::MessageBodySegment {
                kind: crate::render_document::MessageBodySegmentKind::Authored,
                start_bytes: 0,
                end_bytes: unsafe_body.len(),
            }];
        let ReviewContents::Case(contents) = &mut model.contents else {
            unreachable!();
        };
        contents.collection.messages.push(message_document);
        let page =
            render_page(&workspace, &model, TEST_NONCE).expect("typed Review template renders");
        assert!(page.contains("Notes"));
        assert!(page.contains("Reply"));
        assert!(page.contains("Readable human message"));
        assert!(page.contains("Readable human message &lt;script&gt;alert(1)&lt;"));
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(!page.contains("<img src=x onerror=alert(1)>"));
        assert!(page.contains("message-card"));
        assert!(!page.contains("<pre"));
        assert!(!page.contains("JSON.stringify"));
        assert!(!page.contains("document.createElement"));
        assert!(!page.contains("M.contents"));
        assert!(page.contains("Autosaves to this workspace"));
        assert!(
            page.contains("setTimeout(() =&gt; persist(form), 650)")
                || page.contains("setTimeout(() => persist(form), 650)")
        );
        assert!(page.contains("beforeunload"));
        assert!(page.contains("Quoted history and attachments remain unchanged."));
        // Each editor names the route its save posts to, and the page's own
        // runtime resolves that route against the session root. The single-path
        // `action=` form the host used to demultiplex is gone.
        assert!(page.contains(r#"data-afmail-autosave="notes""#));
        assert!(!page.contains(r#"name="action""#));
        assert!(page.contains("post(form.dataset.afmailAutosave"));
        assert!(page.contains("fetch(sessionRoot + route"));
        assert!(page.contains("data-afmail-submit-field"));

        assert!(
            saved_draft_target(
                &ReviewSurface::Case {
                    case_ref: case_ref.clone()
                },
                &model,
                &draft_name,
            )
            .is_ok()
        );
        assert_eq!(
            saved_draft_target(&ReviewSurface::Case { case_ref }, &model, "../other.md")
                .expect_err("unlisted draft must fail")
                .error_code,
            "review_draft_not_allowed"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_review_ready_progress_is_emitted_without_exposing_the_session_url() {
        let (root, workspace, case_ref, _) = temp_workspace("local-ready");
        let model = case_review_document(&workspace, &case_ref);
        let mut output = Vec::new();
        {
            let mut emitter = CliEmitter::new(&mut output, agent_first_data::OutputFormat::Json)
                .with_strict_protocol();
            emit_review_ready(
                &mut emitter,
                &model,
                ReviewMode::Local,
                None,
                None,
                Instant::now(),
            )
            .expect("local ready progress emits");
        }
        let event: Value = serde_json::from_slice(&output).expect("progress is JSON");
        assert!(agent_first_data::validate_protocol_event(&event, false).is_ok());
        assert_eq!(event["kind"], "progress");
        assert_eq!(event["progress"]["phase"], "review_ready");
        assert_eq!(event["progress"]["review_mode"], "local");
        assert_eq!(event["progress"]["surface_kind"], "case_review");
        assert!(event["progress"].get("review_url").is_none());
        // Nothing to say when the built-in UI is serving, so nothing is said.
        assert!(event["progress"].get("review_frontend_id").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A surface's `ui_kind` is the override key, and the list of kinds a
    /// person can install a frontend for is written down separately from the
    /// mapping. Neither is allowed to grow a name the other does not have.
    #[test]
    fn every_review_surface_has_exactly_one_installable_ui_kind() {
        let mapped = [
            ReviewSurfaceKind::Triage,
            ReviewSurfaceKind::Case,
            ReviewSurfaceKind::Notification,
            ReviewSurfaceKind::Draft,
        ]
        .map(host::review_ui_kind);
        let mut mapped = mapped.to_vec();
        mapped.sort_unstable();
        let mut declared = crate::review_frontend::UI_KINDS.to_vec();
        declared.sort_unstable();
        assert_eq!(mapped, declared);
    }

    #[test]
    fn notes_save_writes_continuations_and_stale_save_does_not_overwrite() {
        let (root, workspace, case_ref, _) = temp_workspace("notes-save");
        let first_hash = value_hash(
            &workspace
                .case_notes_render_document(&case_ref)
                .expect("notes render document"),
        )
        .expect("notes document hash");
        let first = save_notes_in_workspace(&workspace, &case_ref, "First note", &first_hash)
            .expect("notes save")
            .expect("notes must not be stale");
        assert_eq!(
            std::fs::read_to_string(root.join(&first.path)).expect("read notes"),
            "First note\n"
        );
        let second =
            save_notes_in_workspace(&workspace, &case_ref, "Second note", &first.base_hash)
                .expect("second notes save")
                .expect("updated hash must save");
        let before_stale = std::fs::read_to_string(root.join(&second.path)).expect("read notes");
        assert!(
            save_notes_in_workspace(&workspace, &case_ref, "Stale", &first_hash)
                .expect("stale notes result")
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.join(&second.path)).expect("read notes"),
            before_stale
        );
    }

    #[test]
    fn notes_save_becomes_stale_when_the_exact_document_path_changes() {
        let (root, workspace, case_ref, _) = temp_workspace("notes-path-stale");
        let initial = workspace
            .case_notes_render_document(&case_ref)
            .expect("initial notes render document");
        let initial_hash = value_hash(&initial).expect("initial notes document hash");

        workspace
            .rename_case(
                &case_ref,
                "Renamed review",
                Some("exercise review concurrency"),
            )
            .expect("case rename");
        let moved = workspace
            .case_notes_render_document(&case_ref)
            .expect("moved notes render document");
        assert_eq!(moved.text, initial.text);
        assert_ne!(moved.notes_path, initial.notes_path);

        let stale = save_notes_in_workspace(&workspace, &case_ref, "must not save", &initial_hash)
            .expect("stale notes result");
        assert_eq!(
            stale.expect_err("moved notes document must be stale"),
            value_hash(&moved).expect("moved notes document hash")
        );
        assert_eq!(
            workspace
                .case_notes_render_document(&case_ref)
                .expect("unchanged notes render document")
                .text,
            moved.text
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn draft_save_preserves_template_sections_and_stale_save_does_not_overwrite() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("draft-save");
        let initial = workspace
            .draft_render_document(&case_ref, &draft_name)
            .expect("draft render document");
        let initial_hash = value_hash(&initial).expect("draft hash");
        let initial_text =
            std::fs::read_to_string(root.join(&initial.locator.draft_path)).expect("read draft");
        let mut edit = DraftEdit {
            base_hash: initial_hash.clone(),
            draft_name: draft_name.clone(),
            subject: Some("Updated subject".to_string()),
            to: "\"Doe, Jane\" <jane@example.com>, customer@example.com".to_string(),
            cc: String::new(),
            bcc: "Hidden Reviewer <hidden@example.com>".to_string(),
            body: Some("Updated authored text".to_string()),
        };
        let saved = save_draft_in_workspace(&workspace, &case_ref, &draft_name, &edit)
            .expect("draft save")
            .expect("draft must not be stale");
        let updated_text =
            std::fs::read_to_string(root.join(&saved.path)).expect("read updated draft");
        assert!(updated_text.contains("Updated authored text"));
        assert!(updated_text.contains("<!-- afmail:compose:start -->"));
        assert!(updated_text.contains("<!-- afmail:compose:end -->"));
        assert!(initial_text.contains("<!-- afmail:compose:start -->"));
        let shown = workspace
            .show_draft(&case_ref, &draft_name)
            .expect("show parsed recipients");
        assert_eq!(
            shown["draft"]["frontmatter"]["to"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            shown["draft"]["frontmatter"]["to"][0],
            "\"Doe, Jane\" <jane@example.com>"
        );
        assert_eq!(
            shown["draft"]["frontmatter"]["bcc"][0],
            "Hidden Reviewer <hidden@example.com>"
        );

        edit.base_hash = saved.base_hash.clone();
        edit.to = String::new();
        edit.bcc = String::new();
        let error = save_draft_in_workspace(&workspace, &case_ref, &draft_name, &edit)
            .expect_err("empty recipient set must be rejected");
        assert_eq!(error.error_code, "draft_invalid");
        let unchanged_draft = workspace
            .show_draft(&case_ref, &draft_name)
            .expect("show unchanged recipients");
        assert_eq!(
            unchanged_draft["draft"]["frontmatter"]["to"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        let updated_text =
            std::fs::read_to_string(root.join(&saved.path)).expect("read unchanged draft");
        edit.base_hash = initial_hash;
        edit.to = "customer@example.com".to_string();
        edit.body = Some("Stale authored text".to_string());
        assert!(
            save_draft_in_workspace(&workspace, &case_ref, &draft_name, &edit)
                .expect("stale draft result")
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.join(&saved.path)).expect("read draft"),
            updated_text
        );
    }

    #[test]
    fn review_writes_refuse_an_incomplete_workspace_transaction() {
        let (root, workspace, case_ref, _) = temp_workspace("transaction-guard");
        let transaction_path = root.join(".afmail/transactions/transaction_review_test.json");
        let transaction = json!({
            "schema_name": "local_transaction",
            "schema_version": 1,
            "transaction_id": "transaction_review_test",
            "kind": "review_test",
            "created_rfc3339": "2026-07-21T00:00:00Z",
            "paths": ["cases/test/notes.md"]
        });
        std::fs::write(
            transaction_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&transaction).expect("serialize transaction")
            ),
        )
        .expect("write incomplete transaction");
        let base_hash = value_hash(
            &workspace
                .case_notes_render_document(&case_ref)
                .expect("notes render document"),
        )
        .expect("notes document hash");
        let error = save_notes_in_workspace(&workspace, &case_ref, "blocked", &base_hash)
            .expect_err("review write must be blocked");
        assert_eq!(error.error_code, "transaction_incomplete");
    }

    #[test]
    fn destructive_case_suggestion_requires_explicit_notes_confirmation() {
        let (root, workspace, case_ref, _) = temp_workspace("destructive-confirmation");
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                crate::types::CaseSuggestedAction::CaseDelete { allow_notes: true },
                "Remove the obsolete collection",
                None,
            )
            .expect("case suggestion");
        let model = case_review_document(&workspace, &case_ref);
        let values = HashMap::from([("case_decision".to_string(), "accept".to_string())]);
        let error = submitted_result(
            &ReviewSurface::Case { case_ref },
            &model,
            &ReviewSessionState::default(),
            &values,
        )
        .expect_err("missing destructive confirmation must fail");
        assert_eq!(error.error_code, "invalid_case_decision");
        assert!(
            render_page(&workspace, &model, TEST_NONCE)
                .expect("Review template renders")
                .contains("permanently delete")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A frontend the person installed, already resolved and trusted.
    ///
    /// Discovery, the trust gate and safe mode belong to AFUI and are driven
    /// end to end in `tests/review_frontend.rs`; these tests are about what
    /// afmail does with a frontend once it has one.
    fn frontend_at(root: &std::path::Path) -> agent_first_ui::FrontendLocation {
        std::fs::create_dir_all(root.join("templates/en-US/review"))
            .expect("frontend Review directory");
        agent_first_ui::FrontendLocation {
            scope: agent_first_ui::FrontendScope::Workspace,
            root: root.to_path_buf(),
            manifest: agent_first_ui::FrontendManifest {
                frontend_id: "test_review_ui".to_string(),
                ui_api_version: crate::review_frontend::UI_API_VERSION.to_string(),
            },
        }
    }

    #[test]
    fn custom_review_page_must_place_exactly_one_trusted_runtime_marker() {
        let (root, workspace, case_ref, _) = temp_workspace("trusted-runtime-marker");
        let model = case_review_document(&workspace, &case_ref);
        let frontend = frontend_at(&root.join("frontend"));
        let page_path = frontend.root.join("templates/en-US/review/page.html.j2");
        std::fs::write(
            &page_path,
            "<!doctype html><html><body><main>custom</main></body></html>",
        )
        .expect("custom Review page");
        let error = render_page_from(&workspace, &model, TEST_NONCE, Some(&frontend))
            .expect_err("Review page without a trusted runtime marker must fail");
        assert_eq!(error.error_code, "review_template_invalid");

        std::fs::write(
            &page_path,
            "<!doctype html><html><body><main>custom</main><!-- afmail:trusted-runtime --></body></html>",
        )
        .expect("custom Review page with marker");
        let page = render_page_from(&workspace, &model, TEST_NONCE, Some(&frontend))
            .expect("trusted runtime marker renders");
        // The page's script runs only because the session's own policy names
        // this nonce; a page rendered against any other value is inert.
        assert!(page.contains(&format!("<script nonce=\"{TEST_NONCE}\"")));
        assert!(!page.contains(TRUSTED_RUNTIME_MARKER));
        assert!(
            page.find("<script nonce=").unwrap_or_default()
                < page.find("</body>").unwrap_or_default()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A frontend supplies presentation. It does not get its own escaping
    /// rules, its own JavaScript, or a way to make the page say a save
    /// happened — everything the built-in UI is held to still holds.
    #[test]
    fn a_frontend_is_held_to_the_same_review_contract_as_the_builtin_ui() {
        let (root, workspace, case_ref, _) = temp_workspace("frontend-contract");
        let model = case_review_document(&workspace, &case_ref);
        let frontend = frontend_at(&root.join("frontend"));
        let page_path = frontend.root.join("templates/en-US/review/page.html.j2");

        std::fs::write(
            &page_path,
            "<html><body><script>fetch('/submit')</script><!-- afmail:trusted-runtime --></body></html>",
        )
        .expect("frontend Review page with JavaScript");
        let error = render_page_from(&workspace, &model, TEST_NONCE, Some(&frontend))
            .expect_err("a frontend may not ship JavaScript");
        assert_eq!(error.error_code, "review_template_unsafe");

        // Values a frontend prints are escaped by the same rules, so a case
        // subject cannot become markup just because the UI is custom.
        std::fs::write(
            &page_path,
            "<html><body><main>{{ contents.collection.collection.collection_name }}</main>\
             <!-- afmail:trusted-runtime --></body></html>",
        )
        .expect("frontend Review page");
        let page = render_page_from(&workspace, &model, TEST_NONCE, Some(&frontend))
            .expect("frontend Review page renders");
        assert!(page.contains("<main>Review</main>"), "{page}");
        let _ = std::fs::remove_dir_all(root);
    }
}
