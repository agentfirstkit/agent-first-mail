use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use ring::hmac;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::model::{
    CaseActionRequest, CaseActionResult, CaseCreateRequest, CaseCreateResult, CaseListQuery,
    CaseListResult, CaseMessageAddRequest, CaseMessageAddResult, CaseShowResult,
    CaseSuggestionClearResult, CaseSuggestionSetRequest, CaseSuggestionSetResult,
    DraftCreateRequest, DraftCreateResult, DraftDeleteQuery, DraftDeleteResult, DraftListResult,
    DraftPatchRequest, DraftPatchResult, DraftQueueRequest, DraftQueueResult, DraftShowResult,
    DraftValidateResult, MessageDispositionRequest, MessageDispositionResult, MessageListQuery,
    MessageListResult, MessageShowResult, MessageSuggestionClearResult,
    MessageSuggestionSetRequest, MessageSuggestionSetResult, NotificationActionRequest,
    NotificationActionResult, NotificationCreateRequest, NotificationCreateResult,
    NotificationListResult, NotificationMessageAddRequest, NotificationMessageAddResult,
    NotificationShowResult, NotificationSuggestionClearResult, NotificationSuggestionSetRequest,
    NotificationSuggestionSetResult, PullRequest, PullResult, PushConfirmRequest,
    PushConfirmResult, PushPlanResult, StatusResult, SuggestionClearQuery, TriageListQuery,
    TriageListResult, case_suggestion_clear_command, message_suggestion_clear_command,
    notification_suggestion_clear_command,
};
use super::schema::{openapi_document, schema_index, standalone_schemas};
use crate::cli::{CaseCommand, CaseDraftAction, Command, NotificationCommand, PushAction};
use crate::error::AppError;
use crate::runner::dispatch::execute_command_at;
use crate::types::CommandResult;

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct ApiState {
    workspace_root: Arc<PathBuf>,
    access_token_secret: Arc<Vec<u8>>,
}

impl ApiState {
    pub(crate) fn new(workspace_root: PathBuf, access_token_secret: String) -> Self {
        Self {
            workspace_root: Arc::new(workspace_root),
            access_token_secret: Arc::new(access_token_secret.into_bytes()),
        }
    }
}

struct ApiPath<T>(T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(path_rejection)
    }
}

pub(crate) fn router(state: ApiState) -> Router {
    let public = Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .route("/schemas/index.json", get(schemas_index))
        .route("/schemas/{schema_file}", get(schema))
        .method_not_allowed_fallback(method_not_allowed);
    let protected = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/pull", post(pull))
        .route("/v1/triage", get(triage_list))
        .route("/v1/messages", get(message_list))
        .route("/v1/messages/{message_id}", get(message_show))
        .route(
            "/v1/messages/{message_id}/disposition",
            post(message_disposition),
        )
        .route(
            "/v1/messages/{message_id}/agent-suggestion",
            put(message_suggestion_set).delete(message_suggestion_clear),
        )
        .route("/v1/cases", get(case_list).post(case_create))
        .route("/v1/cases/{case_ref}", get(case_show))
        .route("/v1/cases/{case_ref}/messages", post(case_message_add))
        .route("/v1/cases/{case_ref}/actions", post(case_action))
        .route(
            "/v1/cases/{case_ref}/agent-suggestion",
            put(case_suggestion_set).delete(case_suggestion_clear),
        )
        .route(
            "/v1/notifications",
            get(notification_list).post(notification_create),
        )
        .route(
            "/v1/notifications/{notification_ref}",
            get(notification_show),
        )
        .route(
            "/v1/notifications/{notification_ref}/messages",
            post(notification_message_add),
        )
        .route(
            "/v1/notifications/{notification_ref}/actions",
            post(notification_action),
        )
        .route(
            "/v1/notifications/{notification_ref}/agent-suggestion",
            put(notification_suggestion_set).delete(notification_suggestion_clear),
        )
        .route(
            "/v1/cases/{case_ref}/drafts",
            get(draft_list).post(draft_create),
        )
        .route(
            "/v1/cases/{case_ref}/drafts/{draft_name}",
            get(draft_show).patch(draft_patch).delete(draft_delete),
        )
        .route(
            "/v1/cases/{case_ref}/drafts/{draft_name}/validate",
            post(draft_validate),
        )
        .route(
            "/v1/cases/{case_ref}/drafts/{draft_name}/queue",
            post(draft_queue),
        )
        .route("/v1/push/plan", get(push_plan))
        .route("/v1/push/confirm", post(push_confirm))
        .method_not_allowed_fallback(method_not_allowed)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));

    public
        .merge(protected)
        .fallback(not_found)
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(security_headers))
}

async fn health() -> Response {
    result_response(
        StatusCode::OK,
        json!({
            "service": "afmail",
            "version": env!("CARGO_PKG_VERSION"),
            "status": "ready",
        }),
        0,
    )
}

async fn openapi() -> Response {
    raw_json_response(
        StatusCode::OK,
        openapi_document(),
        "application/vnd.oai.openapi+json;version=3.2",
    )
}

async fn schemas_index() -> Response {
    raw_json_response(StatusCode::OK, schema_index(), "application/json")
}

async fn schema(ApiPath(schema_file): ApiPath<String>) -> Response {
    match standalone_schemas().remove(&schema_file) {
        Some(schema) => raw_json_response(StatusCode::OK, schema, "application/schema+json"),
        None => error_response(
            AppError::new("schema_not_found", "JSON Schema not found"),
            0,
        ),
    }
}

async fn not_found() -> Response {
    error_response(
        AppError::new("api_route_not_found", "API route not found"),
        0,
    )
}

async fn method_not_allowed() -> Response {
    error_response(
        AppError::new(
            "api_method_not_allowed",
            "HTTP method is not allowed for this route",
        ),
        0,
    )
}

async fn status(State(state): State<ApiState>) -> Response {
    dispatch::<StatusResult>(state, Command::Status, None).await
}

async fn pull(
    State(state): State<ApiState>,
    body: Result<Json<PullRequest>, JsonRejection>,
) -> Response {
    let request = match json_body(body) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let command = match request.into_command() {
        Ok(command) => command,
        Err(error) => return error_response(error, 0),
    };
    dispatch::<PullResult>(state, command, None).await
}

async fn triage_list(
    State(state): State<ApiState>,
    query: Result<Query<TriageListQuery>, QueryRejection>,
) -> Response {
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<TriageListResult>(state, query.into_command(), None).await
}

async fn message_list(
    State(state): State<ApiState>,
    query: Result<Query<MessageListQuery>, QueryRejection>,
) -> Response {
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<MessageListResult>(state, query.into_command(), None).await
}

async fn message_show(
    State(state): State<ApiState>,
    ApiPath(message_id): ApiPath<String>,
) -> Response {
    dispatch::<MessageShowResult>(
        state,
        Command::Message {
            action: crate::cli::MessageAction::Show { message_id },
        },
        None,
    )
    .await
}

async fn message_disposition(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(message_id): ApiPath<String>,
    body: Result<Json<MessageDispositionRequest>, JsonRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    dispatch::<MessageDispositionResult>(state, body.into_command(message_id), Some(request_id))
        .await
}

async fn message_suggestion_set(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(message_id): ApiPath<String>,
    body: Result<Json<MessageSuggestionSetRequest>, JsonRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    dispatch::<MessageSuggestionSetResult>(state, body.into_command(message_id), Some(request_id))
        .await
}

async fn message_suggestion_clear(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(message_id): ApiPath<String>,
    query: Result<Query<SuggestionClearQuery>, QueryRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<MessageSuggestionClearResult>(
        state,
        message_suggestion_clear_command(message_id, query),
        Some(request_id),
    )
    .await
}

async fn case_list(
    State(state): State<ApiState>,
    query: Result<Query<CaseListQuery>, QueryRejection>,
) -> Response {
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<CaseListResult>(state, query.into_command(), None).await
}

async fn case_create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Json<CaseCreateRequest>, JsonRejection>,
) -> Response {
    mutation_body::<CaseCreateResult, _, _>(state, headers, body, |body| body.into_command()).await
}

async fn case_show(State(state): State<ApiState>, ApiPath(case_ref): ApiPath<String>) -> Response {
    dispatch::<CaseShowResult>(
        state,
        Command::Case {
            action: CaseCommand::Show { case_ref },
        },
        None,
    )
    .await
}

async fn case_message_add(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(case_ref): ApiPath<String>,
    body: Result<Json<CaseMessageAddRequest>, JsonRejection>,
) -> Response {
    mutation_body::<CaseMessageAddResult, _, _>(state, headers, body, |body| {
        body.into_command(case_ref)
    })
    .await
}

async fn case_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(case_ref): ApiPath<String>,
    body: Result<Json<CaseActionRequest>, JsonRejection>,
) -> Response {
    mutation_body::<CaseActionResult, _, _>(state, headers, body, |body| {
        body.into_command(case_ref)
    })
    .await
}

async fn case_suggestion_set(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(case_ref): ApiPath<String>,
    body: Result<Json<CaseSuggestionSetRequest>, JsonRejection>,
) -> Response {
    mutation_body::<CaseSuggestionSetResult, _, _>(state, headers, body, |body| {
        body.into_command(case_ref)
    })
    .await
}

async fn case_suggestion_clear(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(case_ref): ApiPath<String>,
    query: Result<Query<SuggestionClearQuery>, QueryRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<CaseSuggestionClearResult>(
        state,
        case_suggestion_clear_command(case_ref, query),
        Some(request_id),
    )
    .await
}

async fn notification_list(State(state): State<ApiState>) -> Response {
    dispatch::<NotificationListResult>(
        state,
        Command::Notification {
            action: NotificationCommand::List,
        },
        None,
    )
    .await
}

async fn notification_create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Json<NotificationCreateRequest>, JsonRejection>,
) -> Response {
    mutation_body::<NotificationCreateResult, _, _>(state, headers, body, |body| {
        body.into_command()
    })
    .await
}

async fn notification_show(
    State(state): State<ApiState>,
    ApiPath(notification_ref): ApiPath<String>,
) -> Response {
    dispatch::<NotificationShowResult>(
        state,
        Command::Notification {
            action: NotificationCommand::Show { notification_ref },
        },
        None,
    )
    .await
}

async fn notification_message_add(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(notification_ref): ApiPath<String>,
    body: Result<Json<NotificationMessageAddRequest>, JsonRejection>,
) -> Response {
    mutation_body::<NotificationMessageAddResult, _, _>(state, headers, body, |body| {
        body.into_command(notification_ref)
    })
    .await
}

async fn notification_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(notification_ref): ApiPath<String>,
    body: Result<Json<NotificationActionRequest>, JsonRejection>,
) -> Response {
    mutation_body::<NotificationActionResult, _, _>(state, headers, body, |body| {
        body.into_command(notification_ref)
    })
    .await
}

async fn notification_suggestion_set(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(notification_ref): ApiPath<String>,
    body: Result<Json<NotificationSuggestionSetRequest>, JsonRejection>,
) -> Response {
    mutation_body::<NotificationSuggestionSetResult, _, _>(state, headers, body, |body| {
        body.into_command(notification_ref)
    })
    .await
}

async fn notification_suggestion_clear(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(notification_ref): ApiPath<String>,
    query: Result<Query<SuggestionClearQuery>, QueryRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<NotificationSuggestionClearResult>(
        state,
        notification_suggestion_clear_command(notification_ref, query),
        Some(request_id),
    )
    .await
}

async fn draft_list(State(state): State<ApiState>, ApiPath(case_ref): ApiPath<String>) -> Response {
    dispatch::<DraftListResult>(
        state,
        Command::Case {
            action: CaseCommand::Draft {
                action: CaseDraftAction::List { case_ref },
            },
        },
        None,
    )
    .await
}

async fn draft_create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath(case_ref): ApiPath<String>,
    body: Result<Json<DraftCreateRequest>, JsonRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let command = match body.into_command(case_ref) {
        Ok(command) => command,
        Err(error) => return error_response(error, 0),
    };
    dispatch::<DraftCreateResult>(state, command, Some(request_id)).await
}

async fn draft_show(
    State(state): State<ApiState>,
    ApiPath((case_ref, draft_name)): ApiPath<(String, String)>,
) -> Response {
    dispatch::<DraftShowResult>(
        state,
        Command::Case {
            action: CaseCommand::Draft {
                action: CaseDraftAction::Show {
                    case_ref,
                    draft_name,
                },
            },
        },
        None,
    )
    .await
}

async fn draft_patch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath((case_ref, draft_name)): ApiPath<(String, String)>,
    body: Result<Json<DraftPatchRequest>, JsonRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let command = match body.into_command(case_ref, draft_name) {
        Ok(command) => command,
        Err(error) => return error_response(error, 0),
    };
    dispatch::<DraftPatchResult>(state, command, Some(request_id)).await
}

async fn draft_delete(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath((case_ref, draft_name)): ApiPath<(String, String)>,
    query: Result<Query<DraftDeleteQuery>, QueryRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let query = match query_value(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    dispatch::<DraftDeleteResult>(
        state,
        Command::Case {
            action: CaseCommand::Draft {
                action: CaseDraftAction::Remove {
                    case_ref,
                    draft_name,
                    reason: query.reason,
                },
            },
        },
        Some(request_id),
    )
    .await
}

async fn draft_validate(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath((case_ref, draft_name)): ApiPath<(String, String)>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    dispatch::<DraftValidateResult>(
        state,
        Command::Case {
            action: CaseCommand::Draft {
                action: CaseDraftAction::Validate {
                    case_ref,
                    draft_name,
                },
            },
        },
        Some(request_id),
    )
    .await
}

async fn draft_queue(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiPath((case_ref, draft_name)): ApiPath<(String, String)>,
    body: Result<Json<DraftQueueRequest>, JsonRejection>,
) -> Response {
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    dispatch::<DraftQueueResult>(
        state,
        body.into_command(case_ref, draft_name),
        Some(request_id),
    )
    .await
}

async fn push_plan(State(state): State<ApiState>) -> Response {
    dispatch::<PushPlanResult>(
        state,
        Command::Push {
            dry_run: true,
            confirm: None,
            action: Some(PushAction::List),
        },
        None,
    )
    .await
}

async fn push_confirm(
    State(state): State<ApiState>,
    body: Result<Json<PushConfirmRequest>, JsonRejection>,
) -> Response {
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    dispatch::<PushConfirmResult>(
        state,
        Command::Push {
            dry_run: false,
            confirm: Some(body.plan_id),
            action: None,
        },
        None,
    )
    .await
}

async fn mutation_body<R, T, F>(
    state: ApiState,
    headers: HeaderMap,
    body: Result<Json<T>, JsonRejection>,
    command: F,
) -> Response
where
    R: DeserializeOwned + Send + 'static,
    T: Send + 'static,
    F: FnOnce(T) -> Command + Send,
{
    let request_id = match idempotency_key(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let body = match json_body(body) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    dispatch::<R>(state, command(body), Some(request_id)).await
}

async fn dispatch<T>(state: ApiState, command: Command, request_id: Option<String>) -> Response
where
    T: DeserializeOwned + Send + 'static,
{
    let started = Instant::now();
    let root = state.workspace_root.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        execute_command_at(command, request_id.as_deref(), &root, None)
    })
    .await;
    let duration_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(Ok(result)) if result.is_success() => {
            let payload = normalize_api_payload(result.into_value());
            if let Err(error) = serde_json::from_value::<T>(payload.clone()) {
                return error_response(
                    AppError::new(
                        "api_contract_violation",
                        format!("afmail returned a result outside the documented schema: {error}"),
                    ),
                    duration_ms,
                );
            }
            result_response(StatusCode::OK, payload, duration_ms)
        }
        Ok(Ok(result)) => error_response(command_result_error(result), duration_ms),
        Ok(Err(error)) => error_response(error, duration_ms),
        Err(_) => error_response(
            AppError::new("store_error", "API command task did not complete"),
            duration_ms,
        ),
    }
}

fn command_result_error(result: CommandResult) -> AppError {
    AppError::new("command_failed", "one or more command items failed").with_details(json!({
        "succeeded_count": result.succeeded_count(),
        "failed_count": result.failed_count(),
        "result": normalize_api_payload(result.into_value()),
    }))
}

fn normalize_api_payload(mut value: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.remove("code");
        fields.remove("ok");
        fields.remove("outcome");
    }
    value
}

fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, Box<Response>> {
    body.map(|Json(body)| body).map_err(|rejection| {
        let (code, status) = match rejection.status() {
            StatusCode::PAYLOAD_TOO_LARGE => ("payload_too_large", StatusCode::PAYLOAD_TOO_LARGE),
            StatusCode::UNSUPPORTED_MEDIA_TYPE => {
                ("unsupported_media_type", StatusCode::UNSUPPORTED_MEDIA_TYPE)
            }
            _ => ("invalid_request", StatusCode::BAD_REQUEST),
        };
        Box::new(error_response_with_status(
            AppError::new(code, "invalid JSON request body")
                .with_details(json!({"details": {"kind": "json_rejection"}})),
            status,
            0,
        ))
    })
}

fn query_value<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, Box<Response>> {
    query.map(|Query(query)| query).map_err(|_| {
        Box::new(error_response(
            AppError::new("invalid_request", "invalid query parameters")
                .with_details(json!({"details": {"kind": "query_rejection"}})),
            0,
        ))
    })
}

fn path_rejection(_rejection: PathRejection) -> Response {
    error_response(
        AppError::new("invalid_request", "invalid path parameters")
            .with_details(json!({"details": {"kind": "path_rejection"}})),
        0,
    )
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, Box<Response>> {
    let value = headers
        .get("idempotency-key")
        .ok_or_else(|| {
            Box::new(error_response(
                AppError::new(
                    "idempotency_key_required",
                    "Idempotency-Key header is required for this local mutation",
                ),
                0,
            ))
        })?
        .to_str()
        .map_err(|_| {
            Box::new(error_response(
                AppError::new("invalid_request", "Idempotency-Key must be valid ASCII"),
                0,
            ))
        })?;
    let mut characters = value.chars();
    let valid_first = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let valid_rest = characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'));
    if !(8..=200).contains(&value.len()) || !valid_first || !valid_rest {
        return Err(Box::new(error_response(
            AppError::new(
                "invalid_request",
                "Idempotency-Key must contain 8 through 200 ASCII letters, digits, dots, underscores, or hyphens and start with a letter or digit",
            ),
            0,
        )));
    }
    Ok(value.to_string())
}

async fn require_bearer(
    State(state): State<ApiState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if query_may_contain_credential(request.uri().query()) {
        return error_response(
            AppError::new(
                "credential_location_invalid",
                "credentials are accepted only in the Authorization header",
            ),
            0,
        );
    }
    let authorized = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token)
        .is_some_and(|token| bearer_matches(token, state.access_token_secret.as_slice()));
    if !authorized {
        return error_response(
            AppError::new(
                "authentication_required",
                "missing or invalid bearer credential",
            ),
            0,
        );
    }
    next.run(request).await
}

fn bearer_token(value: &str) -> Option<&str> {
    let mut parts = value.split_ascii_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(token)
}

fn bearer_matches(candidate: &str, expected: &[u8]) -> bool {
    const CONTEXT: &[u8] = b"afmail-api-access-token-v1";
    let candidate_key = hmac::Key::new(hmac::HMAC_SHA256, candidate.as_bytes());
    let candidate_tag = hmac::sign(&candidate_key, CONTEXT);
    let expected_key = hmac::Key::new(hmac::HMAC_SHA256, expected);
    hmac::verify(&expected_key, CONTEXT, candidate_tag.as_ref()).is_ok()
}

fn query_may_contain_credential(query: Option<&str>) -> bool {
    query.unwrap_or_default().split('&').any(|pair| {
        let key = pair
            .split_once('=')
            .map_or(pair, |(key, _)| key)
            .to_ascii_lowercase();
        matches!(
            key.as_str(),
            "token" | "authorization" | "access_token" | "access_token_secret"
        ) || key.ends_with("_token")
            || key.ends_with("_secret")
    })
}

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

fn result_response(status: StatusCode, payload: Value, duration_ms: u64) -> Response {
    let event: Value = agent_first_data::json_result(payload)
        .trace(json!({"duration_ms": duration_ms}))
        .build()
        .into();
    json_response(status, agent_first_data::redacted_value(&event))
}

fn error_response(error: AppError, duration_ms: u64) -> Response {
    let status = error_status(&error);
    error_response_with_status(error, status, duration_ms)
}

fn error_response_with_status(error: AppError, status: StatusCode, duration_ms: u64) -> Response {
    let mut response = json_response(
        status,
        agent_first_data::redacted_value(&error.to_protocol_value(duration_ms)),
    );
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"afmail\""),
        );
    }
    response
}

fn raw_json_response(status: StatusCode, value: Value, media_type: &'static str) -> Response {
    let mut response = (status, Json(agent_first_data::redacted_value(&value))).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(media_type));
    response
}

fn json_response(status: StatusCode, value: Value) -> Response {
    (status, Json(value)).into_response()
}

fn error_status(error: &AppError) -> StatusCode {
    match error.error_code {
        "authentication_required" => StatusCode::UNAUTHORIZED,
        "api_route_not_found"
        | "schema_not_found"
        | "message_not_found"
        | "case_not_found"
        | "notification_not_found"
        | "notification_entry_not_found"
        | "draft_not_found" => StatusCode::NOT_FOUND,
        "workspace_locked"
        | "transaction_incomplete"
        | "idempotency_conflict"
        | "idempotency_incomplete"
        | "review_conflict"
        | "push_plan_changed" => StatusCode::CONFLICT,
        "api_method_not_allowed" => StatusCode::METHOD_NOT_ALLOWED,
        "payload_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "unsupported_media_type" => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "store_error" | "api_contract_violation" => StatusCode::INTERNAL_SERVER_ERROR,
        "invalid_request"
        | "idempotency_key_required"
        | "credential_location_invalid"
        | "request_id_not_supported" => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::{ApiState, MAX_BODY_BYTES, router};
    use crate::store::Workspace;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn discovery_is_public_but_workspace_routes_require_bearer() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));

        let openapi = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(openapi.status(), StatusCode::OK);

        let status = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(status.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer realm=\"afmail\"")
        );
        let body = to_bytes(status.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let body = String::from_utf8_lossy(&body);
        assert!(!body.contains(TOKEN));

        let wrong_method = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/status")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_method.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_status_uses_afdata_envelope() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let response = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()))
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .header("authorization", format!("bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["kind"], "result");
        assert!(value["result"]["workspace_uid"].is_string());
        assert!(value["trace"]["duration_ms"].is_u64());
    }

    #[tokio::test]
    async fn malformed_json_is_structured_and_mutations_require_idempotency() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/cases")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("idempotency-key", "case-create-json-001")
                    .header("content-type", "application/json")
                    .body(Body::from("{"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["kind"], "error");
        assert_eq!(value["error"]["code"], "invalid_request");
        assert_eq!(value["error"]["details"]["kind"], "json_rejection");

        let unsupported = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/cases")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("idempotency-key", "case-create-media-001")
                    .body(Body::from(r#"{"case_name":"media","group":"support"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(unsupported.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let oversized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/cases")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("idempotency-key", "case-create-large-001")
                    .header("content-type", "application/json")
                    .body(Body::from(vec![b'a'; MAX_BODY_BYTES + 1]))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/cases")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"case_name":"support","group":"support","message_id":null,"summary":null,"reason":"test"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let value = response_json(response).await;
        assert_eq!(value["error"]["code"], "idempotency_key_required");
    }

    #[tokio::test]
    async fn extractor_rejections_keep_status_and_do_not_echo_request_canaries() {
        const CANARY: &str = "AFMAILExtractorPrivateCanary";
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));
        let mut oversized = vec![b'a'; MAX_BODY_BYTES + 1];
        oversized[..CANARY.len()].copy_from_slice(CANARY.as_bytes());
        let cases = [
            (
                "POST",
                "/v1/cases".to_string(),
                Some("application/json"),
                Body::from(format!(
                    r#"{{"case_name":"test","group":"support","{CANARY}":"private"}}"#
                )),
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "invalid JSON request body",
                "json_rejection",
            ),
            (
                "POST",
                "/v1/cases".to_string(),
                Some("application/json"),
                Body::from(oversized),
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "invalid JSON request body",
                "json_rejection",
            ),
            (
                "POST",
                "/v1/cases".to_string(),
                Some("text/plain"),
                Body::from(CANARY),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "invalid JSON request body",
                "json_rejection",
            ),
            (
                "GET",
                format!("/v1/cases?scope={CANARY}"),
                None,
                Body::empty(),
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "invalid query parameters",
                "query_rejection",
            ),
            (
                "GET",
                format!("/v1/cases/{CANARY}%FF"),
                None,
                Body::empty(),
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "invalid path parameters",
                "path_rejection",
            ),
        ];

        for (method, uri, content_type, body, status, code, message, kind) in cases {
            let mut request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("idempotency-key", "extractor-canary-001");
            if let Some(content_type) = content_type {
                request = request.header("content-type", content_type);
            }
            let response = app
                .clone()
                .oneshot(request.body(body).expect("request"))
                .await
                .expect("response");
            assert_eq!(response.status(), status);
            let body = to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("response body");
            let text = std::str::from_utf8(&body).expect("UTF-8 response");
            assert!(!text.contains(CANARY));
            assert!(!text.contains(TOKEN));
            let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
            assert_eq!(value["kind"], "error");
            assert_eq!(value["error"]["code"], code);
            assert_eq!(value["error"]["message"], message);
            assert_eq!(value["error"]["retryable"], false);
            assert_eq!(value["error"]["details"], serde_json::json!({"kind": kind}));
            assert_eq!(value["trace"]["duration_ms"], 0);
        }
    }

    #[tokio::test]
    async fn invalid_case_create_does_not_claim_key_and_corrected_request_succeeds() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let workspace = Workspace::at(temp.path());
        workspace.init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));
        let request_id = "case-create-correctable-001";
        let record_path = temp
            .path()
            .join(".afmail/requests")
            .join(format!("{request_id}.json"));

        let invalid = app
            .clone()
            .oneshot(case_create_request(request_id, "   "))
            .await
            .expect("invalid response");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(invalid).await["error"]["code"],
            "invalid_request"
        );
        assert!(!record_path.exists());
        assert_eq!(
            workspace.case_list(false, false).expect("list cases")["count"],
            0
        );

        let corrected = app
            .oneshot(case_create_request(request_id, "corrected"))
            .await
            .expect("corrected response");
        assert_eq!(corrected.status(), StatusCode::OK);
        let corrected = response_json(corrected).await;
        assert_eq!(corrected["result"]["case_name"], "corrected");
        assert_eq!(
            workspace.case_list(false, false).expect("list cases")["count"],
            1
        );
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(record_path).expect("read completed record"))
                .expect("record JSON");
        assert_eq!(record["status"], "completed");
    }

    #[tokio::test]
    async fn completed_case_create_replays_before_current_config_precheck() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));
        let request_id = "case-create-cached-001";
        let record_path = temp
            .path()
            .join(".afmail/requests")
            .join(format!("{request_id}.json"));
        let first = app
            .clone()
            .oneshot(case_create_request(request_id, "cached"))
            .await
            .expect("first response");
        assert_eq!(first.status(), StatusCode::OK);
        let first = response_json(first).await;
        let record_before = std::fs::read(&record_path).expect("read completed record");
        std::fs::write(temp.path().join(".afmail/config.json"), "{")
            .expect("change current config");

        let fresh = app
            .clone()
            .oneshot(case_create_request("case-create-fresh-001", "fresh"))
            .await
            .expect("fresh response");
        assert_eq!(fresh.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response_json(fresh).await["error"]["code"], "store_error");
        assert!(
            !temp
                .path()
                .join(".afmail/requests/case-create-fresh-001.json")
                .exists()
        );

        let retry = app
            .oneshot(case_create_request(request_id, "cached"))
            .await
            .expect("cached retry response");
        assert_eq!(retry.status(), StatusCode::OK);
        assert_eq!(response_json(retry).await["result"], first["result"]);
        assert_eq!(
            std::fs::read(record_path).expect("read retained record"),
            record_before
        );
    }

    #[tokio::test]
    async fn case_create_after_effect_failure_keeps_pending_and_api_retry_is_incomplete() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let workspace = Workspace::at(temp.path());
        workspace.init().expect("init workspace");
        let request_id = "case-create-pending-001";
        let failure = workspace
            .run_idempotent_request(
                request_id,
                "case_create",
                serde_json::json!({
                    "name": "pending",
                    "group": "support",
                    "message": null,
                    "summary": null,
                    "reason": "test",
                }),
                || {
                    workspace.create_case("pending", Some("support"), None, None, Some("test"))?;
                    Err(crate::error::AppError::new(
                        "after_effect_test_failure",
                        "forced failure after creating the case",
                    ))
                },
            )
            .expect_err("forced failure");
        assert_eq!(failure.error_code, "after_effect_test_failure");
        let record_path = temp
            .path()
            .join(".afmail/requests")
            .join(format!("{request_id}.json"));
        let record_before = std::fs::read(&record_path).expect("read pending record");
        let record: serde_json::Value =
            serde_json::from_slice(&record_before).expect("record JSON");
        assert_eq!(record["status"], "pending");
        assert_eq!(record["result"], serde_json::Value::Null);
        let cases_before = workspace
            .case_list(false, false)
            .expect("list created case");
        assert_eq!(cases_before["count"], 1);

        let response = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()))
            .oneshot(case_create_request(request_id, "pending"))
            .await
            .expect("retry response");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "idempotency_incomplete"
        );
        assert_eq!(
            std::fs::read(record_path).expect("read retained pending record"),
            record_before
        );
        assert_eq!(
            workspace
                .case_list(false, false)
                .expect("list retained case"),
            cases_before
        );
    }

    #[tokio::test]
    async fn local_mutation_reuses_persistent_idempotency_and_rejects_changed_input() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));
        let request = |case_name: &str| {
            Request::builder()
                .method("POST")
                .uri("/v1/cases")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("idempotency-key", "case-create-001")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"case_name":"{case_name}","group":"support","message_id":null,"summary":null,"reason":"test"}}"#
                )))
                .expect("request")
        };

        let first = app.clone().oneshot(request("first")).await.expect("first");
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value = serde_json::from_slice(
            &to_bytes(first.into_body(), 1024 * 1024)
                .await
                .expect("first body"),
        )
        .expect("first JSON");

        let retry = app.clone().oneshot(request("first")).await.expect("retry");
        assert_eq!(retry.status(), StatusCode::OK);
        let retry: serde_json::Value = serde_json::from_slice(
            &to_bytes(retry.into_body(), 1024 * 1024)
                .await
                .expect("retry body"),
        )
        .expect("retry JSON");
        assert_eq!(retry["result"], first["result"]);

        let changed = app.oneshot(request("changed")).await.expect("changed");
        assert_eq!(changed.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn typed_collection_draft_and_push_results_match_runtime_output() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));

        let case = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/cases")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("idempotency-key", "typed-case-create-001")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"case_name":"typed","group":"support","reason":"test"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("case response");
        assert_eq!(case.status(), StatusCode::OK);
        let case = response_json(case).await;
        let case_uid = case["result"]["case_uid"]
            .as_str()
            .expect("case uid")
            .to_string();

        for uri in [
            "/v1/triage".to_string(),
            "/v1/messages".to_string(),
            "/v1/cases".to_string(),
            format!("/v1/cases/{case_uid}"),
            format!("/v1/cases/{case_uid}/drafts"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("authorization", format!("Bearer {TOKEN}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("read response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let notification = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/notifications")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("idempotency-key", "typed-notification-create-001")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"notification_name":"typed","reason":"test"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("notification response");
        assert_eq!(notification.status(), StatusCode::OK);
        let notification = response_json(notification).await;
        let notification_uid = notification["result"]["notification_uid"]
            .as_str()
            .expect("notification uid");
        for uri in [
            "/v1/notifications".to_string(),
            format!("/v1/notifications/{notification_uid}"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("authorization", format!("Bearer {TOKEN}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("notification read response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let plan = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/push/plan")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("plan response");
        assert_eq!(plan.status(), StatusCode::OK);
        let plan = response_json(plan).await;
        let plan_id = plan["result"]["plan_id"].as_str().expect("plan id");
        let confirm = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/push/confirm")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"plan_id":"{plan_id}"}}"#)))
                    .expect("request"),
            )
            .await
            .expect("confirm response");
        assert_eq!(confirm.status(), StatusCode::OK);
    }

    #[cfg(feature = "files")]
    #[tokio::test]
    async fn files_only_pull_returns_a_documented_result() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        std::fs::create_dir_all(temp.path().join("input")).expect("create inbox");
        let config_path = temp.path().join(".afmail/config.json");
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("read config"))
                .expect("parse config");
        config["files"] = serde_json::json!({"sources":{
            "inquiries":{"inbox_path":"input","outbox_path":"output"},
            "later":{"inbox_path":"missing-input","outbox_path":"later-output"}
        }});
        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&config).expect("serialize config"),
        )
        .expect("write config");
        std::fs::write(
            temp.path().join("input/one.json"),
            serde_json::json!({
                "schema_name":"channel_message","schema_version":1,
                "external_message_id":"web/one","conversation_id":"inquiry/1",
                "sender_id":"visitor/1","recipient_ids":["support"],
                "subject":"Hello","body_text":"Hi"
            })
            .to_string(),
        )
        .expect("write input");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));
        for body in [
            r#"{"files":true}"#,
            r#"{"files":true,"dry_run":true}"#,
            "{}",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/pull")
                        .header("authorization", format!("Bearer {TOKEN}"))
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK, "{body}");
            let value = response_json(response).await;
            assert_eq!(value["kind"], "result", "{body}: {value}");
            assert_eq!(
                value["result"]["files"]["warnings"][0]["code"], "channel_inbox_unavailable",
                "{body}: {value}"
            );
            assert_eq!(
                value["result"]["files"]["warnings"][0]["source_id"],
                "later"
            );
        }
    }

    #[tokio::test]
    async fn api_writer_obeys_incomplete_transaction_guard() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let transaction_path = temp
            .path()
            .join(".afmail/transactions/transaction_test.json");
        let transaction = serde_json::json!({
            "schema_name": "local_transaction",
            "schema_version": 1,
            "transaction_id": "transaction_test",
            "kind": "case_create",
            "created_rfc3339": "2026-07-22T00:00:00Z",
            "paths": ["cases/support/case_test"]
        });
        std::fs::write(
            transaction_path,
            serde_json::to_vec_pretty(&transaction).expect("serialize transaction"),
        )
        .expect("write transaction");

        let response = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/cases")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("idempotency-key", "case-create-guard-001")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"case_name":"guarded","group":"support","reason":"test"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["error"]["code"], "transaction_incomplete");
    }

    #[tokio::test]
    async fn query_credentials_and_unknown_routes_return_structured_errors() {
        let temp = tempfile::tempdir().expect("create temp dir");
        Workspace::at(temp.path()).init().expect("init workspace");
        let app = router(ApiState::new(temp.path().to_path_buf(), TOKEN.to_string()));

        let query_credential = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/status?access_token={TOKEN}"))
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(query_credential.status(), StatusCode::BAD_REQUEST);

        let unknown = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(unknown.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["kind"], "error");

        let wrong_method = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = to_bytes(wrong_method.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["error"]["code"], "api_method_not_allowed");
    }

    fn case_create_request(request_id: &str, case_name: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/cases")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("idempotency-key", request_id)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "case_name": case_name,
                    "group": "support",
                    "message_id": null,
                    "summary": null,
                    "reason": "test",
                })
                .to_string(),
            ))
            .expect("case create request")
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        serde_json::from_slice(&body).expect("response JSON")
    }
}
