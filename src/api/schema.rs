use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use super::model::{
    ApiErrorEnvelope, CaseActionRequest, CaseActionResult, CaseCreateRequest, CaseCreateResult,
    CaseListQuery, CaseListResult, CaseMessageAddRequest, CaseMessageAddResult, CaseShowResult,
    CaseSuggestionClearResult, CaseSuggestionSetRequest, CaseSuggestionSetResult,
    DraftCreateRequest, DraftCreateResult, DraftDeleteQuery, DraftDeleteResult, DraftListResult,
    DraftPatchRequest, DraftPatchResult, DraftQueueRequest, DraftQueueResult, DraftShowResult,
    DraftValidateResult, HealthResult, MessageDispositionRequest, MessageDispositionResult,
    MessageListQuery, MessageListResult, MessageShowResult, MessageSuggestionClearResult,
    MessageSuggestionSetRequest, MessageSuggestionSetResult, NotificationActionRequest,
    NotificationActionResult, NotificationCreateRequest, NotificationCreateResult,
    NotificationListResult, NotificationMessageAddRequest, NotificationMessageAddResult,
    NotificationShowResult, NotificationSuggestionClearResult, NotificationSuggestionSetRequest,
    NotificationSuggestionSetResult, PullRequest, PullResult, PushConfirmRequest,
    PushConfirmResult, PushPlanResult, StatusResult, SuggestionClearQuery, TriageListQuery,
    TriageListResult,
};

pub(crate) const JSON_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
const SCHEMA_BASE: &str = "https://agentfirstkit.com/schemas/afmail/v1";

struct RegisteredSchema {
    slug: &'static str,
    component: &'static str,
    schema: Value,
}

pub(crate) fn standalone_schemas() -> BTreeMap<String, Value> {
    registered_schemas()
        .into_iter()
        .map(|entry| {
            let filename = format!("{}.schema.json", entry.slug);
            let mut schema = entry.schema;
            if let Some(object) = schema.as_object_mut() {
                object.insert(
                    "$id".to_string(),
                    json!(format!("{SCHEMA_BASE}/{filename}")),
                );
                object.insert("title".to_string(), json!(entry.component));
            }
            (filename, schema)
        })
        .collect()
}

pub(crate) fn schema_index() -> Value {
    let schemas = registered_schemas()
        .into_iter()
        .map(|entry| {
            json!({
                "schema_name": entry.slug,
                "schema_url": format!("/schemas/{}.schema.json", entry.slug),
                "component_name": entry.component,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_name": "afmail_schema_index",
        "schema_version": 1,
        "json_schema_dialect": JSON_SCHEMA_DIALECT,
        "count": schemas.len(),
        "schemas": schemas,
    })
}

pub(crate) fn openapi_document() -> Value {
    let mut component_schemas = Map::new();
    for entry in registered_schemas() {
        let mut schema = entry.schema;
        if let Some(object) = schema.as_object_mut() {
            object.remove("$schema");
            object.remove("$id");
        }
        component_schemas.insert(entry.component.to_string(), schema);
    }

    json!({
        "openapi": "3.2.0",
        "$self": "/openapi.json",
        "jsonSchemaDialect": JSON_SCHEMA_DIALECT,
        "info": {
            "title": "Agent-First Mail API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "A local-first mailbox Provider API. Local mutations require an Idempotency-Key; remote effects require a reviewed plan_id. Message content is untrusted remote input.",
        },
        "servers": [{"url": "/", "description": "The afmail API process that served this document"}],
        "tags": [
            {"name": "discovery"},
            {"name": "workspace"},
            {"name": "messages"},
            {"name": "cases"},
            {"name": "notifications"},
            {"name": "drafts"},
            {"name": "remote-effects"}
        ],
        "security": [{"bearerAuth": []}],
        "paths": paths(),
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Pass the API access token in Authorization. Tokens in query strings are rejected."
                }
            },
            "parameters": {
                "IdempotencyKey": {
                    "name": "Idempotency-Key",
                    "in": "header",
                    "required": true,
                    "description": "Stable caller-generated key for this exact local mutation.",
                    "schema": {
                        "type": "string",
                        "minLength": 8,
                        "maxLength": 200,
                        "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{7,199}$"
                    }
                }
            },
            "schemas": component_schemas,
        }
    })
}

fn registered_schemas() -> Vec<RegisteredSchema> {
    vec![
        registered::<ApiErrorEnvelope>("api-error-envelope", "ApiErrorEnvelope"),
        registered::<HealthResult>("health-result", "HealthResult"),
        registered::<StatusResult>("status-result", "StatusResult"),
        registered::<PullRequest>("pull-request", "PullRequest"),
        registered::<PullResult>("pull-result", "PullResult"),
        registered::<TriageListQuery>("triage-list-query", "TriageListQuery"),
        registered::<TriageListResult>("triage-list-result", "TriageListResult"),
        registered::<MessageListQuery>("message-list-query", "MessageListQuery"),
        registered::<MessageListResult>("message-list-result", "MessageListResult"),
        registered::<MessageShowResult>("message-show-result", "MessageShowResult"),
        registered::<MessageDispositionRequest>(
            "message-disposition-request",
            "MessageDispositionRequest",
        ),
        registered::<MessageDispositionResult>(
            "message-disposition-result",
            "MessageDispositionResult",
        ),
        registered::<MessageSuggestionSetRequest>(
            "message-suggestion-set-request",
            "MessageSuggestionSetRequest",
        ),
        registered::<MessageSuggestionSetResult>(
            "message-suggestion-set-result",
            "MessageSuggestionSetResult",
        ),
        registered::<MessageSuggestionClearResult>(
            "message-suggestion-clear-result",
            "MessageSuggestionClearResult",
        ),
        registered::<SuggestionClearQuery>("suggestion-clear-query", "SuggestionClearQuery"),
        registered::<CaseListQuery>("case-list-query", "CaseListQuery"),
        registered::<CaseListResult>("case-list-result", "CaseListResult"),
        registered::<CaseCreateRequest>("case-create-request", "CaseCreateRequest"),
        registered::<CaseCreateResult>("case-create-result", "CaseCreateResult"),
        registered::<CaseShowResult>("case-show-result", "CaseShowResult"),
        registered::<CaseMessageAddRequest>("case-message-add-request", "CaseMessageAddRequest"),
        registered::<CaseMessageAddResult>("case-message-add-result", "CaseMessageAddResult"),
        registered::<CaseActionRequest>("case-action-request", "CaseActionRequest"),
        registered::<CaseActionResult>("case-action-result", "CaseActionResult"),
        registered::<CaseSuggestionSetRequest>(
            "case-suggestion-set-request",
            "CaseSuggestionSetRequest",
        ),
        registered::<CaseSuggestionSetResult>(
            "case-suggestion-set-result",
            "CaseSuggestionSetResult",
        ),
        registered::<CaseSuggestionClearResult>(
            "case-suggestion-clear-result",
            "CaseSuggestionClearResult",
        ),
        registered::<NotificationCreateRequest>(
            "notification-create-request",
            "NotificationCreateRequest",
        ),
        registered::<NotificationCreateResult>(
            "notification-create-result",
            "NotificationCreateResult",
        ),
        registered::<NotificationListResult>("notification-list-result", "NotificationListResult"),
        registered::<NotificationShowResult>("notification-show-result", "NotificationShowResult"),
        registered::<NotificationMessageAddRequest>(
            "notification-message-add-request",
            "NotificationMessageAddRequest",
        ),
        registered::<NotificationMessageAddResult>(
            "notification-message-add-result",
            "NotificationMessageAddResult",
        ),
        registered::<NotificationActionRequest>(
            "notification-action-request",
            "NotificationActionRequest",
        ),
        registered::<NotificationActionResult>(
            "notification-action-result",
            "NotificationActionResult",
        ),
        registered::<NotificationSuggestionSetRequest>(
            "notification-suggestion-set-request",
            "NotificationSuggestionSetRequest",
        ),
        registered::<NotificationSuggestionSetResult>(
            "notification-suggestion-set-result",
            "NotificationSuggestionSetResult",
        ),
        registered::<NotificationSuggestionClearResult>(
            "notification-suggestion-clear-result",
            "NotificationSuggestionClearResult",
        ),
        registered::<DraftCreateRequest>("draft-create-request", "DraftCreateRequest"),
        registered::<DraftCreateResult>("draft-create-result", "DraftCreateResult"),
        registered::<DraftPatchRequest>("draft-patch-request", "DraftPatchRequest"),
        registered::<DraftPatchResult>("draft-patch-result", "DraftPatchResult"),
        registered::<DraftDeleteQuery>("draft-delete-query", "DraftDeleteQuery"),
        registered::<DraftDeleteResult>("draft-delete-result", "DraftDeleteResult"),
        registered::<DraftQueueRequest>("draft-queue-request", "DraftQueueRequest"),
        registered::<DraftQueueResult>("draft-queue-result", "DraftQueueResult"),
        registered::<DraftValidateResult>("draft-validate-result", "DraftValidateResult"),
        registered::<DraftListResult>("draft-list-result", "DraftListResult"),
        registered::<DraftShowResult>("draft-show-result", "DraftShowResult"),
        registered::<PushPlanResult>("push-plan-result", "PushPlanResult"),
        registered::<PushConfirmRequest>("push-confirm-request", "PushConfirmRequest"),
        registered::<PushConfirmResult>("push-confirm-result", "PushConfirmResult"),
    ]
}

fn registered<T: JsonSchema>(slug: &'static str, component: &'static str) -> RegisteredSchema {
    RegisteredSchema {
        slug,
        component,
        schema: schema_for::<T>(),
    }
}

fn schema_for<T: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12().with(|settings| {
        settings.inline_subschemas = true;
    });
    let generator = SchemaGenerator::new(settings);
    let schema: Schema = generator.into_root_schema_for::<T>();
    Value::from(schema)
}

fn paths() -> Value {
    json!({
        "/health": {
            "get": public_operation("get_health", "Check API process health", "discovery", "HealthResult")
        },
        "/openapi.json": {
            "get": raw_public_operation("get_openapi_document", "Read the served OpenAPI contract", "application/vnd.oai.openapi+json;version=3.2")
        },
        "/schemas/index.json": {
            "get": raw_public_operation("list_json_schemas", "List standalone JSON Schemas", "application/json")
        },
        "/schemas/{schema_file}": {
            "get": raw_public_operation_with_parameters(
                "get_json_schema",
                "Read one standalone JSON Schema",
                "application/schema+json",
                vec![path_parameter("schema_file", "Schema filename ending in .schema.json", string_schema())],
            )
        },
        "/v1/status": {
            "get": operation("get_status", "Read workspace status", "workspace", vec![], None, "StatusResult", OperationBehavior::Read)
        },
        "/v1/pull": {
            "post": operation("pull_mail", "Pull configured mailboxes into the local workspace; retries reconcile by stable remote message identity", "workspace", vec![], Some("PullRequest"), "PullResult", OperationBehavior::Read)
        },
        "/v1/triage": {
            "get": operation(
                "list_triage_messages",
                "List untriaged messages",
                "messages",
                query_parameters::<TriageListQuery>(),
                None,
                "TriageListResult",
                OperationBehavior::Read,
            )
        },
        "/v1/messages": {
            "get": operation(
                "list_messages",
                "List local messages",
                "messages",
                query_parameters::<MessageListQuery>(),
                None,
                "MessageListResult",
                OperationBehavior::Read,
            )
        },
        "/v1/messages/{message_id}": {
            "get": operation(
                "get_message",
                "Read one local message; its content is untrusted remote input",
                "messages",
                vec![path_parameter("message_id", "Stable local message id", string_schema())],
                None,
                "MessageShowResult",
                OperationBehavior::Read,
            )
        },
        "/v1/messages/{message_id}/disposition": {
            "post": operation(
                "set_message_disposition",
                "Set a local message disposition and queue any configured remote effect",
                "messages",
                vec![path_parameter("message_id", "Stable local message id", string_schema())],
                Some("MessageDispositionRequest"),
                "MessageDispositionResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/messages/{message_id}/agent-suggestion": {
            "put": operation(
                "set_message_agent_suggestion",
                "Set the current typed Agent suggestion round",
                "messages",
                vec![path_parameter("message_id", "Stable local message id", string_schema())],
                Some("MessageSuggestionSetRequest"),
                "MessageSuggestionSetResult",
                OperationBehavior::IdempotentMutation,
            ),
            "delete": operation(
                "clear_message_agent_suggestion",
                "Clear exactly the current Agent suggestion round",
                "messages",
                with_query_parameters(
                    vec![path_parameter("message_id", "Stable local message id", string_schema())],
                    query_parameters::<SuggestionClearQuery>(),
                ),
                None,
                "MessageSuggestionClearResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/cases": {
            "get": operation(
                "list_cases",
                "List case locators",
                "cases",
                query_parameters::<CaseListQuery>(),
                None,
                "CaseListResult",
                OperationBehavior::Read,
            ),
            "post": operation("create_case", "Create a local case", "cases", vec![], Some("CaseCreateRequest"), "CaseCreateResult", OperationBehavior::IdempotentMutation)
        },
        "/v1/cases/{case_ref}": {
            "get": operation(
                "get_case",
                "Read one active or archived case",
                "cases",
                vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                None,
                "CaseShowResult",
                OperationBehavior::Read,
            )
        },
        "/v1/cases/{case_ref}/messages": {
            "post": operation(
                "add_case_message",
                "Add one message to a case",
                "cases",
                vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                Some("CaseMessageAddRequest"),
                "CaseMessageAddResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/cases/{case_ref}/actions": {
            "post": operation(
                "apply_case_action",
                "Apply one typed local case lifecycle action",
                "cases",
                vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                Some("CaseActionRequest"),
                "CaseActionResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/cases/{case_ref}/agent-suggestion": {
            "put": operation(
                "set_case_agent_suggestion",
                "Set the current typed Agent suggestion round",
                "cases",
                vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                Some("CaseSuggestionSetRequest"),
                "CaseSuggestionSetResult",
                OperationBehavior::IdempotentMutation,
            ),
            "delete": operation(
                "clear_case_agent_suggestion",
                "Clear exactly the current Agent suggestion round",
                "cases",
                with_query_parameters(
                    vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                    query_parameters::<SuggestionClearQuery>(),
                ),
                None,
                "CaseSuggestionClearResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/notifications": {
            "get": operation("list_notifications", "List notification collections", "notifications", vec![], None, "NotificationListResult", OperationBehavior::Read),
            "post": operation("create_notification", "Create a local notification collection", "notifications", vec![], Some("NotificationCreateRequest"), "NotificationCreateResult", OperationBehavior::IdempotentMutation)
        },
        "/v1/notifications/{notification_ref}": {
            "get": operation(
                "get_notification",
                "Read one notification collection",
                "notifications",
                vec![path_parameter("notification_ref", "Stable notification ref", string_schema())],
                None,
                "NotificationShowResult",
                OperationBehavior::Read,
            )
        },
        "/v1/notifications/{notification_ref}/messages": {
            "post": operation(
                "add_notification_message",
                "Add one message to a notification collection",
                "notifications",
                vec![path_parameter("notification_ref", "Stable notification ref", string_schema())],
                Some("NotificationMessageAddRequest"),
                "NotificationMessageAddResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/notifications/{notification_ref}/actions": {
            "post": operation(
                "apply_notification_action",
                "Apply one typed notification collection action",
                "notifications",
                vec![path_parameter("notification_ref", "Stable notification ref", string_schema())],
                Some("NotificationActionRequest"),
                "NotificationActionResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/notifications/{notification_ref}/agent-suggestion": {
            "put": operation(
                "set_notification_agent_suggestion",
                "Set the current typed Agent suggestion round",
                "notifications",
                vec![path_parameter("notification_ref", "Stable notification ref", string_schema())],
                Some("NotificationSuggestionSetRequest"),
                "NotificationSuggestionSetResult",
                OperationBehavior::IdempotentMutation,
            ),
            "delete": operation(
                "clear_notification_agent_suggestion",
                "Clear exactly the current Agent suggestion round",
                "notifications",
                with_query_parameters(
                    vec![path_parameter("notification_ref", "Stable notification ref", string_schema())],
                    query_parameters::<SuggestionClearQuery>(),
                ),
                None,
                "NotificationSuggestionClearResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/cases/{case_ref}/drafts": {
            "get": operation(
                "list_case_drafts",
                "List drafts in a case",
                "drafts",
                vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                None,
                "DraftListResult",
                OperationBehavior::Read,
            ),
            "post": operation(
                "create_case_draft",
                "Create a new or reply draft",
                "drafts",
                vec![path_parameter("case_ref", "Stable case ref", string_schema())],
                Some("DraftCreateRequest"),
                "DraftCreateResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/cases/{case_ref}/drafts/{draft_name}": {
            "get": operation(
                "get_case_draft",
                "Read one draft",
                "drafts",
                draft_path_parameters(),
                None,
                "DraftShowResult",
                OperationBehavior::Read,
            ),
            "patch": operation(
                "update_case_draft",
                "Update inline draft fields",
                "drafts",
                draft_path_parameters(),
                Some("DraftPatchRequest"),
                "DraftPatchResult",
                OperationBehavior::IdempotentMutation,
            ),
            "delete": operation(
                "delete_case_draft",
                "Remove one local draft",
                "drafts",
                with_query_parameters(
                    draft_path_parameters(),
                    query_parameters::<DraftDeleteQuery>(),
                ),
                None,
                "DraftDeleteResult",
                OperationBehavior::IdempotentMutation,
            )
        },
        "/v1/cases/{case_ref}/drafts/{draft_name}/validate": {
            "post": operation("validate_case_draft", "Validate one draft and persist validation state", "drafts", draft_path_parameters(), None, "DraftValidateResult", OperationBehavior::IdempotentMutation)
        },
        "/v1/cases/{case_ref}/drafts/{draft_name}/queue": {
            "post": operation("queue_case_draft", "Queue a draft save or send as a local push effect", "drafts", draft_path_parameters(), Some("DraftQueueRequest"), "DraftQueueResult", OperationBehavior::IdempotentMutation)
        },
        "/v1/push/plan": {
            "get": operation("get_push_plan", "Preview the complete resolved remote-effect plan", "remote-effects", vec![], None, "PushPlanResult", OperationBehavior::RemoteEffect)
        },
        "/v1/push/confirm": {
            "post": operation("confirm_push_plan", "Apply exactly the reviewed remote-effect plan", "remote-effects", vec![], Some("PushConfirmRequest"), "PushConfirmResult", OperationBehavior::RemoteEffect)
        }
    })
}

#[derive(Clone, Copy)]
enum OperationBehavior {
    Read,
    IdempotentMutation,
    RemoteEffect,
}

fn operation(
    operation_id: &str,
    summary: &str,
    tag: &str,
    mut parameters: Vec<Value>,
    request_schema: Option<&str>,
    result_schema: &str,
    behavior: OperationBehavior,
) -> Value {
    if matches!(behavior, OperationBehavior::IdempotentMutation) {
        parameters.push(json!({"$ref": "#/components/parameters/IdempotencyKey"}));
    }
    let mut value = json!({
        "operationId": operation_id,
        "summary": summary,
        "tags": [tag],
        "parameters": parameters,
        "x-afmail-remote-effect": matches!(behavior, OperationBehavior::RemoteEffect),
        "responses": standard_responses(result_schema),
    });
    if let Some(request_schema) = request_schema {
        value["requestBody"] = json!({
            "required": true,
            "content": {
                "application/json": {
                    "schema": {"$ref": format!("#/components/schemas/{request_schema}")}
                }
            }
        });
    }
    value
}

fn public_operation(operation_id: &str, summary: &str, tag: &str, result_schema: &str) -> Value {
    let mut value = operation(
        operation_id,
        summary,
        tag,
        Vec::new(),
        None,
        result_schema,
        OperationBehavior::Read,
    );
    value["security"] = json!([]);
    value
}

fn raw_public_operation(operation_id: &str, summary: &str, media_type: &str) -> Value {
    raw_public_operation_with_parameters(operation_id, summary, media_type, Vec::new())
}

fn raw_public_operation_with_parameters(
    operation_id: &str,
    summary: &str,
    media_type: &str,
    parameters: Vec<Value>,
) -> Value {
    json!({
        "operationId": operation_id,
        "summary": summary,
        "tags": ["discovery"],
        "security": [],
        "parameters": parameters,
        "responses": {
            "200": {
                "description": "Document",
                "content": {(media_type): {"schema": {"type": "object"}}}
            },
            "404": error_response("Not found"),
            "405": error_response("Method not allowed")
        }
    })
}

fn standard_responses(result_schema: &str) -> Value {
    json!({
        "200": {
            "description": "Successful AFDATA result envelope",
            "content": {
                "application/json": {
                    "schema": result_envelope_schema(result_schema)
                }
            }
        },
        "400": error_response("Invalid request"),
        "401": authentication_error_response(),
        "404": error_response("Resource not found"),
        "405": error_response("Method not allowed"),
        "409": error_response("Workspace lock, transaction, idempotency, or optimistic-concurrency conflict"),
        "413": error_response("Request body exceeds the documented limit"),
        "415": error_response("Request body is not application/json"),
        "422": error_response("Request was syntactically valid but cannot be applied"),
        "500": error_response("Internal failure")
    })
}

fn result_envelope_schema(result_schema: &str) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "result", "trace"],
        "properties": {
            "kind": {"const": "result"},
            "result": {"$ref": format!("#/components/schemas/{result_schema}")},
            "trace": {
                "type": "object",
                "additionalProperties": false,
                "required": ["duration_ms"],
                "properties": {"duration_ms": {"type": "integer", "minimum": 0}}
            }
        }
    })
}

fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": {"$ref": "#/components/schemas/ApiErrorEnvelope"}
            }
        }
    })
}

fn authentication_error_response() -> Value {
    let mut response = error_response("Missing or invalid bearer credential");
    response["headers"] = json!({
        "WWW-Authenticate": {
            "description": "Bearer challenge",
            "schema": {"type": "string"}
        }
    });
    response
}

fn draft_path_parameters() -> Vec<Value> {
    vec![
        path_parameter("case_ref", "Stable case ref", string_schema()),
        path_parameter("draft_name", "Draft filename", string_schema()),
    ]
}

fn path_parameter(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "description": description,
        "schema": schema,
    })
}

fn query_parameters<T: JsonSchema>() -> Vec<Value> {
    let schema = schema_for::<T>();
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    schema["properties"]
        .as_object()
        .into_iter()
        .flat_map(|properties| properties.iter())
        .map(|(name, property)| {
            let mut property = property.clone();
            normalize_query_parameter_schema(&mut property);
            let mut parameter = json!({
                "name": name,
                "in": "query",
                "required": required.iter().any(|value| value == name),
                "schema": property,
            });
            if let Some(description) = parameter["schema"].get("description").cloned() {
                parameter["description"] = description;
            }
            parameter
        })
        .collect()
}

fn normalize_query_parameter_schema(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if object.get("default").is_some_and(Value::is_null) {
        object.remove("default");
    }
    let Some(types) = object.get("type").and_then(Value::as_array) else {
        return;
    };
    let non_null = types
        .iter()
        .filter(|value| value.as_str() != Some("null"))
        .cloned()
        .collect::<Vec<_>>();
    if non_null.len() == 1 {
        object.insert("type".to_string(), non_null[0].clone());
    } else if !non_null.is_empty() {
        object.insert("type".to_string(), Value::Array(non_null));
    }
}

fn with_query_parameters(mut path: Vec<Value>, query: Vec<Value>) -> Vec<Value> {
    path.extend(query);
    path
}

fn string_schema() -> Value {
    json!({"type": "string", "minLength": 1})
}

#[cfg(test)]
mod tests {
    use super::{JSON_SCHEMA_DIALECT, openapi_document, standalone_schemas};
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn openapi_operation_ids_are_unique_and_schema_refs_resolve() {
        let document = openapi_document();
        assert_eq!(document["openapi"], "3.2.0");
        assert_eq!(document["jsonSchemaDialect"], JSON_SCHEMA_DIALECT);
        let component_names = document["components"]["schemas"]
            .as_object()
            .map(|schemas| schemas.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let mut operation_ids = BTreeSet::new();
        visit(&document["paths"], &mut |value| {
            if let Some(operation_id) = value.get("operationId").and_then(Value::as_str) {
                assert!(operation_ids.insert(operation_id.to_string()));
            }
            if let Some(reference) = value.get("$ref").and_then(Value::as_str)
                && let Some(component) = reference.strip_prefix("#/components/schemas/")
            {
                assert!(
                    component_names.contains(component),
                    "missing schema {component}"
                );
            }
        });
        assert!(operation_ids.len() >= 30);
    }

    #[test]
    fn standalone_schemas_use_2020_12_and_have_stable_ids() {
        let schemas = standalone_schemas();
        assert!(schemas.len() >= 45);
        for (filename, schema) in schemas {
            assert_eq!(schema["$schema"], JSON_SCHEMA_DIALECT, "{filename}");
            assert!(
                schema["$id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with(&filename)),
                "{filename}"
            );
        }
    }

    #[test]
    fn every_local_mutation_declares_idempotency_key() {
        let document = openapi_document();
        for (_, item) in document["paths"].as_object().into_iter().flatten() {
            for (method, operation) in item.as_object().into_iter().flatten() {
                if !matches!(method.as_str(), "post" | "put" | "patch" | "delete")
                    || operation["x-afmail-remote-effect"] == true
                    || operation["operationId"] == "pull_mail"
                {
                    continue;
                }
                let has_key = operation["parameters"]
                    .as_array()
                    .is_some_and(|parameters| {
                        parameters.iter().any(|parameter| {
                            parameter["$ref"] == "#/components/parameters/IdempotencyKey"
                        })
                    });
                assert!(has_key, "{} {method}", operation["operationId"]);
            }
        }
    }

    #[test]
    fn every_afdata_success_response_names_a_typed_result_schema() {
        let document = openapi_document();
        for (_, item) in document["paths"].as_object().into_iter().flatten() {
            for (method, operation) in item.as_object().into_iter().flatten() {
                if !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                    continue;
                }
                let result = &operation["responses"]["200"]["content"]["application/json"]["schema"]
                    ["properties"]["result"];
                if result.is_null() {
                    continue;
                }
                let reference = result["$ref"].as_str().unwrap_or_else(|| {
                    panic!("{} has no typed result ref", operation["operationId"])
                });
                assert!(reference.starts_with("#/components/schemas/"));
                assert!(!reference.ends_with("ObjectResult"));
                assert!(!reference.ends_with("ApiResultEnvelope"));
            }
        }
        assert!(
            document["components"]["schemas"]
                .get("ObjectResult")
                .is_none()
        );
    }

    #[test]
    fn authentication_and_method_errors_are_part_of_the_contract() {
        let document = openapi_document();
        let responses = &document["paths"]["/v1/status"]["get"]["responses"];
        assert!(responses["401"]["headers"]["WWW-Authenticate"].is_object());
        assert!(responses["405"].is_object());
    }

    #[test]
    fn committed_contract_matches_the_rust_source() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi");
        let committed_openapi: Value = serde_json::from_slice(
            &std::fs::read(root.join("openapi.json")).expect("read committed OpenAPI"),
        )
        .expect("parse committed OpenAPI");
        assert_eq!(committed_openapi, openapi_document());

        let expected_schemas = standalone_schemas();
        let committed_names = std::fs::read_dir(root.join("schemas"))
            .expect("read committed schemas")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".schema.json"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            committed_names,
            expected_schemas.keys().cloned().collect::<BTreeSet<_>>()
        );
        for (filename, expected) in expected_schemas {
            let committed: Value = serde_json::from_slice(
                &std::fs::read(root.join("schemas").join(&filename))
                    .expect("read committed schema"),
            )
            .expect("parse committed schema");
            assert_eq!(committed, expected, "{filename}");
        }
    }

    fn visit(value: &Value, callback: &mut impl FnMut(&Value)) {
        callback(value);
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, callback);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    visit(value, callback);
                }
            }
            _ => {}
        }
    }
}
