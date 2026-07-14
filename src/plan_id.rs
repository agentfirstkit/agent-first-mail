use crate::error::{AppError, Result};
use crate::workspace_identity::valid_workspace_uid;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct PlanHashInput<'a, T> {
    namespace: &'a str,
    workspace_uid: &'a str,
    payload: &'a T,
}

/// Derive a deterministic, workspace-bound plan id from a typed payload.
#[allow(dead_code)] // The push and purge planning slices consume this shared foundation.
pub(crate) fn canonical_plan_id<T: Serialize>(
    namespace: &str,
    workspace_uid: &str,
    payload: &T,
) -> Result<String> {
    validate_namespace(namespace)?;
    if !valid_workspace_uid(workspace_uid) {
        return Err(AppError::new(
            "workspace_identity_invalid",
            "workspace_uid must be workspace_ followed by 32 lowercase hex digits",
        ));
    }
    let input = PlanHashInput {
        namespace,
        workspace_uid,
        payload,
    };
    let value = serde_json::to_value(input)
        .map_err(|err| AppError::json("serialize canonical plan input", &err))?;
    let bytes = serde_json::to_vec(&canonicalize_json(value))
        .map_err(|err| AppError::json("serialize canonical plan bytes", &err))?;
    Ok(format!(
        "{namespace}_plan_{}",
        blake3::hash(&bytes).to_hex()
    ))
}

fn validate_namespace(namespace: &str) -> Result<()> {
    let mut bytes = namespace.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid_namespace_error(namespace));
    };
    if !first.is_ascii_lowercase()
        || !bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(invalid_namespace_error(namespace));
    }
    Ok(())
}

fn invalid_namespace_error(namespace: &str) -> AppError {
    AppError::new(
        "invalid_request",
        format!("plan namespace must match [a-z][a-z0-9_]*: {namespace:?}"),
    )
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_json).collect()),
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut canonical = serde_json::Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonicalize_json(value));
            }
            Value::Object(canonical)
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use serde_json::{Map, json};

    const WORKSPACE_A: &str = "workspace_0123456789abcdef0123456789abcdef";
    const WORKSPACE_B: &str = "workspace_fedcba9876543210fedcba9876543210";

    #[derive(Serialize)]
    struct TestPayload<'a> {
        action: &'a str,
        count: u32,
    }

    #[test]
    fn plan_id_is_stable_and_bound_to_workspace_and_payload() -> Result<()> {
        let payload = TestPayload {
            action: "push",
            count: 2,
        };
        let first = canonical_plan_id("push", WORKSPACE_A, &payload)?;
        let second = canonical_plan_id("push", WORKSPACE_A, &payload)?;
        let other_workspace = canonical_plan_id("push", WORKSPACE_B, &payload)?;
        let other_payload = canonical_plan_id(
            "push",
            WORKSPACE_A,
            &TestPayload {
                action: "push",
                count: 3,
            },
        )?;

        assert_eq!(first, second);
        assert_ne!(first, other_workspace);
        assert_ne!(first, other_payload);
        let hash = first.strip_prefix("push_plan_").unwrap_or_default();
        assert_eq!(hash.len(), 64);
        assert!(
            hash.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        Ok(())
    }

    #[test]
    fn canonical_json_ignores_object_insertion_order() -> Result<()> {
        let mut left = Map::new();
        left.insert("b".to_string(), json!(2));
        left.insert("a".to_string(), json!({"d": 4, "c": 3}));
        let mut right = Map::new();
        right.insert("a".to_string(), json!({"c": 3, "d": 4}));
        right.insert("b".to_string(), json!(2));

        let left_id = canonical_plan_id("purge", WORKSPACE_A, &Value::Object(left))?;
        let right_id = canonical_plan_id("purge", WORKSPACE_A, &Value::Object(right))?;
        assert_eq!(left_id, right_id);
        Ok(())
    }
}
