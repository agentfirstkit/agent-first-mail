use crate::error::{AppError, Result};
use crate::util::write_json_pretty;
use chrono::{DateTime, SecondsFormat, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const WORKSPACE_IDENTITY_SCHEMA_NAME: &str = "workspace_identity";
pub(crate) const WORKSPACE_IDENTITY_SCHEMA_VERSION: u32 = 2;
pub(crate) const WORKSPACE_LAYOUT_VERSION: u32 = 2;
const WORKSPACE_UID_PREFIX: &str = "workspace_";
const WORKSPACE_UID_RANDOM_BYTES: usize = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceIdentity {
    pub(crate) schema_name: String,
    pub(crate) schema_version: u32,
    pub(crate) layout_version: u32,
    pub(crate) workspace_uid: String,
    pub(crate) created_rfc3339: String,
}

impl WorkspaceIdentity {
    fn generate() -> Result<Self> {
        let mut random = [0_u8; WORKSPACE_UID_RANDOM_BYTES];
        SystemRandom::new().fill(&mut random).map_err(|_| {
            AppError::new(
                "workspace_identity_generation_failed",
                "secure random generation failed for workspace identity",
            )
        })?;
        Ok(Self {
            schema_name: WORKSPACE_IDENTITY_SCHEMA_NAME.to_string(),
            schema_version: WORKSPACE_IDENTITY_SCHEMA_VERSION,
            layout_version: WORKSPACE_LAYOUT_VERSION,
            workspace_uid: format!("{WORKSPACE_UID_PREFIX}{}", lowercase_hex(&random)),
            created_rfc3339: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        })
    }

    fn validate(&self, path: &Path) -> Result<()> {
        self.validate_fields(path)?;
        if self.schema_version != WORKSPACE_IDENTITY_SCHEMA_VERSION {
            return Err(invalid_identity_error(
                path,
                format!("schema_version must be {WORKSPACE_IDENTITY_SCHEMA_VERSION}"),
            ));
        }
        if self.layout_version != WORKSPACE_LAYOUT_VERSION {
            return Err(invalid_identity_error(
                path,
                format!("layout_version must be {WORKSPACE_LAYOUT_VERSION}"),
            ));
        }
        Ok(())
    }

    fn validate_fields(&self, path: &Path) -> Result<()> {
        let invalid_reason = if self.schema_name != WORKSPACE_IDENTITY_SCHEMA_NAME {
            Some(format!(
                "schema_name must be {WORKSPACE_IDENTITY_SCHEMA_NAME:?}"
            ))
        } else if !valid_workspace_uid(&self.workspace_uid) {
            Some("workspace_uid must be workspace_ followed by 32 lowercase hex digits".to_string())
        } else if DateTime::parse_from_rfc3339(&self.created_rfc3339).is_err() {
            Some("created_rfc3339 must be a valid RFC 3339 timestamp".to_string())
        } else {
            None
        };
        match invalid_reason {
            Some(reason) => Err(invalid_identity_error(path, reason)),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWorkspaceIdentity {
    schema_name: String,
    schema_version: u32,
    workspace_uid: String,
    created_rfc3339: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceIdentityInitState {
    pub(crate) identity: WorkspaceIdentity,
    pub(crate) layout_version_before: u32,
    pub(crate) workspace_upgraded: bool,
}

/// Strictly load and validate the immutable identity for `workspace_root`.
pub(crate) fn load_workspace_identity(workspace_root: &Path) -> Result<WorkspaceIdentity> {
    let path = identity_path(workspace_root);
    let data = fs::read_to_string(&path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            missing_identity_error(&path)
        } else {
            AppError::io("read workspace identity", &err)
                .with_details(json!({ "workspace_identity_path": path_to_string(&path) }))
        }
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&data)
        .map_err(|err| invalid_identity_error(&path, err.to_string()))?;
    let schema_version = identity_schema_version(&value, &path)?;
    if schema_version == 1 {
        let legacy = serde_json::from_value::<LegacyWorkspaceIdentity>(value)
            .map_err(|err| invalid_identity_error(&path, err.to_string()))?;
        validate_legacy_identity(&legacy, &path)?;
        return Err(workspace_upgrade_required_error(&path, 1));
    }
    if schema_version > u64::from(WORKSPACE_IDENTITY_SCHEMA_VERSION) {
        return Err(workspace_version_unsupported_error(
            &path,
            schema_version,
            value
                .get("layout_version")
                .and_then(serde_json::Value::as_u64),
        ));
    }
    if schema_version != u64::from(WORKSPACE_IDENTITY_SCHEMA_VERSION) {
        return Err(invalid_identity_error(
            &path,
            "unsupported schema_version".to_string(),
        ));
    }
    let identity = serde_json::from_value::<WorkspaceIdentity>(value)
        .map_err(|err| invalid_identity_error(&path, err.to_string()))?;
    identity.validate_fields(&path)?;
    match identity.layout_version.cmp(&WORKSPACE_LAYOUT_VERSION) {
        std::cmp::Ordering::Less => {
            return Err(workspace_upgrade_required_error(
                &path,
                identity.layout_version,
            ));
        }
        std::cmp::Ordering::Greater => {
            return Err(workspace_version_unsupported_error(
                &path,
                u64::from(identity.schema_version),
                Some(u64::from(identity.layout_version)),
            ));
        }
        std::cmp::Ordering::Equal => {}
    }
    Ok(identity)
}

pub(crate) fn workspace_identity_for_init(
    workspace_root: &Path,
) -> Result<WorkspaceIdentityInitState> {
    let path = identity_path(workspace_root);
    if !path.exists() {
        let metadata_root = workspace_root.join(".afmail");
        if metadata_root.exists() && metadata_root_has_workspace_data(&metadata_root)? {
            return Err(missing_identity_error(&path));
        }
        return Ok(WorkspaceIdentityInitState {
            identity: WorkspaceIdentity::generate()?,
            layout_version_before: 0,
            workspace_upgraded: false,
        });
    }
    let data = fs::read_to_string(&path).map_err(|err| {
        AppError::io("read workspace identity", &err)
            .with_details(json!({ "workspace_identity_path": path_to_string(&path) }))
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&data)
        .map_err(|err| invalid_identity_error(&path, err.to_string()))?;
    match identity_schema_version(&value, &path)? {
        1 => {
            let legacy = serde_json::from_value::<LegacyWorkspaceIdentity>(value)
                .map_err(|err| invalid_identity_error(&path, err.to_string()))?;
            validate_legacy_identity(&legacy, &path)?;
            Ok(WorkspaceIdentityInitState {
                identity: WorkspaceIdentity {
                    schema_name: WORKSPACE_IDENTITY_SCHEMA_NAME.to_string(),
                    schema_version: WORKSPACE_IDENTITY_SCHEMA_VERSION,
                    layout_version: WORKSPACE_LAYOUT_VERSION,
                    workspace_uid: legacy.workspace_uid,
                    created_rfc3339: legacy.created_rfc3339,
                },
                layout_version_before: 1,
                workspace_upgraded: true,
            })
        }
        version if version == u64::from(WORKSPACE_IDENTITY_SCHEMA_VERSION) => {
            let mut identity = serde_json::from_value::<WorkspaceIdentity>(value)
                .map_err(|err| invalid_identity_error(&path, err.to_string()))?;
            identity.validate_fields(&path)?;
            if identity.layout_version > WORKSPACE_LAYOUT_VERSION {
                return Err(workspace_version_unsupported_error(
                    &path,
                    version,
                    Some(u64::from(identity.layout_version)),
                ));
            }
            let layout_version_before = identity.layout_version;
            identity.layout_version = WORKSPACE_LAYOUT_VERSION;
            Ok(WorkspaceIdentityInitState {
                identity,
                layout_version_before,
                workspace_upgraded: layout_version_before < WORKSPACE_LAYOUT_VERSION,
            })
        }
        version if version > u64::from(WORKSPACE_IDENTITY_SCHEMA_VERSION) => {
            Err(workspace_version_unsupported_error(
                &path,
                version,
                value
                    .get("layout_version")
                    .and_then(serde_json::Value::as_u64),
            ))
        }
        _ => Err(invalid_identity_error(
            &path,
            "unsupported schema_version".to_string(),
        )),
    }
}

pub(crate) fn write_workspace_identity(
    workspace_root: &Path,
    identity: &WorkspaceIdentity,
) -> Result<()> {
    identity.validate(&identity_path(workspace_root))?;
    write_json_pretty(&identity_path(workspace_root), identity)
}

/// Create an identity only for an otherwise-new metadata directory; existing
/// workspaces must load their original identity instead of silently rotating it.
#[cfg(test)]
pub(crate) fn initialize_workspace_identity(workspace_root: &Path) -> Result<WorkspaceIdentity> {
    let path = identity_path(workspace_root);
    if path.exists() {
        return load_workspace_identity(workspace_root);
    }
    let metadata_root = workspace_root.join(".afmail");
    if metadata_root.exists() && metadata_root_has_workspace_data(&metadata_root)? {
        return Err(missing_identity_error(&path));
    }
    let identity = WorkspaceIdentity::generate()?;
    write_workspace_identity(workspace_root, &identity)?;
    Ok(identity)
}

fn workspace_upgrade_required_error(path: &Path, layout_version_before: u32) -> AppError {
    AppError::new(
        "workspace_upgrade_required",
        "workspace layout must be upgraded before this command can run",
    )
    .with_hint("Run `afmail init` once from this workspace root, then retry the command.")
    .with_details(json!({
        "workspace_identity_path": path_to_string(path),
        "layout_version_before": layout_version_before,
        "layout_version_after": WORKSPACE_LAYOUT_VERSION,
    }))
}

fn workspace_version_unsupported_error(
    path: &Path,
    schema_version: u64,
    layout_version: Option<u64>,
) -> AppError {
    AppError::new(
        "workspace_version_unsupported",
        "workspace was created by a newer afmail version",
    )
    .with_hint(
        "Upgrade afmail to a version that supports this workspace, then retry; do not run `afmail init` with this older binary.",
    )
    .with_details(json!({
        "workspace_identity_path": path_to_string(path),
        "workspace_identity_schema_version": schema_version,
        "supported_workspace_identity_schema_version": WORKSPACE_IDENTITY_SCHEMA_VERSION,
        "workspace_layout_version": layout_version,
        "supported_workspace_layout_version": WORKSPACE_LAYOUT_VERSION,
    }))
}

fn identity_schema_version(value: &serde_json::Value, path: &Path) -> Result<u64> {
    value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            invalid_identity_error(
                path,
                "schema_version must be a non-negative integer".to_string(),
            )
        })
}

fn validate_legacy_identity(identity: &LegacyWorkspaceIdentity, path: &Path) -> Result<()> {
    if identity.schema_name != WORKSPACE_IDENTITY_SCHEMA_NAME
        || identity.schema_version != 1
        || !valid_workspace_uid(&identity.workspace_uid)
        || DateTime::parse_from_rfc3339(&identity.created_rfc3339).is_err()
    {
        return Err(invalid_identity_error(
            path,
            "legacy workspace identity fields are invalid".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn valid_workspace_uid(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix(WORKSPACE_UID_PREFIX) else {
        return false;
    };
    suffix.len() == WORKSPACE_UID_RANDOM_BYTES * 2
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn metadata_root_has_workspace_data(metadata_root: &Path) -> Result<bool> {
    if !metadata_root.is_dir() {
        return Ok(true);
    }
    let entries = fs::read_dir(metadata_root)
        .map_err(|err| AppError::io("inspect workspace metadata", &err))?;
    for entry in entries {
        let entry = entry.map_err(|err| AppError::io("inspect workspace metadata", &err))?;
        if entry.file_name() == "workspace.lock" {
            continue;
        }
        if entry.file_name() == "cache" && entry.path().is_dir() {
            let cache_entries = fs::read_dir(entry.path())
                .map_err(|err| AppError::io("inspect workspace cache", &err))?;
            for cache_entry in cache_entries {
                let cache_entry =
                    cache_entry.map_err(|err| AppError::io("inspect workspace cache", &err))?;
                if cache_entry.file_name() != "workspace.lock" {
                    return Ok(true);
                }
            }
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn identity_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".afmail/workspace.json")
}

fn missing_identity_error(path: &Path) -> AppError {
    AppError::new(
        "workspace_identity_missing",
        "workspace identity is missing; refusing to generate a replacement identity for an existing workspace",
    )
    .with_hint("Restore .afmail/workspace.json from the workspace repository or backup.")
    .with_details(json!({ "workspace_identity_path": path_to_string(path) }))
}

fn invalid_identity_error(path: &Path, reason: String) -> AppError {
    AppError::new(
        "workspace_identity_invalid",
        format!("workspace identity is invalid: {reason}"),
    )
    .with_hint("Restore a valid .afmail/workspace.json from the workspace repository or backup.")
    .with_details(json!({
        "workspace_identity_path": path_to_string(path),
        "reason": reason,
    }))
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "afmail-workspace-identity-{name}-{}-{stamp}",
            std::process::id()
        ))
    }

    #[test]
    fn initialize_generates_and_then_preserves_identity_bytes() -> Result<()> {
        let root = temp_root("stable");
        let first = initialize_workspace_identity(&root)?;
        let path = identity_path(&root);
        let before = fs::read(&path).map_err(|err| AppError::io("read created identity", &err))?;
        let second = initialize_workspace_identity(&root)?;
        let after = fs::read(&path).map_err(|err| AppError::io("read reloaded identity", &err))?;

        assert_eq!(first, second);
        assert_eq!(before, after);
        assert!(valid_workspace_uid(&first.workspace_uid));
        assert_eq!(first.schema_name, WORKSPACE_IDENTITY_SCHEMA_NAME);
        assert_eq!(first.schema_version, WORKSPACE_IDENTITY_SCHEMA_VERSION);
        assert!(DateTime::parse_from_rfc3339(&first.created_rfc3339).is_ok());
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn strict_load_rejects_missing_corrupt_and_unknown_fields() -> Result<()> {
        let root = temp_root("invalid");
        let missing = load_workspace_identity(&root);
        assert_eq!(
            missing.err().map(|error| error.error_code),
            Some("workspace_identity_missing")
        );

        let metadata_root = root.join(".afmail");
        fs::create_dir_all(&metadata_root)
            .map_err(|err| AppError::io("create metadata root", &err))?;
        fs::write(metadata_root.join("workspace.json"), "not json\n")
            .map_err(|err| AppError::io("write corrupt identity", &err))?;
        let corrupt = load_workspace_identity(&root);
        assert_eq!(
            corrupt.err().map(|error| error.error_code),
            Some("workspace_identity_invalid")
        );

        fs::write(
            metadata_root.join("workspace.json"),
            r#"{"schema_name":"workspace_identity","schema_version":1,"workspace_uid":"workspace_0123456789abcdef0123456789abcdef","created_rfc3339":"2026-07-11T00:00:00Z","extra":true}"#,
        )
        .map_err(|err| AppError::io("write identity with unknown field", &err))?;
        let unknown = load_workspace_identity(&root);
        assert_eq!(
            unknown.err().map(|error| error.error_code),
            Some("workspace_identity_invalid")
        );
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn existing_workspace_without_identity_is_not_reidentified() -> Result<()> {
        let root = temp_root("missing-existing");
        fs::create_dir_all(root.join(".afmail/messages"))
            .map_err(|err| AppError::io("create existing metadata", &err))?;
        let error = initialize_workspace_identity(&root);
        assert_eq!(
            error.err().map(|error| error.error_code),
            Some("workspace_identity_missing")
        );
        let _ = fs::remove_dir_all(root);
        Ok(())
    }
}
