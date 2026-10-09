use super::*;
use crate::store::doctor::DoctorIssue;

#[derive(Debug, Deserialize, Serialize)]
struct IdempotencyRecord {
    schema_name: String,
    schema_version: u32,
    request_id: String,
    command: String,
    input_hash: String,
    status: String,
    created_rfc3339: String,
    updated_rfc3339: String,
    result: Option<Value>,
}

impl Workspace {
    pub fn run_idempotent_request<F>(
        &self,
        request_id: &str,
        command: &str,
        input: Value,
        operation: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Result<Value>,
    {
        self.run_idempotent_request_with_precheck(
            request_id,
            command,
            input,
            || Ok(()),
            |()| operation(),
        )
    }

    /// Replay an existing request before checking a new operation's inputs.
    ///
    /// `precheck` must not change the workspace or a remote system. Its result
    /// is passed directly to `operation`, after the pending record is written.
    /// A precheck error leaves no record; an operation error keeps the pending
    /// record because the effect may already have happened.
    pub fn run_idempotent_request_with_precheck<T, P, F>(
        &self,
        request_id: &str,
        command: &str,
        input: Value,
        precheck: P,
        operation: F,
    ) -> Result<Value>
    where
        P: FnOnce() -> Result<T>,
        F: FnOnce(T) -> Result<Value>,
    {
        self.require_workspace()?;
        validate_id("request_id", request_id)?;
        validate_id("idempotency command", command)?;
        let path = self.idempotency_record_path(request_id);
        let input_hash = blake3_json_fingerprint(&json!({
            "command": command,
            "input": input,
        }))?;
        if path.exists() {
            let data = read_to_string(&path, "read idempotency record")?;
            let record: IdempotencyRecord = serde_json::from_str(&data)
                .map_err(|error| AppError::json("parse idempotency record", &error))?;
            if record.schema_name != "idempotency_request" || record.schema_version != 1 {
                return Err(AppError::new(
                    "idempotency_record_invalid",
                    format!(
                        "invalid idempotency record schema: {}",
                        path_to_string(&path)
                    ),
                ));
            }
            if record.command != command || record.input_hash != input_hash {
                return Err(AppError::new(
                    "idempotency_conflict",
                    format!("request id {request_id} was already used with different input"),
                )
                .with_details(json!({
                    "request_id": request_id,
                    "command": command,
                    "stored_command": record.command,
                    "input_hash": input_hash,
                    "stored_input_hash": record.input_hash,
                })));
            }
            if record.status == "completed" {
                return record.result.ok_or_else(|| {
                    AppError::new(
                        "idempotency_record_invalid",
                        format!("completed idempotency record has no result: {request_id}"),
                    )
                });
            }
            return Err(AppError::new(
                "idempotency_incomplete",
                format!("request id {request_id} has an incomplete prior attempt"),
            )
            .with_hint("Run `afmail doctor` before retrying this request id; use a new request id only after verifying the previous effect did not complete.")
            .with_details(json!({
                "request_id": request_id,
                "command": command,
                "input_hash": input_hash,
                "record_path": rel_path(&self.root, &path),
            })));
        }

        let prepared = precheck()?;
        let now = now_rfc3339();
        let pending = IdempotencyRecord {
            schema_name: "idempotency_request".to_string(),
            schema_version: 1,
            request_id: request_id.to_string(),
            command: command.to_string(),
            input_hash: input_hash.clone(),
            status: "pending".to_string(),
            created_rfc3339: now.clone(),
            updated_rfc3339: now,
            result: None,
        };
        write_json_pretty(&path, &pending)?;
        let result = operation(prepared)?;
        maybe_fail_idempotency_after_operation(request_id, command)?;
        let completed = IdempotencyRecord {
            schema_name: "idempotency_request".to_string(),
            schema_version: 1,
            request_id: request_id.to_string(),
            command: command.to_string(),
            input_hash,
            status: "completed".to_string(),
            created_rfc3339: pending.created_rfc3339,
            updated_rfc3339: now_rfc3339(),
            result: Some(result.clone()),
        };
        write_json_pretty(&path, &completed)?;
        Ok(result)
    }

    fn idempotency_record_path(&self, request_id: &str) -> PathBuf {
        self.root
            .join(".afmail")
            .join("requests")
            .join(format!("{request_id}.json"))
    }

    pub(super) fn check_idempotency_requests(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let dir = self.root.join(".afmail").join("requests");
        if !dir.exists() {
            return Ok(());
        }
        for entry in read_dir(&dir, "read idempotency requests")? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let rel = rel_path(&self.root, &path);
            let data = read_to_string(&path, "read idempotency request")?;
            let record: IdempotencyRecord = match serde_json::from_str(&data) {
                Ok(record) => record,
                Err(err) => {
                    issues.push(DoctorIssue::error(
                        "idempotency_record_invalid",
                        format!("invalid idempotency request record: {err}"),
                        Some(rel),
                    ));
                    continue;
                }
            };
            if record.schema_name != "idempotency_request" || record.schema_version != 1 {
                issues.push(DoctorIssue::error(
                    "idempotency_record_invalid",
                    "invalid idempotency request schema".to_string(),
                    Some(rel),
                ));
                continue;
            }
            if record.status != "pending" && record.status != "completed" {
                issues.push(DoctorIssue::error(
                    "idempotency_record_invalid",
                    format!("invalid idempotency request status {}", record.status),
                    Some(rel),
                ));
                continue;
            }
            if record.status == "pending" {
                issues.push(DoctorIssue::error(
                    "idempotency_incomplete",
                    format!(
                        "idempotent request {} ({}) did not finish recording its result",
                        record.request_id, record.command
                    ),
                    Some(rel),
                ));
            }
        }
        Ok(())
    }
}

fn maybe_fail_idempotency_after_operation(request_id: &str, command: &str) -> Result<()> {
    if std::env::var("AFMAIL_TEST_FAIL_IDEMPOTENCY_AFTER_OPERATION")
        .ok()
        .as_deref()
        == Some(request_id)
    {
        return Err(AppError::new(
            "idempotency_after_operation_test_failure",
            format!("test failure after idempotent operation {command} for {request_id}"),
        ));
    }
    Ok(())
}
