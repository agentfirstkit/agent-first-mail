use crate::error::Result;
use crate::types::CommandResult;
use agent_first_data::{CliEmitter, CliEmitterError, OutputFormat};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const OUTPUT_WRITE_FAILED_EXIT_CODE: i32 = 4;

struct CommonItemError {
    code: String,
    message: String,
    retryable: bool,
}

pub fn emit_result<W: Write>(
    emitter: &mut CliEmitter<W>,
    result: Result<CommandResult>,
    duration_ms: u64,
) -> std::result::Result<i32, CliEmitterError> {
    match result {
        Ok(result) => {
            let exit_code = result.exit_code();
            let succeeded_count = result.succeeded_count();
            let failed_count = result.failed_count();
            let is_success = result.is_success();
            let aggregate_error = if is_success {
                None
            } else {
                common_item_error(result.value())
            };
            let payload = normalize_command_payload(result.into_value(), is_success);
            let trace = json!({"duration_ms": duration_ms});
            if is_success {
                emitter.emit(agent_first_data::json_result(payload).trace(trace).build())?;
            } else {
                let is_partial = succeeded_count > 0;
                let code = if is_partial {
                    "partial"
                } else {
                    aggregate_error
                        .as_ref()
                        .map_or("command_failed", |error| error.code.as_str())
                };
                let message = aggregate_error.as_ref().map_or_else(
                    || {
                        format!(
                            "{failed_count} of {} items failed",
                            succeeded_count + failed_count
                        )
                    },
                    |error| error.message.clone(),
                );
                let builder = agent_first_data::json_error(code, &message)
                    // `retryable` on the envelope answers "is re-running this
                    // same call safe and worth it". For a partial batch it
                    // never is: some items already took effect — a sent reply,
                    // a moved message — and repeating the call repeats them.
                    // Only a batch where nothing succeeded can inherit the
                    // items' own verdict. The per-item errors keep their
                    // `retryable` either way, which is where a caller looks to
                    // decide what to re-queue.
                    .retryable_if(
                        !is_partial
                            && aggregate_error
                                .as_ref()
                                .is_some_and(|error| error.retryable),
                    )
                    .fields(payload)
                    .trace(trace);
                emitter.emit(builder.build().map_err(CliEmitterError::Build)?)?;
            }
            Ok(exit_code)
        }
        Err(err) => {
            let mut builder = agent_first_data::json_error(err.error_code, &err.message)
                .retryable_if(err.retryable)
                .trace(json!({"duration_ms": duration_ms}));
            if let Some(hint) = err.resolved_hint() {
                builder = builder.hint(hint);
            }
            builder = builder.fields(err.protocol_fields());
            emitter.emit(builder.build().map_err(CliEmitterError::Build)?)?;
            Ok(1)
        }
    }
}

fn common_item_error(value: &Value) -> Option<CommonItemError> {
    let items = value
        .get("results")
        .or_else(|| value.get("failures"))?
        .as_array()?;
    let mut errors = items.iter().map(|item| item.get("error"));
    let first = errors.next()??;
    let code = first.get("code")?.as_str()?;
    let retryable = first
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if errors.any(|error| {
        error
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            != Some(code)
            || error
                .and_then(|value| value.get("retryable"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                != retryable
    }) {
        return None;
    }
    Some(CommonItemError {
        code: code.to_string(),
        message: first.get("message")?.as_str()?.to_string(),
        retryable,
    })
}

fn normalize_command_payload(mut value: Value, success: bool) -> Value {
    if let Value::Object(map) = &mut value {
        let is_typed_review_result = map.contains_key("surface_kind")
            && map.contains_key("result_kind")
            && map.contains_key("interaction_id");
        let is_confirmed_push_result = map.contains_key("confirmed")
            && map.contains_key("plan_id")
            && map.contains_key("workspace_uid")
            && map.contains_key("succeeded_items")
            && map.contains_key("failures");
        map.remove("code");
        map.remove("ok");
        if !is_typed_review_result {
            map.remove("outcome");
        }
        if is_confirmed_push_result {
            for key in ["succeeded_items", "failures"] {
                if let Some(Value::Array(items)) = map.get_mut(key) {
                    for item in items {
                        strip_success_markers(item);
                    }
                }
            }
            return value;
        }
        if let Some(Value::Array(mut items)) = map.remove("results") {
            for item in &mut items {
                strip_success_markers(item);
            }
            if success {
                return Value::Array(items);
            }
            map.remove("succeeded_count");
            map.remove("failed_count");
            map.insert("items".to_string(), Value::Array(items));
        }
        let succeeded_items = map.remove("succeeded_items");
        let failed_items = map.remove("failures");
        if succeeded_items.is_some() || failed_items.is_some() {
            let mut items = succeeded_items
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default();
            items.extend(
                failed_items
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default(),
            );
            for item in &mut items {
                strip_success_markers(item);
            }
            if success {
                return Value::Array(items);
            }
            map.insert("items".to_string(), Value::Array(items));
        }
    }
    value
}

fn strip_success_markers(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    if !map.contains_key("error") {
        map.remove("code");
        map.remove("ok");
        map.remove("outcome");
    }
}

/// Report a failed write of the canonical output and give the process's answer.
///
/// A closed reader is success, the same verdict afdata's `CliEmitter::finish`
/// and `write_raw` give: `afmail … | head -1` is a caller taking what it asked
/// for, not a command that failed. Answering 4 here made one binary give two
/// answers to one pipe — `afmail --docs | head -1` exits 0 because it goes
/// through afdata's writer, while everything else exited 4.
pub fn output_write_failed_exit_code(err: &(impl std::error::Error + 'static)) -> i32 {
    if error_chain_has_broken_pipe(err) {
        return 0;
    }
    emit_output_write_failure(err);
    OUTPUT_WRITE_FAILED_EXIT_CODE
}

#[allow(clippy::disallowed_methods)]
pub(crate) fn emit_output_write_failure(err: &(impl std::error::Error + 'static)) {
    let result = writeln!(
        std::io::stderr(),
        "afmail write_failed: canonical output was not fully written: {err}"
    );
    drop(result);
}

fn error_chain_has_broken_pipe(err: &(impl std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(error) = current {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::BrokenPipe)
        {
            return true;
        }
        current = error.source();
    }
    false
}

pub(super) fn log_event(event: &str, level: &str, fields: Value) -> Value {
    let mut value = match fields {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    if let Value::Object(map) = &mut value {
        let default_message = if map.contains_key("message") {
            None
        } else {
            Some(log_message(event, map))
        };
        map.insert("level".to_string(), json!(level));
        map.insert("event".to_string(), json!(event));
        map.insert(
            "timestamp_epoch_ms".to_string(),
            json!(timestamp_epoch_ms()),
        );
        if let Some(message) = default_message {
            map.insert("message".to_string(), json!(message));
        }
    }
    value
}

pub(super) fn progress_event(fields: Value) -> Value {
    let mut fields = match fields {
        Value::Object(fields) => fields,
        _ => Map::new(),
    };
    fields.insert("event".to_string(), json!("progress"));
    if !fields.contains_key("level") {
        fields.insert("level".to_string(), json!("info"));
    }
    if !fields.contains_key("message") {
        let message = fields
            .get("phase")
            .and_then(Value::as_str)
            .map(|phase| format!("afmail command progress: {phase}"))
            .unwrap_or_else(|| "afmail command progress".to_string());
        fields.insert("message".to_string(), json!(message));
    }
    Value::Object(fields)
}

fn timestamp_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn log_message(event: &str, fields: &Map<String, Value>) -> String {
    match event {
        "startup" => "afmail startup".to_string(),
        "request" => "afmail request started".to_string(),
        "progress" => match fields.get("phase").and_then(Value::as_str) {
            Some("finish") => "afmail command finished".to_string(),
            Some(phase) => format!("afmail command progress: {phase}"),
            None => "afmail command progress".to_string(),
        },
        "retry" => "afmail retryable error".to_string(),
        _ => "afmail log event".to_string(),
    }
}

pub(super) fn redact_log_fields(value: Value) -> Value {
    redact_log_fields_inner(value, None)
}

fn redact_log_fields_inner(value: Value, key: Option<&str>) -> Value {
    match value {
        Value::String(text) if key.is_some_and(is_private_log_key) => {
            json!({
                "redacted": true,
                "original_len_bytes": text.len(),
                "original_hash": stable_text_hash(&text)
            })
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| redact_log_fields_inner(value, key))
                .collect(),
        ),
        Value::Object(map) => {
            let private_kind = map
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(is_private_log_key);
            Value::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        let redaction_key = if private_kind && key == "value" {
                            Some("text")
                        } else {
                            Some(key.as_str())
                        };
                        let redacted = redact_log_fields_inner(value, redaction_key);
                        (key, redacted)
                    })
                    .collect(),
            )
        }
        other => other,
    }
}

/// Names whose values are the user's own content rather than operational
/// metadata: message text, people, addresses.
///
/// None of these is a credential, so AFDATA's `_secret` convention does not
/// reach them and the policy is afmail's own — which is exactly why there is
/// one list. There used to be two, one for log fields and one for argv, and
/// they drifted: `--org`, `--role`, `--phone`, `--sender` and
/// `--subject-contains` were private as flags and public as log fields.
///
/// Spelled in the normalized form [`private_name_key`] produces, so a long flag
/// and a log field are looked up the same way.
const PRIVATE_NAMES: &[&str] = &[
    "bcc",
    "body",
    "body_text",
    "cc",
    "email",
    "error",
    "from",
    "name",
    "org",
    "phone",
    "reason",
    "reply_to",
    "role",
    "sender",
    "subject",
    "subject_contains",
    "summary",
    "text",
    "to",
];

/// Reduce a log field name or a long flag to one spelling.
fn private_name_key(name: &str) -> String {
    name.trim_start_matches('-').replace('-', "_")
}

fn is_private_name(name: &str) -> bool {
    PRIVATE_NAMES.contains(&private_name_key(name).as_str())
}

fn is_private_log_key(key: &str) -> bool {
    is_private_name(key)
}

fn stable_text_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// Redact a command line for the startup diagnostic.
///
/// `command_path` is the path the registry resolved this invocation to, and the
/// command tokens are exactly the argv entries occupying it — a *position*, not
/// a set of known words. The predecessor matched against a hand-kept list of
/// command names, which drifted from the registry in both directions and, worse,
/// could not tell the command `review` from a positional whose value happens to
/// be `review`.
pub(super) fn redact_argv(argv: &[String], command_path: &[String]) -> Vec<String> {
    let mut redact_next = false;
    let mut out = Vec::with_capacity(argv.len());
    for (index, arg) in argv.iter().enumerate() {
        if index == 0 {
            out.push(command_basename(arg));
            continue;
        }
        if index <= command_path.len() {
            out.push(arg.clone());
            continue;
        }
        if redact_next {
            out.push("***".to_string());
            redact_next = false;
            continue;
        }
        if is_secret_assignment(arg) || arg.starts_with("literal:") || arg.contains("=literal:") {
            out.push("***".to_string());
            continue;
        }
        if let Some(name) = arg.split_once('=').map(|(name, _value)| name)
            && is_value_arg_name(name)
        {
            out.push(format!("{name}=***"));
            continue;
        }
        if is_value_arg_name(arg) {
            out.push(arg.clone());
            redact_next = true;
            continue;
        }
        if arg.starts_with('-') {
            out.push(arg.clone());
            continue;
        }
        out.push("***".to_string());
    }
    out
}

pub(super) fn argv_shape(argv: &[String], command_path: &[String]) -> Value {
    let executable = argv
        .first()
        .map(|arg| command_basename(arg))
        .unwrap_or_else(|| "afmail".to_string());
    let mut skip_next_value = false;
    let command_tokens = command_path.to_vec();
    let mut flag_names = BTreeSet::new();
    let mut positional_count = 0usize;
    let mut redacted_value_count = 0usize;

    for arg in argv.iter().skip(1 + command_path.len()) {
        if skip_next_value {
            redacted_value_count += 1;
            skip_next_value = false;
            continue;
        }
        if is_secret_assignment(arg) || arg.starts_with("literal:") || arg.contains("=literal:") {
            positional_count += 1;
            redacted_value_count += 1;
            continue;
        }
        if let Some((name, _value)) = arg.split_once('=')
            && name.starts_with('-')
        {
            flag_names.insert(name.to_string());
            redacted_value_count += 1;
            continue;
        }
        if arg.starts_with('-') {
            flag_names.insert(arg.to_string());
            if is_value_arg_name(arg) {
                skip_next_value = true;
            }
            continue;
        }
        positional_count += 1;
        redacted_value_count += 1;
    }

    json!({
        "executable": executable,
        "command_tokens": command_tokens,
        "flag_names": flag_names.into_iter().collect::<Vec<_>>(),
        "positional_count": positional_count,
        "redacted_value_count": redacted_value_count
    })
}

fn command_basename(arg: &str) -> String {
    Path::new(arg)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("afmail")
        .to_string()
}

fn is_secret_assignment(arg: &str) -> bool {
    arg.contains(".password_secret=")
        || (arg.contains("_secret=") && !arg.contains("_secret_env="))
        || (arg.contains("-secret=") && !arg.contains("-secret-env="))
}

fn is_secret_arg_name(arg: &str) -> bool {
    arg.ends_with(".password_secret")
        || arg.ends_with("_secret")
        || arg.ends_with("-secret")
        || arg == "--password-secret"
}

fn is_value_arg_name(arg: &str) -> bool {
    is_secret_arg_name(arg) || is_private_arg_name(arg) || is_operational_value_arg_name(arg)
}

fn is_private_arg_name(arg: &str) -> bool {
    arg.starts_with("--") && is_private_name(arg)
}

fn is_operational_value_arg_name(arg: &str) -> bool {
    matches!(
        arg,
        "--output"
            | "--output-to"
            | "--log"
            | "--stdout-file"
            | "--stderr-file"
            | "--workspace"
            | "--request-id"
            | "--confirm"
            | "--older-than-days"
            | "--since-days"
            | "--max-new-messages"
            | "--max-total-bytes"
            | "--limit"
            | "--since"
            | "--in"
            | "--group"
            | "--identity"
            | "--body-file"
            | "--agent"
            | "--scope"
            | "--skills-dir"
            | "--path"
            | "--port"
            | "--mode"
            | "--directory"
            | "--bin"
            | "--scenario"
            | "--imap-port"
            | "--smtp-port"
            // A suggestion's action, its target ref, and its review hash. The
            // action's values overlap command literals (`trash`, `archive`,
            // `rename`), so naming the flag here is what keeps the logged
            // command shape from reporting one of them as a subcommand.
            | "--action"
            | "--tag"
            | "--case-ref"
            | "--notification-ref"
            | "--message"
            | "--from-case"
            | "--agent-suggestion-uid"
            | "--confidence-percent"
            | "--message-review-base-hash"
            | "--case-review-base-hash"
            | "--notification-review-base-hash"
    )
}

pub(super) fn output_format_name(output: OutputFormat) -> &'static str {
    match output {
        OutputFormat::Json => "json",
        OutputFormat::Yaml => "yaml",
        OutputFormat::Plain => "plain",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        argv_shape, common_item_error, emit_result, error_chain_has_broken_pipe,
        normalize_command_payload, redact_argv, redact_log_fields,
    };
    use crate::types::CommandResult;
    use agent_first_data::{CliEmitter, OutputFormat};
    use serde_json::json;

    /// Run one aggregate outcome through the real emitter and read back the
    /// terminal event, so these assertions cover what a caller receives rather
    /// than an intermediate the emitter could still reshape.
    fn terminal_event(
        succeeded_count: usize,
        failed_count: usize,
        value: serde_json::Value,
    ) -> serde_json::Value {
        let mut emitter = CliEmitter::new(Vec::new(), OutputFormat::Json);
        let result = CommandResult::aggregate(value, succeeded_count, failed_count);
        emit_result(&mut emitter, Ok(result), 0).expect("the emitter writes to a Vec");
        let written = String::from_utf8(emitter.into_inner()).expect("events are UTF-8");
        let last = written
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .expect("one terminal event");
        serde_json::from_str(last).expect("the terminal event is JSON")
    }

    #[test]
    fn redacts_private_runtime_log_fields_recursively() {
        let value = redact_log_fields(json!({
            "phase": "test_phase",
            "summary": "private summary text",
            "details": {
                "subject": "private subject",
                "body": "private body",
                "count": 2
            },
            "items": [
                {"reason": "private reason"},
                {"kind": "subject", "value": "private field value"},
                {"kind": "safe_kind"}
            ]
        }));

        assert_eq!(value["phase"], "test_phase");
        assert_eq!(value["details"]["count"], 2);
        assert_eq!(value["items"][2]["kind"], "safe_kind");
        for path in [
            &value["summary"],
            &value["details"]["subject"],
            &value["details"]["body"],
            &value["items"][0]["reason"],
            &value["items"][1]["value"],
        ] {
            assert_eq!(path["redacted"], true);
            assert!(path["original_len_bytes"].as_u64().is_some());
            assert!(
                path["original_hash"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("fnv1a64:"))
            );
        }
        let rendered = value.to_string();
        assert!(!rendered.contains("private summary text"));
        assert!(!rendered.contains("private subject"));
        assert!(!rendered.contains("private body"));
        assert!(!rendered.contains("private reason"));
        assert!(!rendered.contains("private field value"));
    }

    #[test]
    fn a_partial_batch_is_never_retryable_as_a_whole() {
        // The shape a confirmed push returns, since that is the command whose
        // exit 3 a caller is told never to retry blindly.
        let push = |succeeded_items: serde_json::Value| {
            json!({
                "confirmed": true,
                "plan_id": "push_plan_1",
                "workspace_uid": "workspace_1",
                "succeeded_items": succeeded_items,
                "failures": [{
                    "error": {
                        "code": "imap_connect_failed",
                        "message": "connection refused",
                        "retryable": true
                    }
                }],
            })
        };

        // Nothing succeeded: no effect has landed, so the batch may inherit the
        // items' own verdict and a caller may re-run the same call.
        let total_failure = terminal_event(0, 1, push(json!([])));
        assert_eq!(total_failure["error"]["code"], "imap_connect_failed");
        assert_eq!(total_failure["error"]["retryable"], true);

        // One item already took effect. Re-running the call would repeat it, so
        // the envelope says no however retryable the individual failures are.
        let partial = terminal_event(1, 1, push(json!([{"push_id": "push_1"}])));
        assert_eq!(partial["error"]["code"], "partial");
        assert_ne!(
            partial["error"]["retryable"], true,
            "a partial batch must not invite a blind retry: {partial}"
        );
        // The per-item verdict survives — that is where a caller looks to decide
        // what to re-queue.
        assert_eq!(partial["error"]["failures"][0]["error"]["retryable"], true);
    }

    #[test]
    fn detects_broken_pipe_through_cli_emitter_error_chain() {
        let error = agent_first_data::CliEmitterError::from(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "closed",
        ));
        assert!(error_chain_has_broken_pipe(&error));
    }

    #[test]
    fn redacts_api_access_token_from_argv() {
        let argv = vec![
            "/tmp/afmail".to_string(),
            "api".to_string(),
            "serve".to_string(),
            "--access-token-secret".to_string(),
            "private-bearer-value".to_string(),
        ];
        let command_path = ["api".to_string(), "serve".to_string()];
        let redacted = redact_argv(&argv, &command_path);
        assert_eq!(redacted[0], "afmail");
        assert_eq!(redacted[1], "api");
        assert_eq!(redacted[2], "serve");
        assert_eq!(redacted[4], "***");
        assert!(!redacted.join(" ").contains("private-bearer-value"));
    }

    #[test]
    fn a_positional_that_reads_like_a_command_is_still_redacted() {
        // The resolved path says which tokens are the command; anything after
        // it is a value however familiar it looks. A name-matching predecessor
        // let `review` through here, printing a case reference in the clear.
        let argv = vec![
            "afmail".to_string(),
            "case".to_string(),
            "notes".to_string(),
            "review".to_string(),
        ];
        let command_path = ["case".to_string(), "notes".to_string()];
        let redacted = redact_argv(&argv, &command_path);
        assert_eq!(redacted, vec!["afmail", "case", "notes", "***"]);

        let shape = argv_shape(&argv, &command_path);
        assert_eq!(shape["command_tokens"], json!(["case", "notes"]));
        assert_eq!(shape["positional_count"], 1);
    }

    #[test]
    fn output_to_value_is_not_counted_as_a_positional() {
        let argv = vec![
            "afmail".to_string(),
            "status".to_string(),
            "--output-to".to_string(),
            "stdout".to_string(),
        ];
        let shape = argv_shape(&argv, &["status".to_string()]);
        assert_eq!(shape["command_tokens"], json!(["status"]));
        assert_eq!(shape["flag_names"], json!(["--output-to"]));
        assert_eq!(shape["positional_count"], 0);
        assert_eq!(shape["redacted_value_count"], 1);
    }

    #[test]
    fn normalization_preserves_only_typed_review_outcome() {
        let review = normalize_command_payload(
            json!({
                "interaction_id":"i1",
                "surface_kind":"case_review",
                "result_kind":"case",
                "outcome":"dismissed",
                "ok":true
            }),
            true,
        );
        assert_eq!(review["outcome"], "dismissed");
        assert!(review.get("ok").is_none());

        let ordinary = normalize_command_payload(
            json!({"surface_kind":"not_enough","outcome":"generic","ok":true}),
            true,
        );
        assert!(ordinary.get("outcome").is_none());
    }

    #[test]
    fn normalization_preserves_confirmed_push_envelope() {
        let value = normalize_command_payload(
            json!({
                "confirmed": true,
                "plan_id": "push_plan_1",
                "workspace_uid": "workspace_1",
                "pushed_count": 1,
                "failed_count": 0,
                "succeeded_items": [{
                    "code": "push_succeeded",
                    "ok": true,
                    "push_id": "push_1"
                }],
                "failures": []
            }),
            true,
        );

        assert_eq!(value["confirmed"], true);
        assert_eq!(value["plan_id"], "push_plan_1");
        assert_eq!(value["pushed_count"], 1);
        assert_eq!(value["succeeded_items"][0]["push_id"], "push_1");
        assert!(value["succeeded_items"][0].get("code").is_none());
        assert!(value["succeeded_items"][0].get("ok").is_none());
    }

    #[test]
    fn common_failure_reads_push_failures_and_retryability() {
        let error = common_item_error(&json!({
            "failures": [
                {
                    "error": {
                        "code": "imap_connect_failed",
                        "message": "connection refused",
                        "retryable": true
                    }
                },
                {
                    "error": {
                        "code": "imap_connect_failed",
                        "message": "connection refused",
                        "retryable": true
                    }
                }
            ]
        }));

        assert_eq!(
            error.as_ref().map(|error| error.code.as_str()),
            Some("imap_connect_failed")
        );
        assert!(error.is_some_and(|error| error.retryable));
    }
}
