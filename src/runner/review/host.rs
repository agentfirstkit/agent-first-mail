//! The one place the review domain meets AFUI.
//!
//! AFUI owns delivery: the listener, the credential in the URL path, remote
//! attention and warning grace,
//! the security headers, the exact-`Host` and `Origin` checks, the isolated
//! browser window and its disposable profile, and the shutdown that follows.
//! This module owns only the things afmail can say back — a rendered page, a
//! hash-checked notes save, a hash-checked draft save, and a hash-checked
//! per-item settlement — and the summary it reads back from the workspace
//! when the session ends.
//!
//! The session hands nothing back through AFUI: there is no moment
//! "the review" completes, only items settling one at a time as a person
//! decides them, so its session type is `()`. AFUI's `Outcome<()>` therefore
//! has no reachable `Completed` here — only `Closed` (the window closed,
//! which is how every review ordinarily ends) and `Expired` (an unattended
//! `link` delivery lapsed after AFUI's warning grace).

use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};
use agent_first_ui::{
    Outcome, UiBlobPolicy, UiBlobRef, UiCall, UiCallError, UiCspNonce, UiDeliveryMode,
    UiPagePolicy, UiPageScript, UiSecurityPolicy, UiSession,
};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc as tokio_mpsc;

use super::{
    DraftEdit, NotesEdit, ReviewRenderDocument, ReviewRunResult, ReviewSessionState, ReviewSurface,
    ReviewSurfaceKind, SavedEdit, SettleFields, apply_saved_edit_to_model, emit_review_ready,
    render_page, render_page_from, review_envelope, save_draft, save_notes, saved_draft_target,
    settle_from_fields,
};
use crate::config::{MailConfig, ReviewSection};
use crate::error::{AppError, Result};
use crate::review_frontend::{review_app_icon, review_ui_frontend};
use crate::store::Workspace;
use crate::templates::safe_review_image_content_type;
use crate::types::{AttachmentRef, MessageFile};

/// The AFUI `ui_kind` for one review surface.
///
/// Each surface gets its own kind rather than a shared `"review"`, because
/// `ui_kind` is what a user frontend override is keyed by
/// (`.afui/frontends/<provider_id>/<ui_kind>/`). Collapsing all four into one
/// name would mean replacing the triage UI silently replaced the case, draft,
/// and notification UIs too. These are the same names the result envelope
/// already uses for `surface_kind`, so the two cannot drift.
pub(super) fn review_ui_kind(surface_kind: ReviewSurfaceKind) -> &'static str {
    match surface_kind {
        ReviewSurfaceKind::Triage => "triage_review",
        ReviewSurfaceKind::Case => "case_review",
        ReviewSurfaceKind::Notification => "notification_review",
        ReviewSurfaceKind::Draft => "draft_review",
    }
}

const CONTROLLER_INPUT_CAPACITY: usize = 16;
const REVIEW_PAGE_MEDIA_TYPE: &str = "text/html; charset=utf-8";
const MIN_REVIEW_BLOB_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ReviewUiAction {
    SaveNotes { edit: NotesEdit },
    SaveDraft { edit: DraftEdit },
    Settle { fields: SettleFields },
    Refresh,
    LoadImage { message_id: String, part_id: String },
    AgentMessage { message: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReviewUiReply {
    Saved { base_hash: String },
    Settlement { result: Value },
    Page { blob: UiBlobRef },
    Image { blob: UiBlobRef },
    AgentAccepted,
    AgentResponse { message: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReviewUiState {
    Changed,
}

type ReviewCall = UiCall<ReviewUiAction, ReviewUiReply>;

/// One JSON line written by the session controller to this review process.
///
/// This is afmail's bridge from its long-running CLI invocation to the local
/// AFUI controller handle. AFUI itself knows nothing about the line, the
/// response, or whether the Review frontend keeps a visible history.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ReviewControllerInput {
    ReviewResponse {
        message: String,
        #[serde(default)]
        refresh: bool,
    },
}

/// Run one review session to its single outcome.
///
/// The CLI is synchronous and AFUI is async, so the runtime starts and stops
/// here: the whole review is one blocking call from the runner's point of view.
pub(super) fn run_session<W: Write>(
    workspace: &Workspace,
    surface: ReviewSurface,
    model: ReviewRenderDocument,
    mode: Option<UiDeliveryMode>,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let review_config = MailConfig::load(workspace.root())?.review;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            AppError::new(
                "review_host_failed",
                format!("could not start the review session runtime: {error}"),
            )
        })?;
    runtime.block_on(host_session(
        workspace,
        surface,
        model,
        mode,
        review_config,
        emitter,
        started,
    ))
}

/// `--mode`, resolved: explicit > `AFUI_DELIVERY` > `window`.
///
/// Read once so the readiness event and AFUI's delivery plan name the same
/// resolved mode.  The plan's two-stage `start`/`wait` API exposes a link's
/// bearer URL at exactly that readiness boundary.
fn resolve_review_mode(explicit: Option<UiDeliveryMode>) -> Result<UiDeliveryMode> {
    UiDeliveryMode::resolve(explicit).map_err(|error| delivery_mode_error(&error))
}

fn delivery_mode_error(error: &agent_first_ui::Error) -> AppError {
    AppError::new("review_delivery_mode_invalid", error.to_string())
        .with_hint("Set AFUI_DELIVERY to one of window, link, session, or pass --mode explicitly.")
}

async fn host_session<W: Write>(
    workspace: &Workspace,
    surface: ReviewSurface,
    model: ReviewRenderDocument,
    mode: Option<UiDeliveryMode>,
    review_config: ReviewSection,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let mode = resolve_review_mode(mode)?;
    let nonce = UiCspNonce::generate().map_err(|error| ui_error(mode, &error))?;
    let ui_kind = review_ui_kind(model.surface_kind);
    // Resolved before anything is served so that a frontend afmail cannot load
    // fails the command outright, with the reason on the Agent's stream, rather
    // than opening a window onto an error page nobody asked for.
    let frontend = review_ui_frontend(workspace.root(), ui_kind)?;
    let frontend_id = frontend.frontend_id().map(str::to_owned);
    let app_icon = review_app_icon(&frontend, ui_kind)?;
    // And rendered once, for the same reason. Resolving only proves the
    // frontend can be *read*; a page that will not render, or that dropped a
    // hook the runtime binds through, fails no earlier than the browser's
    // first request — which is a window onto an error page, and a command
    // that reports a review nobody could do. Rendering stays per-request
    // after this: editing a trusted frontend mid-review must still revoke it.
    render_page_from(workspace, &model, nonce.as_str(), &frontend)?;
    // `T = ()` only ever means "nothing is handed back through completion"
    // (see the module doc), not "nobody is waiting". Every item on this page
    // is unresolved until a person settles it, so this session is
    // unmistakably waiting on a person the same way a `T != ()` one would be
    // — `afui session list` needs to say so explicitly rather than infer it
    // from a type that carries no value either way.
    let mut session = UiSession::<()>::new("afmail", ui_kind)
        .map_err(|error| ui_error(mode, &error))?
        .awaiting_person()
        .with_app_icon(app_icon)
        .with_security_policy(review_security_policy(&nonce));
    // What this review is about, so a person scanning `afui session list` can
    // tell two open case reviews apart. Identity, not presentation.
    if let Some(subject) = model.subject_refs.first().cloned() {
        session = session.with_subject(subject);
    }
    let (session, runtime) = session
        .with_runtime_and_blobs::<ReviewUiAction, ReviewUiReply, ReviewUiState>(review_blob_policy(
            review_config
                .max_inline_image_bytes
                .max(MIN_REVIEW_BLOB_BYTES),
        ))
        .map_err(|error| ui_error(mode, &error))?;
    let state = ReviewState {
        inner: Arc::new(ReviewSession {
            workspace: workspace.clone(),
            surface,
            model: Mutex::new(model),
            nonce: nonce.as_str().to_owned(),
            max_inline_image_bytes: review_config.max_inline_image_bytes,
            saved: Mutex::new(ReviewSessionState::default()),
        }),
    };
    // The state outlives the session so that the summary — read back from
    // the workspace, not carried out of the session — can still be built for
    // every outcome, including one nobody was watching for.
    let ended = Arc::clone(&state.inner);
    runtime
        .publish_state(&ReviewUiState::Changed)
        .map_err(|error| ui_error(mode, &error))?;
    let router = review_router(state.clone());

    // The same offer the `--mode` flag was built from, so a word a person read
    // in `--help` is a word this honours.
    let plan = crate::cli::REVIEW_DELIVERY
        .resolve(Some(mode))
        .map_err(|error| ui_error(mode, &error))?;
    let active = plan
        .start(session, router)
        .await
        .map_err(|error| ui_error(mode, &error))?;
    ended.with_model(|model| {
        emit_review_ready(
            emitter,
            model,
            mode,
            active.link_url_secret(),
            active.attention_policy(),
            frontend_id.as_deref(),
            started,
        )
    })?;

    let mut controller_lines = controller_input_lines()?;
    let mut controller_open = true;
    let mut runtime_open = true;
    let mut pending_agent: Option<ReviewCall> = None;
    let wait = active.wait();
    tokio::pin!(wait);
    let outcome = loop {
        tokio::select! {
            outcome = &mut wait => {
                break outcome.map_err(|error| ui_error(mode, &error))?;
            }
            call = runtime.recv(), if runtime_open => {
                let call = match call {
                    Ok(Some(call)) => call,
                    Ok(None) | Err(_) => {
                        runtime_open = false;
                        continue;
                    }
                };
                if let ReviewUiAction::AgentMessage { message } = call.action() {
                    if pending_agent.is_some() {
                        let _ignored = call.fail(UiCallError::new(
                            "review_agent_busy",
                            "Wait for the current Review response before sending another message.",
                        )).await;
                        continue;
                    }
                    if let Some(reason) = invalid_review_message(
                        message,
                        review_config.max_channel_text_bytes,
                    ) {
                        let _ignored = call.fail(UiCallError::new(
                            "review_input_invalid",
                            reason,
                        )).await;
                        continue;
                    }
                    if call.progress(&ReviewUiReply::AgentAccepted).await.is_err() {
                        runtime_open = false;
                        continue;
                    }
                    ended.with_model(|model| {
                        emit_review_input(emitter, model, message, started)
                    })?;
                    pending_agent = Some(call);
                    continue;
                }

                match execute_review_action(&state.inner, &call) {
                    Ok(reply) => {
                        if call.finish(&reply).await.is_err() {
                            runtime_open = false;
                        }
                    }
                    Err(failure) => {
                        if call.fail(failure).await.is_err() {
                            runtime_open = false;
                        }
                    }
                }
            }
            line = controller_lines.recv(), if controller_open => {
                let Some(line) = line else {
                    controller_open = false;
                    continue;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let response = match serde_json::from_str::<ReviewControllerInput>(&line) {
                    Ok(response) => response,
                    Err(error) => {
                        emit_review_controller_warning(
                            emitter,
                            "review_controller_input_invalid",
                            &format!("ignored invalid Review controller input: {error}"),
                            started,
                        )?;
                        fail_pending_agent(&mut pending_agent, "review_controller_input_invalid").await;
                        continue;
                    }
                };
                let ReviewControllerInput::ReviewResponse {
                    message,
                    refresh,
                } = response;
                if let Some(reason) = invalid_review_message(
                    &message,
                    review_config.max_channel_text_bytes,
                ) {
                    emit_review_controller_warning(
                        emitter,
                        "review_controller_input_invalid",
                        &reason,
                        started,
                    )?;
                    fail_pending_agent(&mut pending_agent, "review_controller_input_invalid").await;
                    continue;
                }
                let Some(call) = pending_agent.take() else {
                    emit_review_controller_warning(
                        emitter,
                        "review_controller_response_unexpected",
                        "ignored a Review controller response because no UI request is waiting",
                        started,
                    )?;
                    continue;
                };
                // The state says *that* the review changed and the page rereads
                // it; the reply carries only what the agent said back. Saying
                // it in both places would need the two to arrive in a
                // particular order, and AFUI does not order a state against a
                // call result — it would be a race the page could not lose
                // visibly enough to ever be reported.
                if refresh {
                    runtime
                        .publish_state(&ReviewUiState::Changed)
                        .map_err(|error| ui_error(mode, &error))?;
                }
                let _ignored = call
                    .finish(&ReviewUiReply::AgentResponse { message })
                    .await;
            }
        }
    };

    Ok(ended.with_model(|model| {
        ended.with_state(|state| {
            ended_envelope(&ended.workspace, &ended.surface, model, state, outcome)
        })
    })?)
}

fn controller_input_lines() -> Result<tokio_mpsc::Receiver<String>> {
    let (sender, receiver) = tokio_mpsc::channel(CONTROLLER_INPUT_CAPACITY);
    std::thread::Builder::new()
        .name("afmail-review-controller-input".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut reader = stdin.lock();
            let mut line = String::new();
            loop {
                line.clear();
                let Ok(read) = reader.read_line(&mut line) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                let value = line.trim_end_matches(['\r', '\n']).to_string();
                if sender.blocking_send(value).is_err() {
                    return;
                }
            }
        })
        .map_err(|error| {
            AppError::new(
                "review_controller_input_failed",
                format!("could not start the Review controller input reader: {error}"),
            )
        })?;
    Ok(receiver)
}

async fn fail_pending_agent(pending: &mut Option<ReviewCall>, code: &'static str) {
    let Some(call) = pending.take() else {
        return;
    };
    let _ignored = call
        .fail(UiCallError::new(
            code,
            "The Review controller could not complete this request.",
        ))
        .await;
}

fn invalid_review_message(message: &str, max_channel_text_bytes: u64) -> Option<String> {
    if message.trim().is_empty() {
        return Some("message must not be empty".to_string());
    }
    if message.len() as u64 > max_channel_text_bytes {
        return Some(format!(
            "message is {} bytes, exceeding the {}-byte Review limit",
            message.len(),
            max_channel_text_bytes
        ));
    }
    None
}

fn emit_review_input<W: Write>(
    emitter: &mut CliEmitter<W>,
    model: &ReviewRenderDocument,
    instruction: &str,
    started: Instant,
) -> std::result::Result<(), CliEmitterError> {
    emitter.emit(
        agent_first_data::json_progress(super::progress_event(json!({
            "phase": "review_input",
            "message": "The person sent an instruction from the open Review UI.",
            "interaction_id": model.interaction_id,
            "surface_kind": model.surface_kind,
            "subject_refs": model.subject_refs,
            "instruction": instruction,
        })))
        .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
        .build(),
    )
}

fn emit_review_controller_warning<W: Write>(
    emitter: &mut CliEmitter<W>,
    code: &str,
    message: &str,
    started: Instant,
) -> std::result::Result<(), CliEmitterError> {
    emitter.emit(
        agent_first_data::json_log(json!({
            "level": "warn",
            "message": message,
            "phase": "review_controller_input",
            "code": code,
        }))
        .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
        .build(),
    )
}

/// Build the terminal result for however the session ended.
///
/// There is only one path: reread the workspace. `Outcome<()>` has no
/// reachable `Completed` — nothing in this module ever calls a completion
/// handle — so `Closed` and `Expired` are the only two cases, and both need
/// exactly the same rereading `review_envelope` already does.
pub(super) fn ended_envelope(
    workspace: &Workspace,
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    state: &ReviewSessionState,
    outcome: Outcome<()>,
) -> Result<Value> {
    let ended = match outcome {
        Outcome::Closed => "closed",
        Outcome::Expired => "expired",
        Outcome::Completed(()) => {
            unreachable!("no route in this module ever completes the session")
        }
    };
    review_envelope(workspace, surface, model, ended, state)
}

/// One review session's shared state, cloned into every handler.
#[derive(Clone)]
struct ReviewState {
    inner: Arc<ReviewSession>,
}

struct ReviewSession {
    workspace: Workspace,
    surface: ReviewSurface,
    model: Mutex<ReviewRenderDocument>,
    nonce: String,
    max_inline_image_bytes: u64,
    saved: Mutex<ReviewSessionState>,
}

impl ReviewSession {
    fn with_model<T>(&self, read: impl FnOnce(&ReviewRenderDocument) -> T) -> T {
        let model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&model)
    }

    fn refresh_model(&self) -> Result<()> {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let refreshed =
            super::refreshed_review_render_document(&self.workspace, &self.surface, &model)?;
        *model = refreshed;
        Ok(())
    }

    /// Read the saved-file list back, whoever is asking and whenever.
    ///
    /// A review reports every file it wrote even when it ends without
    /// anyone ever asking for the terminal result, so this has to work
    /// after the session is over.
    fn with_state<T>(&self, read: impl FnOnce(&mut ReviewSessionState) -> T) -> T {
        let mut state = self
            .saved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&mut state)
    }
}

/// The rendered document is the Provider's only route.
///
/// Typed actions and temporary byte bodies live under AFUI's reserved runtime
/// paths, so this router cannot grow a second Review protocol by accident.
fn review_router(state: ReviewState) -> Router {
    Router::new().route("/", get(page)).with_state(state)
}

/// The page's own script is the only script it may run, and it may talk to
/// nothing but this session.
///
/// The stylesheet is inlined into the layout, images are AFUI-managed session
/// blobs, and every interaction travels over the AFUI runtime. Forms are
/// deliberately not authorized to submit on their own.
fn review_security_policy(nonce: &UiCspNonce) -> UiSecurityPolicy {
    UiPagePolicy::new(UiPageScript::Nonce(nonce.clone()))
        .allow_inline_styles()
        .allow_images()
        .allow_runtime()
        .into_security_policy()
}

async fn page(State(state): State<ReviewState>) -> Response {
    if let Err(error) = state.inner.refresh_model() {
        return (StatusCode::INTERNAL_SERVER_ERROR, error.error_code).into_response();
    }
    match state
        .inner
        .with_model(|model| render_page(&state.inner.workspace, model, &state.inner.nonce))
    {
        Ok(html) => Html(html).into_response(),
        // The page is reread and rendered per request because canonical
        // workspace state and a workspace template can both change under a
        // live session. A broken template is the person's to fix, and the code
        // names which template rule it broke.
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.error_code).into_response(),
    }
}

fn execute_review_action(
    session: &ReviewSession,
    call: &ReviewCall,
) -> std::result::Result<ReviewUiReply, UiCallError> {
    match call.action() {
        ReviewUiAction::SaveNotes { edit } => match save_notes(&session.surface, edit) {
            Ok(Ok(saved)) => record_saved(session, saved).map_err(app_call_error),
            Ok(Err(_base_hash)) => Err(stale_call_error()),
            Err(error) => Err(app_call_error(error)),
        },
        ReviewUiAction::SaveDraft { edit } => {
            let (case_ref, draft_name) = session
                .with_model(|model| saved_draft_target(&session.surface, model, &edit.draft_name))
                .map_err(app_call_error)?;
            match save_draft(&case_ref, &draft_name, edit) {
                Ok(Ok(saved)) => record_saved(session, saved).map_err(app_call_error),
                Ok(Err(_base_hash)) => Err(stale_call_error()),
                Err(error) => Err(app_call_error(error)),
            }
        }
        ReviewUiAction::Settle { fields } => {
            match session.with_model(|model| settle_from_fields(&session.surface, model, fields)) {
                Ok(result) => Ok(ReviewUiReply::Settlement { result }),
                Err(error) if settlement_was_persisted(&error) => Ok(ReviewUiReply::Settlement {
                    result: json!({
                        "settled": true,
                        "repair_required": true,
                        "code": error.error_code,
                        "human_suggestion_uid": error
                            .details
                            .as_ref()
                            .and_then(|details| details["human_suggestion_uid"].as_str()),
                    }),
                }),
                Err(error) => Err(app_call_error(error)),
            }
        }
        ReviewUiAction::Refresh => {
            session.refresh_model().map_err(app_call_error)?;
            let html = session
                .with_model(|model| render_page(&session.workspace, model, &session.nonce))
                .map_err(app_call_error)?;
            let blob = call
                .store_blob(REVIEW_PAGE_MEDIA_TYPE, html.into_bytes())
                .map_err(blob_call_error)?;
            Ok(ReviewUiReply::Page { blob })
        }
        ReviewUiAction::LoadImage {
            message_id,
            part_id,
        } => {
            let image = review_image(session, message_id, part_id)
                .map_err(app_call_error)?
                .ok_or_else(|| {
                    UiCallError::new(
                        "review_image_not_found",
                        "This Review image is not available.",
                    )
                })?;
            let blob = call
                .store_blob(image.content_type, image.bytes)
                .map_err(blob_call_error)?;
            Ok(ReviewUiReply::Image { blob })
        }
        ReviewUiAction::AgentMessage { .. } => Err(UiCallError::new(
            "review_runtime_error",
            "The Review message was routed incorrectly.",
        )),
    }
}

fn record_saved(session: &ReviewSession, saved: SavedEdit) -> Result<ReviewUiReply> {
    let base_hash = saved.file.base_hash.clone();
    {
        let mut model = session
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_saved_edit_to_model(&session.surface, &mut model, &saved)?;
    }
    session.with_state(|state| state.saved_files.push(saved.file));
    Ok(ReviewUiReply::Saved { base_hash })
}

fn stale_call_error() -> UiCallError {
    UiCallError::new(
        "review_stale",
        "This Review item changed after it was shown. Refresh it before retrying.",
    )
}

fn app_call_error(error: AppError) -> UiCallError {
    UiCallError::new(
        error.error_code,
        "The Review action could not be completed.",
    )
    .retryable(error.retryable)
}

fn blob_call_error(_error: agent_first_ui::Error) -> UiCallError {
    UiCallError::new(
        "review_blob_unavailable",
        "The Review content could not be retained for this session.",
    )
}

struct ReviewImage {
    bytes: Vec<u8>,
    content_type: &'static str,
}

/// Every image type a Review page may load, canonicalised.
///
/// `safe_review_image_content_type` maps what an actual message declared onto
/// one of these, which is where `image/jpg` becomes `image/jpeg` and a
/// `; charset=` parameter is dropped. This list is what AFUI then holds the
/// response to.
const REVIEW_IMAGE_MEDIA_TYPES: [&str; 7] = [
    "image/avif",
    "image/bmp",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/x-icon",
];

/// What a Review page may load as an inline image, and how much of it.
///
/// The allowlist is afmail's because only afmail knows what an attachment in a
/// message can be; the bound is the workspace's `max_inline_image_bytes`. What
/// happens once both are satisfied — `no-store`, `nosniff`, `inline`, and a
/// refusal that looks the same whatever the reason — is AFUI's.
fn image_policy(max_inline_image_bytes: u64) -> UiBlobPolicy {
    UiBlobPolicy::new(max_inline_image_bytes, REVIEW_IMAGE_MEDIA_TYPES)
}

/// Everything afmail may hand to AFUI for this Review session.
///
/// Attachment reads remain constrained by `image_policy`; the extra media
/// type here is only for AFUI-managed replacements of the already rendered
/// Review document.
fn review_blob_policy(max_blob_bytes: u64) -> UiBlobPolicy {
    UiBlobPolicy::new(
        max_blob_bytes,
        REVIEW_IMAGE_MEDIA_TYPES
            .into_iter()
            .chain([REVIEW_PAGE_MEDIA_TYPE]),
    )
}

fn review_image(
    session: &ReviewSession,
    message_id: &str,
    part_id: &str,
) -> Result<Option<ReviewImage>> {
    session.with_model(|model| {
        let Some(message) = review_message(model, message_id) else {
            return Ok(None);
        };
        let Some(attachment) = message
            .attachments
            .iter()
            .find(|attachment| attachment.part_id == part_id)
        else {
            return Ok(None);
        };
        let Some(content_type) = safe_review_image_content_type(&attachment.content_type) else {
            return Ok(None);
        };
        let Some(bytes) = review_image_bytes(
            &session.workspace,
            message,
            attachment,
            session.max_inline_image_bytes,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(ReviewImage {
            bytes,
            content_type,
        }))
    })
}

fn review_message<'a>(
    model: &'a ReviewRenderDocument,
    message_id: &str,
) -> Option<&'a MessageFile> {
    match &model.contents {
        super::ReviewContents::Triage(contents) => contents
            .items
            .iter()
            .map(|item| &item.document.context.message.message)
            .find(|message| message.message_id == message_id),
        super::ReviewContents::Case(contents) => contents
            .collection
            .messages
            .iter()
            .map(|document| &document.message)
            .find(|message| message.message_id == message_id),
        super::ReviewContents::Notification(contents) => contents
            .collection
            .messages
            .iter()
            .map(|document| &document.message)
            .find(|message| message.message_id == message_id),
        super::ReviewContents::Draft(_) => None,
    }
}

/// Where an attachment's bytes actually are: its own fetched file, or the
/// message that carries it.
///
/// The bounded, link-refusing read of each is AFUI's; what is afmail's is
/// knowing that a message id is a path segment here, that an attachment may
/// have been fetched beside its message, and that the fallback is a MIME part
/// inside the `.eml`.
fn review_image_bytes(
    workspace: &Workspace,
    message: &MessageFile,
    attachment: &AttachmentRef,
    max_inline_image_bytes: u64,
) -> Result<Option<Vec<u8>>> {
    if attachment.size_bytes > max_inline_image_bytes {
        return Ok(None);
    }
    // A message id becomes a path segment below, so it has to be one: AFUI
    // refuses `..` and a root, but an id carrying a separator would still
    // widen the path by a directory.
    if !message
        .message_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Ok(None);
    }
    let policy = image_policy(max_inline_image_bytes);

    if let Some(file_path) = attachment.file_path.as_deref() {
        let relative = Path::new(file_path);
        let fetched_root =
            PathBuf::from(".afmail/messages").join(format!("{}.files", message.message_id));
        if let Ok(filename) = relative.strip_prefix(&fetched_root)
            && filename.components().count() == 1
            && matches!(filename.components().next(), Some(Component::Normal(_)))
            && let Some(bytes) = policy.read(workspace.root(), relative)
        {
            return Ok(Some(bytes));
        }
    }

    let eml_path = PathBuf::from(".afmail/messages").join(format!("{}.eml", message.message_id));
    let Some(raw) = policy.read(workspace.root(), &eml_path) else {
        return Ok(None);
    };
    // One part out of a container AFUI has no reason to understand. The bound
    // applies again to what came out of it, which `respond` enforces.
    Ok(Some(crate::mail::attachment_bytes(
        &raw,
        &attachment.part_id,
    )?))
}

pub(super) fn settlement_was_persisted(error: &AppError) -> bool {
    error
        .details
        .as_ref()
        .and_then(|details| details["settlement_persisted"].as_bool())
        .unwrap_or(false)
}

/// Translate an AFUI delivery failure into an afmail error an Agent can act on.
fn ui_error(mode: UiDeliveryMode, error: &agent_first_ui::Error) -> AppError {
    use agent_first_ui::Error;
    match error {
        Error::WindowBinaryNotFound => AppError::new(
            "review_browser_not_found",
            format!("{error}; Chrome, Chromium, or Edge is required for local review"),
        ),
        Error::WindowLaunch { .. } | Error::WindowWait(_) => {
            AppError::new("review_browser_failed", error.to_string())
        }
        Error::LinkAddressUnavailable(_) => AppError::new(
            "review_link_unavailable",
            format!("could not select an IPv4 address for link review: {error}"),
        )
        .with_hint("Connect this machine to a trusted IPv4 local network and retry."),
        Error::Io(_) => AppError::retryable(
            "review_bind_failed",
            format!(
                "could not host the {} review session: {error}",
                mode.as_str()
            ),
        )
        .with_hint(
            "Allow afmail to bind a local TCP listener and retry; in a sandbox, grant local listen/network permission or run this review command outside the sandbox.",
        )
        .with_details(json!({"review_mode": mode.as_str()})),
        _ => AppError::new(
            "review_host_failed",
            format!(
                "could not start the {} review session: {error}",
                mode.as_str()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `AFUI_DELIVERY` is process-wide, so every test that points it at a
    // fixed value takes this first — mirroring `agent_first_ui`'s own
    // `ENV_LOCK` for the same variable.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_missing_browser_and_a_refused_listener_are_different_problems() {
        let missing = ui_error(
            UiDeliveryMode::Window,
            &agent_first_ui::Error::WindowBinaryNotFound,
        );
        assert_eq!(missing.error_code, "review_browser_not_found");
        assert!(!missing.retryable);

        let refused = ui_error(
            UiDeliveryMode::Window,
            &agent_first_ui::Error::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        );
        assert_eq!(refused.error_code, "review_bind_failed");
        assert!(refused.retryable);
        assert!(
            refused
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("sandbox"))
        );
        assert_eq!(
            refused
                .details
                .as_ref()
                .and_then(|details| details["review_mode"].as_str()),
            Some("window")
        );
    }

    #[test]
    fn a_missing_link_address_has_an_actionable_link_error() {
        let error = ui_error(
            UiDeliveryMode::Link,
            &agent_first_ui::Error::LinkAddressUnavailable(std::io::Error::from(
                std::io::ErrorKind::AddrNotAvailable,
            )),
        );
        assert_eq!(error.error_code, "review_link_unavailable");
        assert!(!error.retryable);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("trusted IPv4 local network"))
        );
    }

    #[test]
    fn an_explicit_mode_always_wins_over_afui_delivery() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // SAFETY: serialised by `ENV_LOCK`.
        unsafe { std::env::set_var(agent_first_ui::DELIVERY_ENV, "session") };
        let resolved = resolve_review_mode(Some(UiDeliveryMode::Window));
        unsafe { std::env::remove_var(agent_first_ui::DELIVERY_ENV) };
        assert_eq!(resolved.expect("resolves"), UiDeliveryMode::Window);
    }

    #[test]
    fn an_absent_mode_falls_back_to_afui_delivery() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // SAFETY: serialised by `ENV_LOCK`.
        unsafe { std::env::set_var(agent_first_ui::DELIVERY_ENV, "link") };
        let from_env = resolve_review_mode(None);
        unsafe { std::env::remove_var(agent_first_ui::DELIVERY_ENV) };
        assert_eq!(from_env.expect("resolves"), UiDeliveryMode::Link);
    }

    #[test]
    fn an_absent_mode_and_env_defaults_to_window() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // SAFETY: serialised by `ENV_LOCK`.
        unsafe { std::env::remove_var(agent_first_ui::DELIVERY_ENV) };
        let default = resolve_review_mode(None);
        assert_eq!(default.expect("resolves"), UiDeliveryMode::Window);
    }

    #[test]
    fn an_unrecognised_afui_delivery_value_is_rejected_not_defaulted() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // SAFETY: serialised by `ENV_LOCK`.
        unsafe { std::env::set_var(agent_first_ui::DELIVERY_ENV, "lan") };
        let error = resolve_review_mode(None).expect_err("an unknown word is not window");
        unsafe { std::env::remove_var(agent_first_ui::DELIVERY_ENV) };
        assert_eq!(error.error_code, "review_delivery_mode_invalid");
        assert!(error.message.contains("window"), "{}", error.message);
        assert!(error.message.contains("link"), "{}", error.message);
        assert!(error.message.contains("session"), "{}", error.message);
    }

    #[test]
    fn the_page_policy_authorizes_only_this_session_nonce() {
        let nonce = UiCspNonce::generate().expect("nonce generates");
        let policy = review_security_policy(&nonce);
        let mut headers = axum::http::HeaderMap::new();
        policy.apply_headers(&mut headers);
        let header = headers[axum::http::header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("policy is header-safe")
            .to_owned();
        assert!(header.contains(&format!("script-src 'nonce-{}'", nonce.as_str())));
        assert!(header.contains("default-src 'none'"));
        assert!(header.contains("connect-src 'self'"));
    }

    #[test]
    fn review_message_is_bounded_and_nonempty() {
        const MAX_BYTES: u64 = 64;
        assert!(invalid_review_message("Check this message", MAX_BYTES).is_none());
        assert!(invalid_review_message("  \n", MAX_BYTES).is_some());
        assert!(invalid_review_message(&"x".repeat(MAX_BYTES as usize + 1), MAX_BYTES).is_some());
    }

    #[test]
    fn review_image_declared_size_is_checked_before_reading_workspace_files() {
        const MAX_BYTES: u64 = 128;
        let root = tempfile::tempdir().expect("temporary Review workspace");
        let workspace = Workspace::at(root.path());
        let message: MessageFile = serde_json::from_value(json!({
            "schema_name": "message",
            "schema_version": crate::types::MESSAGE_SCHEMA_VERSION,
            "message_id": "message_large_image",
            "attachments": [{
                "part_id": "2",
                "filename": "large.png",
                "content_type": "image/png",
                "size_bytes": MAX_BYTES + 1
            }],
            "workspace": {"status": "triage"}
        }))
        .expect("Review message fixture parses");

        assert!(
            review_image_bytes(&workspace, &message, &message.attachments[0], MAX_BYTES)
                .expect("oversized declared image is handled")
                .is_none()
        );
    }

    #[test]
    fn controller_responses_are_strict_json_lines_and_refresh_is_explicit() {
        let input: ReviewControllerInput =
            serde_json::from_str(r#"{"type":"review_response","message":"Draft updated"}"#)
                .expect("controller response parses");
        let ReviewControllerInput::ReviewResponse { refresh, .. } = input;
        assert!(!refresh);

        assert!(
            serde_json::from_str::<ReviewControllerInput>(
                r#"{"type":"review_response","message":"Done","refresh":true,"suggestion":{"kind":"spam"}}"#,
            )
            .is_err(),
            "the controller response cannot inject a rendered suggestion"
        );
    }

    #[test]
    fn runtime_values_contain_domain_data_without_transport_identity() {
        let value = serde_json::to_value(ReviewUiReply::AgentResponse {
            message: "Draft updated".to_string(),
        })
        .expect("surface update serializes");
        assert_eq!(
            value,
            json!({
                "type": "agent_response",
                "message": "Draft updated",
            }),
            "a reply that also carried the invalidation would be racing the state that carries it"
        );
        assert!(
            serde_json::from_value::<ReviewUiAction>(json!({
                "type": "agent_message",
                "request_id": "not-the-Provider's",
                "message": "Inspect this",
            }))
            .is_err(),
            "AFUI transport identity must not fit a Review action"
        );
    }
}
