use std::io::{BufRead, Write};

use agent_first_ui::{
    AgentFirstUiMessage, MessagePayload, Operation, OwnerRequest, OwnerRequestResult, Risk,
};
use serde_json::{Map, Value, json};

use crate::error::{AppError, Result};
use crate::store::{DraftChange, Workspace};
use crate::workspace_lock::{LockMode, WorkspaceLock};

const OWNER: &str = "afmail";

/// Serve the trusted AFUI host-side owner bridge over JSONL stdio.
///
/// stdin carries an `OwnerRequest` or a host-owned `{"type":"snapshot"}`
/// control message per line. stdout carries the matching result. Logs must
/// stay off stdout because it is the transport.
pub(super) fn run_owner_bridge() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut bridge = AfmailOwnerBridge::new();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| AppError::io("read AFUI owner request", &error))?;
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(&line)
            .map_err(|error| AppError::json("parse AFUI owner bridge request", &error))?;
        let result = if value.get("type").and_then(Value::as_str) == Some("snapshot") {
            json!({
                "type": "snapshot",
                "snapshot": current_snapshot()?,
            })
        } else {
            let request = serde_json::from_value::<OwnerRequest>(value)
                .map_err(|error| AppError::json("parse AFUI owner request", &error))?;
            serde_json::to_value(bridge.handle(request))
                .map_err(|error| AppError::json("serialize AFUI owner result", &error))?
        };
        serde_json::to_writer(&mut stdout, &result)
            .map_err(|error| AppError::json("serialize AFUI owner result", &error))?;
        stdout
            .write_all(b"\n")
            .map_err(|error| AppError::io("write AFUI owner result", &error))?;
        stdout
            .flush()
            .map_err(|error| AppError::io("flush AFUI owner result", &error))?;
    }
    Ok(())
}

fn current_snapshot() -> Result<AgentFirstUiMessage> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let _lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    crate::ui::workspace_snapshot(&workspace)
}

struct AfmailOwnerBridge {
    sequence: u64,
}

impl AfmailOwnerBridge {
    fn new() -> Self {
        Self { sequence: 0 }
    }

    fn handle(&mut self, request: OwnerRequest) -> OwnerRequestResult {
        if request.owner != OWNER {
            return OwnerRequestResult::rejected(
                &request,
                format!("afmail bridge cannot handle owner `{}`", request.owner),
            );
        }
        if request.risk != Risk::LocalMutation {
            return OwnerRequestResult::rejected(
                &request,
                format!(
                    "afmail UI bridge only accepts local_mutation, got {}",
                    request.risk
                ),
            );
        }
        match self.execute(&request) {
            Ok(output) => match self.refreshed_emit() {
                Ok(emit) => OwnerRequestResult::accepted(&request)
                    .with_output(output)
                    .with_emit(emit),
                Err(error) => OwnerRequestResult::failed(&request, error.message),
            },
            Err(error) => OwnerRequestResult::failed(&request, error.message),
        }
    }

    fn execute(&self, request: &OwnerRequest) -> Result<Value> {
        match request.capability.as_str() {
            "draft_change" => self.change_draft(&request.input),
            other => Err(AppError::new(
                "unsupported_ui_capability",
                format!("afmail UI bridge does not handle `{other}`"),
            )),
        }
    }

    fn change_draft(&self, input: &Map<String, Value>) -> Result<Value> {
        let case_ref = required_string(input, "case_ref")?;
        let draft_name = required_string(input, "draft_name")?;
        let to = required_string_array(input, "to")?;
        let cc = optional_string_array(input, "cc")?;
        let subject = required_string(input, "subject")?;
        let body = required_text(input, "body")?;
        let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
        let workspace = Workspace::discover(&cwd)?;
        let _lock = WorkspaceLock::acquire(workspace.root(), LockMode::Exclusive)?;
        workspace.change_draft(
            case_ref,
            draft_name,
            DraftChange {
                subject: Some(subject),
                to: &to,
                cc: &cc,
                clear_cc: cc.is_empty(),
                identity: None,
                body: Some(body),
                body_file: None,
            },
        )
    }

    fn refreshed_emit(&mut self) -> Result<AgentFirstUiMessage> {
        let snapshot = current_snapshot()?;
        let MessagePayload::Snapshot { space, .. } = snapshot.payload else {
            return Err(AppError::new(
                "afui_invalid",
                "afmail workspace snapshot did not contain a snapshot payload",
            ));
        };
        let section =
            space.as_value().get(OWNER).cloned().ok_or_else(|| {
                AppError::new("afui_invalid", "afmail snapshot section is missing")
            })?;
        self.sequence = self.sequence.saturating_add(1);
        Ok(AgentFirstUiMessage::emit(
            OWNER,
            self.sequence,
            vec![Operation::Replace {
                path: format!("/{OWNER}"),
                value: section,
            }],
        ))
    }
}

fn required_string<'a>(input: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "invalid_ui_input",
                format!("AFUI owner request requires non-empty `{key}`"),
            )
        })
}

fn required_text<'a>(input: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::new(
                "invalid_ui_input",
                format!("AFUI owner request requires non-empty `{key}`"),
            )
        })
}

fn required_string_array(input: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let values = optional_string_array(input, key)?;
    if values.is_empty() {
        return Err(AppError::new(
            "invalid_ui_input",
            format!("AFUI owner request requires at least one `{key}` value"),
        ));
    }
    Ok(values)
}

fn optional_string_array(input: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let Some(value) = input.get(key) else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        return Err(AppError::new(
            "invalid_ui_input",
            format!("AFUI owner request `{key}` must be an array of strings"),
        ));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    AppError::new(
                        "invalid_ui_input",
                        format!("AFUI owner request `{key}` contains an empty non-string value"),
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_first_ui::JsonMap;
    use serde_json::json;

    #[test]
    fn string_arrays_are_strict_and_trimmed() {
        let mut input = JsonMap::new();
        input.insert(
            "to".to_string(),
            json!([" a@example.com ", "b@example.com"]),
        );
        let result = required_string_array(&input, "to");
        assert!(result.is_ok());
        let values = result.unwrap_or_default();
        assert_eq!(
            values,
            vec!["a@example.com".to_string(), "b@example.com".to_string()]
        );
    }

    #[test]
    fn unknown_owner_is_rejected_without_execution() {
        let mut bridge = AfmailOwnerBridge::new();
        let request = OwnerRequest {
            request_id: "request_1".to_string(),
            author: "session_operator".to_string(),
            owner: "other".to_string(),
            capability: "draft_change".to_string(),
            risk: Risk::LocalMutation,
            input: JsonMap::new(),
        };
        let result = bridge.handle(request);
        assert_eq!(
            result.status,
            agent_first_ui::RequestExecutionStatus::Rejected
        );
        assert!(result.emits.is_empty());
    }
}
