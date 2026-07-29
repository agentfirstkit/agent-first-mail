//! Local, blocking review sessions. The page is deliberately a thin renderer:
//! it receives a fixed typed Review document and can only save notes or a draft
//! locally.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Child, Command as ProcessCommand};
use std::thread;
use std::time::{Duration, Instant};

use agent_first_data::{CliEmitter, CliEmitterError};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{Value, json};

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

fn run_session<W: Write>(
    workspace: &Workspace,
    surface: ReviewSurface,
    model: ReviewRenderDocument,
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let mut state = ReviewSessionState::default();
    let bind_host = if matches!(mode, ReviewMode::Lan) {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    let listener = TcpListener::bind((bind_host, 0))
        .map_err(|error| review_bind_error(mode, bind_host, &error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| AppError::io("configure local review server", &error))?;
    let address = listener
        .local_addr()
        .map_err(|error| AppError::io("read local review address", &error))?;
    let token = session_token()?;
    let path = format!("/{token}");
    let (expected_host, mut browser) = if matches!(mode, ReviewMode::Lan) {
        let lan_ip = primary_lan_ipv4()?;
        let expected_host = format!("{lan_ip}:{}", address.port());
        let review_url = format!("http://{expected_host}{path}");
        emit_review_ready(emitter, &model, mode, Some(&review_url), started)?;
        (expected_host, None)
    } else {
        let expected_host = address.to_string();
        let browser = open_app_window(&format!("http://{expected_host}{path}"))?;
        emit_review_ready(emitter, &model, mode, None, started)?;
        (expected_host, Some(browser))
    };
    let session_started = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                configure_review_stream(&stream)?;
                if let Some(result) = handle_request(
                    &mut stream,
                    workspace,
                    &surface,
                    &model,
                    &mut state,
                    &path,
                    &expected_host,
                )? {
                    return Ok(result);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(AppError::io("accept local review connection", &error).into());
            }
        }
        if let Some(browser) = browser.as_mut()
            && let Some(status) = browser
                .child
                .try_wait()
                .map_err(|error| AppError::io("wait for review window", &error))?
        {
            if status.success() {
                return Ok(review_envelope(
                    &model,
                    "dismissed",
                    &state,
                    ReviewPayload::Empty,
                )?);
            }
            return Err(browser_exit_error(status.code()).into());
        }
        if matches!(mode, ReviewMode::Lan)
            && session_started.elapsed() >= Duration::from_secs(LAN_REVIEW_EXPIRES_IN_S)
        {
            return Ok(review_envelope(
                &model,
                "dismissed",
                &state,
                ReviewPayload::Empty,
            )?);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn emit_review_ready<W: Write>(
    emitter: &mut CliEmitter<W>,
    model: &ReviewRenderDocument,
    mode: ReviewMode,
    review_url: Option<&str>,
    started: Instant,
) -> std::result::Result<(), CliEmitterError> {
    let progress = match review_url {
        Some(review_url) => json!({
            "phase": "review_ready",
            "message": "Open this short-lived review URL from a device on the trusted local network.",
            "review_url": review_url,
            "expires_in_s": LAN_REVIEW_EXPIRES_IN_S,
            "review_mode": review_mode_name(mode),
            "surface_kind": model.surface_kind,
            "subject_refs": &model.subject_refs,
        }),
        None => json!({
            "phase": "review_ready",
            "message": "The local review window is open.",
            "review_mode": review_mode_name(mode),
            "surface_kind": model.surface_kind,
            "subject_refs": &model.subject_refs,
        }),
    };
    emitter.emit(
        agent_first_data::json_progress(progress_event(progress))
            .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
            .build(),
    )
}

fn review_bind_error(mode: ReviewMode, bind_host: Ipv4Addr, error: &std::io::Error) -> AppError {
    AppError::retryable(
        "review_bind_failed",
        format!(
            "could not bind the {} review server on {bind_host}: {error}",
            review_mode_name(mode)
        ),
    )
    .with_hint(
        "Allow afmail to bind a local TCP listener and retry; in a sandbox, grant local listen/network permission or run this review command outside the sandbox.",
    )
    .with_details(json!({
        "review_mode": review_mode_name(mode),
        "bind_host": bind_host.to_string(),
    }))
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

fn configure_review_stream(stream: &TcpStream) -> Result<()> {
    // Accepted sockets can inherit the listener's nonblocking mode on some
    // platforms. Chrome may connect before sending request bytes, so restore
    // blocking reads and bound them with a timeout before parsing the request.
    stream
        .set_nonblocking(false)
        .map_err(|error| AppError::io("configure review connection blocking mode", &error))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| AppError::io("configure review read timeout", &error))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| AppError::io("configure review write timeout", &error))
}

fn handle_request(
    stream: &mut TcpStream,
    workspace: &Workspace,
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    state: &mut ReviewSessionState,
    path: &str,
    expected_host: &str,
) -> Result<Option<Value>> {
    let request = read_http_request(stream)?;
    let mut parts = request.splitn(2, "\r\n\r\n");
    let header = parts.next().unwrap_or_default();
    let body = parts.next().unwrap_or_default();
    let request_path = header.split_whitespace().nth(1).unwrap_or_default();
    let method = header.split_whitespace().next().unwrap_or_default();
    if method != "GET" && method != "POST" {
        write_response(
            stream,
            "405 Method Not Allowed",
            "text/plain",
            "method not allowed",
        )?;
        return Ok(None);
    }
    let host = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("Host:")
                .or_else(|| line.strip_prefix("host:"))
        })
        .map(str::trim)
        .unwrap_or_default();
    if host != expected_host {
        write_response(stream, "403 Forbidden", "text/plain", "invalid host")?;
        return Ok(None);
    }
    if method == "POST"
        && header
            .lines()
            .find_map(|line| {
                line.strip_prefix("Origin:")
                    .or_else(|| line.strip_prefix("origin:"))
            })
            .is_some_and(|origin| origin.trim() != format!("http://{expected_host}"))
    {
        write_response(stream, "403 Forbidden", "text/plain", "invalid origin")?;
        return Ok(None);
    }
    if request_path != path {
        write_response(stream, "404 Not Found", "text/plain", "not found")?;
        return Ok(None);
    }
    if header.starts_with("GET ") {
        let page = render_page(workspace, model)?;
        return write_review_response(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            &page.html,
            &page.nonce,
        )
        .map(|()| None);
    }
    let values = parse_form(body);
    let action = values.get("action").map(String::as_str).unwrap_or_default();
    match action {
        "save_notes" => {
            if matches!(
                surface,
                ReviewSurface::Case { .. } | ReviewSurface::Notification { .. }
            ) {
                match save_notes(
                    surface,
                    values.get("text").map(String::as_str).unwrap_or_default(),
                    values
                        .get("base_hash")
                        .map(String::as_str)
                        .unwrap_or_default(),
                )? {
                    Ok(saved_file) => {
                        let base_hash = saved_file.base_hash.clone();
                        state.saved_files.push(saved_file);
                        write_response(
                            stream,
                            "200 OK",
                            "application/json",
                            &json!({"saved":true,"base_hash":base_hash}).to_string(),
                        )?
                    }
                    Err(hash) => write_response(
                        stream,
                        "409 Conflict",
                        "application/json",
                        &json!({"saved":false,"code":"review_stale","base_hash":hash}).to_string(),
                    )?,
                }
            } else {
                write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"saved":false}""#,
                )?;
            }
            Ok(None)
        }
        "save_draft" => {
            match saved_draft_target(surface, model, &values) {
                Ok((case_ref, draft_name)) => match save_draft(&case_ref, &draft_name, &values) {
                    Ok(Ok(saved_file)) => {
                        let base_hash = saved_file.base_hash.clone();
                        state.saved_files.push(saved_file);
                        write_response(
                            stream,
                            "200 OK",
                            "application/json",
                            &json!({"saved":true,"base_hash":base_hash}).to_string(),
                        )?
                    }
                    Ok(Err(hash)) => write_response(
                        stream,
                        "409 Conflict",
                        "application/json",
                        &json!({"saved":false,"code":"review_stale","base_hash":hash}).to_string(),
                    )?,
                    Err(error) if review_input_error(&error) => write_response(
                        stream,
                        "400 Bad Request",
                        "application/json",
                        &json!({"saved":false,"code":error.error_code}).to_string(),
                    )?,
                    Err(error) => return Err(error),
                },
                Err(error) => write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"saved":false,"code":error.error_code}).to_string(),
                )?,
            }
            Ok(None)
        }
        "submit" => match submitted_result(surface, model, state, &values) {
            Ok(result) => {
                write_response(
                    stream,
                    "200 OK",
                    "application/json",
                    &json!({"submitted":true}).to_string(),
                )?;
                Ok(Some(result))
            }
            Err(error) => {
                write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"submitted":false,"code":error.error_code}).to_string(),
                )?;
                Ok(None)
            }
        },
        "cancel" => {
            write_response(
                stream,
                "200 OK",
                "application/json",
                &json!({"cancelled":true}).to_string(),
            )?;
            review_envelope(model, "cancelled", state, ReviewPayload::Empty).map(Some)
        }
        _ => {
            write_response(stream, "404 Not Found", "text/plain", "not found")?;
            Ok(None)
        }
    }
}

fn review_input_error(error: &AppError) -> bool {
    matches!(
        error.error_code,
        "invalid_request" | "draft_invalid" | "draft_incomplete" | "config_invalid"
    )
}

fn browser_exit_error(code: Option<i32>) -> AppError {
    let status = code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "a signal".to_string());
    AppError::new(
        "review_browser_failed",
        format!("local review window exited with {status}"),
    )
}

fn read_http_request(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|error| AppError::io("read review request", &error))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end + 4]);
            let method = header.split_whitespace().next().unwrap_or_default();
            let content_len = header
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                        .map(|value| value.trim().parse::<usize>())
                })
                .transpose()
                .map_err(|error| AppError::new("review_request_invalid", error.to_string()))?
                .unwrap_or(0);
            if method == "POST"
                && !header.lines().any(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                        .is_some()
                })
            {
                return Err(AppError::new(
                    "review_request_invalid",
                    "POST review requests require Content-Length",
                ));
            }
            if content_len > 2 * 1024 * 1024 {
                return Err(AppError::new(
                    "review_request_too_large",
                    "local review request exceeds 2 MiB",
                ));
            }
            if bytes.len() >= end + 4 + content_len {
                bytes.truncate(end + 4 + content_len);
                break;
            }
        }
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(AppError::new(
                "review_request_too_large",
                "local review request exceeds 2 MiB",
            ));
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn save_notes(
    surface: &ReviewSurface,
    text: &str,
    base_hash: &str,
) -> Result<std::result::Result<SavedFile, String>> {
    let workspace = current_workspace()?;
    match surface {
        ReviewSurface::Case { case_ref } => {
            save_notes_in_workspace(&workspace, case_ref, text, base_hash)
        }
        ReviewSurface::Notification { notification_ref } => {
            save_notification_notes_in_workspace(&workspace, notification_ref, text, base_hash)
        }
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
    values: &HashMap<String, String>,
) -> Result<std::result::Result<SavedFile, String>> {
    let workspace = current_workspace()?;
    save_draft_in_workspace(&workspace, case_ref, draft_name, values)
}

fn save_draft_in_workspace(
    workspace: &Workspace,
    case_ref: &str,
    draft_name: &str,
    values: &HashMap<String, String>,
) -> Result<std::result::Result<SavedFile, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.draft_render_document(case_ref, draft_name)?;
    let current_hash = value_hash(&current)?;
    if current_hash
        != values
            .get("base_hash")
            .map(String::as_str)
            .unwrap_or_default()
    {
        return Ok(Err(current_hash));
    }
    let to_text = values.get("to").map(String::as_str).unwrap_or_default();
    let cc_text = values.get("cc").map(String::as_str).unwrap_or_default();
    if to_text.trim().is_empty() {
        return Err(AppError::new("draft_invalid", "draft to is required"));
    }
    let to = crate::smtp_send::normalize_recipient_list_text(to_text, "to")?;
    let cc = crate::smtp_send::normalize_recipient_list_text(cc_text, "cc")?;
    let changed = workspace.change_draft(
        case_ref,
        draft_name,
        DraftChange {
            subject: values.get("subject").map(String::as_str),
            to: &to,
            cc: &cc,
            clear_cc: cc_text.trim().is_empty(),
            identity: None,
            body: values.get("body").map(String::as_str),
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
    values: &HashMap<String, String>,
) -> Result<(String, String)> {
    match surface {
        ReviewSurface::Draft {
            case_ref,
            draft_name,
        } => Ok((case_ref.clone(), draft_name.clone())),
        ReviewSurface::Case { case_ref } => {
            let requested = values
                .get("draft_name")
                .map(String::as_str)
                .unwrap_or_default();
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

fn submitted_result(
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    state: &ReviewSessionState,
    values: &HashMap<String, String>,
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

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'none'; style-src 'none'; connect-src 'self'; form-action 'self'; base-uri 'none'\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| AppError::io("write review response", &error))
}

fn write_review_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
    nonce: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'nonce-{nonce}'; style-src 'unsafe-inline'; img-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| AppError::io("write review response", &error))
}

fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|pair| {
            pair.split_once('=')
                .map(|(key, value)| (url_decode(key), url_decode(value)))
        })
        .collect()
}

fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                    out.push(hex);
                    index += 3;
                } else {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct ReviewBrowser {
    child: Child,
    profile: std::path::PathBuf,
}

impl Drop for ReviewBrowser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = remove_review_profile(&self.profile);
    }
}

fn remove_review_profile(profile: &std::path::Path) -> Result<()> {
    let temp_dir = std::env::temp_dir();
    let valid_name = profile
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("afmail-review-"));
    if profile.parent() != Some(temp_dir.as_path()) || !valid_name {
        return Err(AppError::new(
            "review_profile_invalid",
            "refusing to remove a non-review browser profile",
        ));
    }
    std::fs::remove_dir_all(profile)
        .map_err(|error| AppError::io("remove review browser profile", &error))
}

fn open_app_window(url: &str) -> Result<ReviewBrowser> {
    let profile = std::env::temp_dir().join(format!("afmail-review-{}", session_token()?));
    std::fs::create_dir_all(&profile)
        .map_err(|error| AppError::io("create review browser profile", &error))?;
    #[cfg(target_os = "macos")]
    {
        for binary in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            if let Ok(child) = ProcessCommand::new(binary)
                .arg(format!("--app={url}"))
                .arg("--no-first-run")
                .arg("--no-default-browser-check")
                .arg("--disable-background-mode")
                .arg(format!("--user-data-dir={}", profile.display()))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                return Ok(ReviewBrowser { child, profile });
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    for binary in ["google-chrome", "chromium", "chromium-browser"] {
        if let Ok(child) = ProcessCommand::new(binary)
            .arg(format!("--app={url}"))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-mode")
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            return Ok(ReviewBrowser { child, profile });
        }
    }
    #[allow(unreachable_code)]
    let _ = remove_review_profile(&profile);
    Err(AppError::new(
        "review_browser_not_found",
        "Chrome or Chromium is required for local review",
    ))
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
        interaction_id: session_token()?,
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

fn session_token() -> Result<String> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        AppError::new(
            "review_random_failed",
            "could not generate a local review session token",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Debug)]
struct RenderedReviewPage {
    html: String,
    nonce: String,
}

fn render_page(workspace: &Workspace, model: &ReviewRenderDocument) -> Result<RenderedReviewPage> {
    let config = MailConfig::load(workspace.root())?;
    let mut renderer = TemplateRenderer::new(workspace.root(), config.template_language());
    let mut html = renderer.render_with_options(
        TemplateKey::ReviewPage,
        model,
        TemplateRenderOptions {
            timezone_offset: Some(config.resolved_timezone_offset()),
            output_dir: None,
        },
    )?;
    let nonce = session_token()?;
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
    Ok(RenderedReviewPage { html, nonce })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewDraft;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        for outcome in ["submitted", "cancelled", "dismissed"] {
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
        let page = render_page(&workspace, &model)
            .expect("case Review page renders")
            .html;
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
        let page = render_page(&workspace, &review_document)
            .expect("draft Review page renders")
            .html;
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
        let page = render_page(&workspace, &review_document)
            .expect("triage Review page renders")
            .html;
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
        let page = render_page(&workspace, &review_document)
            .expect("notification Review page renders")
            .html;
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
        let page = render_page(&workspace, &model)
            .expect("typed Review template renders")
            .html;
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
        assert!(page.contains("data-afmail-autosave"));
        assert!(page.contains("data-afmail-submit-field"));

        let allowed = HashMap::from([(String::from("draft_name"), draft_name.clone())]);
        assert!(
            saved_draft_target(
                &ReviewSurface::Case {
                    case_ref: case_ref.clone()
                },
                &model,
                &allowed,
            )
            .is_ok()
        );
        let blocked = HashMap::from([(String::from("draft_name"), String::from("../other.md"))]);
        assert_eq!(
            saved_draft_target(&ReviewSurface::Case { case_ref }, &model, &blocked,)
                .expect_err("unlisted draft must fail")
                .error_code,
            "review_draft_not_allowed"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn browser_exit_status_distinguishes_normal_close_from_failure() {
        assert_eq!(
            browser_exit_error(Some(1)).error_code,
            "review_browser_failed"
        );
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn review_bind_failures_have_a_specific_code_and_actionable_hint() {
        let error = review_bind_error(
            ReviewMode::Local,
            Ipv4Addr::LOCALHOST,
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert_eq!(error.error_code, "review_bind_failed");
        assert!(error.retryable);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("sandbox"))
        );
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|details| details["review_mode"].as_str()),
            Some("local")
        );
    }

    #[test]
    fn accepted_review_stream_waits_for_delayed_request_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener must bind");
        let address = listener
            .local_addr()
            .expect("listener address must resolve");
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).expect("client must connect");
            thread::sleep(Duration::from_millis(50));
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .expect("request must write");
        });
        let (mut stream, _) = listener.accept().expect("server must accept");
        stream
            .set_nonblocking(true)
            .expect("test stream must become nonblocking");
        configure_review_stream(&stream).expect("review stream must become blocking");

        let request = read_http_request(&mut stream).expect("delayed request must be read");
        client.join().expect("client thread must finish");
        assert!(request.starts_with("GET / HTTP/1.1\r\n"));
    }

    #[test]
    fn review_profile_cleanup_only_removes_its_explicit_temp_child() {
        let profile =
            std::env::temp_dir().join(format!("afmail-review-test-{}", std::process::id()));
        std::fs::create_dir_all(&profile).expect("test profile must create");
        remove_review_profile(&profile).expect("review profile must remove");
        assert!(!profile.exists());
        assert!(remove_review_profile(std::env::temp_dir().as_path()).is_err());
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
        let mut values = HashMap::from([
            ("base_hash".to_string(), initial_hash.clone()),
            ("subject".to_string(), "Updated subject".to_string()),
            (
                "to".to_string(),
                "\"Doe, Jane\" <jane@example.com>, customer@example.com".to_string(),
            ),
            ("cc".to_string(), "".to_string()),
            ("body".to_string(), "Updated authored text".to_string()),
        ]);
        let saved = save_draft_in_workspace(&workspace, &case_ref, &draft_name, &values)
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

        values.insert("base_hash".to_string(), saved.base_hash);
        values.insert("to".to_string(), String::new());
        let error = save_draft_in_workspace(&workspace, &case_ref, &draft_name, &values)
            .expect_err("empty To must be rejected");
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
        values.insert("base_hash".to_string(), initial_hash);
        values.insert("body".to_string(), "Stale authored text".to_string());
        assert!(
            save_draft_in_workspace(&workspace, &case_ref, &draft_name, &values)
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
            render_page(&workspace, &model)
                .expect("Review template renders")
                .html
                .contains("permanently delete")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn custom_review_page_must_place_exactly_one_trusted_runtime_marker() {
        let (root, workspace, case_ref, _) = temp_workspace("trusted-runtime-marker");
        let model = case_review_document(&workspace, &case_ref);
        let review_root = root.join("templates/en-US/review");
        std::fs::create_dir_all(&review_root).expect("Review template directory");
        std::fs::write(
            review_root.join("page.html.j2"),
            "<!doctype html><html><body><main>custom</main></body></html>",
        )
        .expect("custom Review page");
        let error = render_page(&workspace, &model)
            .expect_err("Review page without a trusted runtime marker must fail");
        assert_eq!(error.error_code, "review_template_invalid");

        std::fs::write(
            review_root.join("page.html.j2"),
            "<!doctype html><html><body><main>custom</main><!-- afmail:trusted-runtime --></body></html>",
        )
        .expect("custom Review page with marker");
        let page = render_page(&workspace, &model).expect("trusted runtime marker renders");
        assert!(page.html.contains("<script nonce=\""));
        assert!(!page.html.contains(TRUSTED_RUNTIME_MARKER));
        assert!(
            page.html.find("<script nonce=").unwrap_or_default()
                < page.html.find("</body>").unwrap_or_default()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
