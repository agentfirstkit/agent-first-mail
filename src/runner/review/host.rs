//! The one place the review domain meets AFUI.
//!
//! AFUI owns delivery: the listener, the credential in the URL path, expiry,
//! the security headers, the exact-`Host` and `Origin` checks, the isolated
//! browser window and its disposable profile, and the shutdown that follows.
//! This module owns only the four things afmail can say back — a rendered page,
//! a hash-checked notes save, a hash-checked draft save, and a decision.

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_first_data::CliEmitter;
use agent_first_ui::{
    Outcome, UiCompletion, UiCspNonce, UiExpiry, UiSecurityPolicy, UiSession, UiUrlDelivery,
    UiWindowConfig,
};
use axum::extract::{Form, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::{
    DraftEdit, LAN_REVIEW_EXPIRES_IN_S, NotesEdit, ReviewMode, ReviewPayload, ReviewRenderDocument,
    ReviewRunResult, ReviewSessionState, ReviewSurface, ReviewSurfaceKind, SubmittedFields,
    emit_review_ready, primary_lan_ipv4, render_page, review_envelope, review_input_error,
    review_mode_name, save_draft, save_notes, saved_draft_target, submitted_result,
};
use crate::error::{AppError, Result};
use crate::review_frontend::review_frontend;
use crate::store::Workspace;

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

/// Run one review session to its single outcome.
///
/// The CLI is synchronous and AFUI is async, so the runtime starts and stops
/// here: the whole review is one blocking call from the runner's point of view.
pub(super) fn run_session<W: Write>(
    workspace: &Workspace,
    surface: ReviewSurface,
    model: ReviewRenderDocument,
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
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
        workspace, surface, model, mode, emitter, started,
    ))
}

async fn host_session<W: Write>(
    workspace: &Workspace,
    surface: ReviewSurface,
    model: ReviewRenderDocument,
    mode: ReviewMode,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let nonce = UiCspNonce::generate().map_err(|error| ui_error(mode, &error))?;
    let ui_kind = review_ui_kind(model.surface_kind);
    // Resolved before anything is served so that a frontend afmail cannot load
    // fails the command outright, with the reason on the Agent's stream, rather
    // than opening a window onto an error page nobody asked for.
    let frontend = review_frontend(workspace.root(), ui_kind)?;
    let frontend_id = frontend.map(|location| location.manifest.frontend_id);
    let mut session = UiSession::<Value>::new("afmail", ui_kind)
        .map_err(|error| ui_error(mode, &error))?
        .with_security_policy(review_security_policy(&nonce)?);
    // What this review is about, so a person scanning `afui session list` can
    // tell two open case reviews apart. Identity, not presentation.
    if let Some(subject) = model.subject_refs.first() {
        session = session.with_subject(subject.clone());
    }
    let state = ReviewState {
        inner: Arc::new(ReviewSession {
            workspace: workspace.clone(),
            surface,
            model,
            nonce: nonce.as_str().to_owned(),
            saved: Mutex::new(ReviewSessionState::default()),
            completion: session.completion(),
        }),
    };
    // The state outlives the session so that every outcome — including one the
    // page never got to report — can still name the files this review saved.
    let ended = Arc::clone(&state.inner);
    let router = review_router(state);

    let outcome = match mode {
        ReviewMode::Lan => {
            let lan_ip = primary_lan_ipv4()?;
            let active = session
                .serve(
                    router,
                    UiUrlDelivery::new(UiExpiry::After(Duration::from_secs(
                        LAN_REVIEW_EXPIRES_IN_S,
                    )))
                    .with_bind_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
                    .with_advertised_host(lan_ip.to_string()),
                )
                .await
                .map_err(|error| ui_error(mode, &error))?;
            emit_review_ready(
                emitter,
                &ended.model,
                mode,
                Some(active.access_url()),
                frontend_id.as_deref(),
                started,
            )?;
            active
                .wait()
                .await
                .map_err(|error| ui_error(mode, &error))?
        }
        ReviewMode::Local => {
            let active = session
                .serve(router, UiUrlDelivery::new(UiExpiry::Never))
                .await
                .map_err(|error| ui_error(mode, &error))?;
            emit_review_ready(
                emitter,
                &ended.model,
                mode,
                None,
                frontend_id.as_deref(),
                started,
            )?;
            active
                .open_window(&UiWindowConfig::default())
                .await
                .map_err(|error| ui_error(mode, &error))?
        }
    };

    Ok(ended.with_state(|state| ended_envelope(&ended.model, state, outcome))?)
}

/// Say how a review ended when the page never got to say it.
///
/// `Completed` is the page speaking: it already built the envelope its decision
/// means. The other two are AFUI reporting how the delivery ended, and afmail
/// keeps them apart — someone who closed the window refused this review, while
/// a LAN URL that merely lapsed can be offered again.
pub(super) fn ended_envelope(
    model: &ReviewRenderDocument,
    state: &ReviewSessionState,
    outcome: Outcome<Value>,
) -> Result<Value> {
    match outcome {
        Outcome::Completed(envelope) => Ok(envelope),
        Outcome::Closed => review_envelope(model, "dismissed", state, ReviewPayload::Empty),
        Outcome::Expired => review_envelope(model, "expired", state, ReviewPayload::Empty),
    }
}

/// One review session's shared state, cloned into every handler.
#[derive(Clone)]
struct ReviewState {
    inner: Arc<ReviewSession>,
}

struct ReviewSession {
    workspace: Workspace,
    surface: ReviewSurface,
    model: ReviewRenderDocument,
    nonce: String,
    saved: Mutex<ReviewSessionState>,
    completion: UiCompletion<Value>,
}

impl ReviewSession {
    /// Read the saved-file list back, whoever is asking and whenever.
    ///
    /// A review reports every file it wrote even when it ends without a
    /// decision, so this has to work after the session is over.
    fn with_state<T>(&self, read: impl FnOnce(&mut ReviewSessionState) -> T) -> T {
        let mut state = self
            .saved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&mut state)
    }

    /// The envelope for a decision the page makes without a typed payload.
    fn envelope(&self, outcome: &str) -> Result<Value> {
        self.with_state(|state| review_envelope(&self.model, outcome, state, ReviewPayload::Empty))
    }
}

/// One route per thing the review page can say.
///
/// Each route names its own payload, so a save cannot arrive shaped like a
/// decision and a decision cannot arrive shaped like a save. Axum's default
/// body limit bounds every one of them.
fn review_router(state: ReviewState) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/notes", post(save_notes_route))
        .route("/draft", post(save_draft_route))
        .route("/submit", post(submit_route))
        .route("/cancel", post(cancel_route))
        .with_state(state)
}

/// The page's own script is the only script it may run, and it may talk to
/// nothing but this session.
fn review_security_policy(nonce: &UiCspNonce) -> Result<UiSecurityPolicy> {
    let policy = format!(
        "default-src 'none'; script-src 'nonce-{}'; style-src 'unsafe-inline'; img-src 'self'; \
         connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        nonce.as_str()
    );
    let policy = HeaderValue::from_str(&policy).map_err(|error| {
        AppError::new(
            "review_host_failed",
            format!("could not build the review content security policy: {error}"),
        )
    })?;
    Ok(UiSecurityPolicy::isolated().with_content_security_policy(policy))
}

async fn page(State(state): State<ReviewState>) -> Response {
    match render_page(
        &state.inner.workspace,
        &state.inner.model,
        &state.inner.nonce,
    ) {
        Ok(html) => Html(html).into_response(),
        // The page is rendered per request because a workspace template can
        // change under a live session; a broken one is the person's to fix, and
        // the code names which template rule it broke.
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.error_code).into_response(),
    }
}

async fn save_notes_route(
    State(state): State<ReviewState>,
    Form(edit): Form<NotesEdit>,
) -> Response {
    let session = &state.inner;
    match save_notes(&session.surface, &edit) {
        Ok(Ok(file)) => saved_response(session, file),
        Ok(Err(base_hash)) => stale_response(base_hash),
        Err(error) => save_error(StatusCode::BAD_REQUEST, error.error_code),
    }
}

async fn save_draft_route(
    State(state): State<ReviewState>,
    Form(edit): Form<DraftEdit>,
) -> Response {
    let session = &state.inner;
    let target = saved_draft_target(&session.surface, &session.model, &edit.draft_name);
    let (case_ref, draft_name) = match target {
        Ok(target) => target,
        Err(error) => return save_error(StatusCode::BAD_REQUEST, error.error_code),
    };
    match save_draft(&case_ref, &draft_name, &edit) {
        Ok(Ok(file)) => saved_response(session, file),
        Ok(Err(base_hash)) => stale_response(base_hash),
        Err(error) if review_input_error(&error) => {
            save_error(StatusCode::BAD_REQUEST, error.error_code)
        }
        Err(error) => save_error(StatusCode::INTERNAL_SERVER_ERROR, error.error_code),
    }
}

async fn submit_route(
    State(state): State<ReviewState>,
    Form(fields): Form<SubmittedFields>,
) -> Response {
    let session = &state.inner;
    let result = session
        .with_state(|saved| submitted_result(&session.surface, &session.model, saved, &fields));
    match result {
        Ok(envelope) => {
            session.completion.complete(envelope).await;
            (StatusCode::OK, Json(json!({"submitted": true}))).into_response()
        }
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"submitted": false, "code": error.error_code})),
        )
            .into_response(),
    }
}

/// Cancelling is a decision, not a disappearance: the person said no.
async fn cancel_route(State(state): State<ReviewState>) -> Response {
    let session = &state.inner;
    match session.envelope("cancelled") {
        Ok(envelope) => {
            session.completion.complete(envelope).await;
            (StatusCode::OK, Json(json!({"cancelled": true}))).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"cancelled": false, "code": error.error_code})),
        )
            .into_response(),
    }
}

fn saved_response(session: &ReviewSession, file: super::SavedFile) -> Response {
    let base_hash = file.base_hash.clone();
    session.with_state(|state| state.saved_files.push(file));
    (
        StatusCode::OK,
        Json(json!({"saved": true, "base_hash": base_hash})),
    )
        .into_response()
}

fn stale_response(base_hash: String) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({"saved": false, "code": "review_stale", "base_hash": base_hash})),
    )
        .into_response()
}

fn save_error(status: StatusCode, code: &'static str) -> Response {
    (status, Json(json!({"saved": false, "code": code}))).into_response()
}

/// Translate an AFUI delivery failure into an afmail error an Agent can act on.
fn ui_error(mode: ReviewMode, error: &agent_first_ui::Error) -> AppError {
    use agent_first_ui::Error;
    match error {
        Error::WindowBinaryNotFound => AppError::new(
            "review_browser_not_found",
            format!("{error}; Chrome, Chromium, or Edge is required for local review"),
        ),
        Error::WindowLaunch { .. } | Error::WindowWait(_) => {
            AppError::new("review_browser_failed", error.to_string())
        }
        Error::Io(_) => AppError::retryable(
            "review_bind_failed",
            format!(
                "could not host the {} review session: {error}",
                review_mode_name(mode)
            ),
        )
        .with_hint(
            "Allow afmail to bind a local TCP listener and retry; in a sandbox, grant local listen/network permission or run this review command outside the sandbox.",
        )
        .with_details(json!({"review_mode": review_mode_name(mode)})),
        _ => AppError::new(
            "review_host_failed",
            format!(
                "could not start the {} review session: {error}",
                review_mode_name(mode)
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_browser_and_a_refused_listener_are_different_problems() {
        let missing = ui_error(
            ReviewMode::Local,
            &agent_first_ui::Error::WindowBinaryNotFound,
        );
        assert_eq!(missing.error_code, "review_browser_not_found");
        assert!(!missing.retryable);

        let refused = ui_error(
            ReviewMode::Local,
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
            Some("local")
        );
    }

    #[test]
    fn the_page_policy_authorizes_only_this_session_nonce() {
        let nonce = UiCspNonce::generate().expect("nonce generates");
        let policy = review_security_policy(&nonce).expect("policy builds");
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
}
