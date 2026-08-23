//! Typed HTTP API, OpenAPI document, and standalone JSON Schema export.

mod model;
mod schema;
mod server;

use std::collections::BTreeSet;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};
use serde_json::{Value, json};

use crate::cli::{ApiAction, ApiMode, Command};
use crate::error::AppError;
use crate::store::Workspace;
use crate::types::CommandResult;

type ApiRunResult<T> = std::result::Result<T, ApiRunError>;

pub(crate) fn is_api_command(command: &Command) -> bool {
    matches!(command, Command::Api { .. })
}

pub(crate) fn run_api_command<W: Write>(
    command: &Command,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ApiRunResult<CommandResult> {
    let Command::Api { action } = command else {
        return Err(AppError::new(
            "api_dispatch_error",
            "run_api_command requires an api command",
        )
        .into());
    };
    match action {
        ApiAction::Export { directory, force } => Ok(CommandResult::succeeded(export_contract(
            Path::new(directory),
            *force,
        )?)),
        ApiAction::Serve {
            mode,
            port,
            access_token_secret,
        } => serve_api(
            *mode,
            *port,
            access_token_secret.as_deref(),
            emitter,
            started,
        )
        .map(CommandResult::succeeded),
    }
}

pub(crate) enum ApiRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for ApiRunError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl From<CliEmitterError> for ApiRunError {
    fn from(value: CliEmitterError) -> Self {
        Self::Output(value)
    }
}

fn serve_api<W: Write>(
    mode: ApiMode,
    port: u16,
    access_token_secret: Option<&str>,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ApiRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    workspace.require_workspace()?;
    let root = workspace.root().to_path_buf();
    let token = resolve_access_token(access_token_secret)?;
    let bind_ip = match mode {
        ApiMode::Local => Ipv4Addr::LOCALHOST,
        ApiMode::Lan => Ipv4Addr::UNSPECIFIED,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| AppError::io("create API runtime", &error))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind((bind_ip, port))
            .await
            .map_err(|error| AppError::io("bind afmail API", &error))?;
        let address = listener
            .local_addr()
            .map_err(|error| AppError::io("read afmail API address", &error))?;
        let advertised_ip = match mode {
            ApiMode::Local => Ipv4Addr::LOCALHOST,
            ApiMode::Lan => primary_lan_ipv4()?,
        };
        let api_url = format!("http://{advertised_ip}:{}", address.port());
        emitter.emit(
            agent_first_data::json_progress(json!({
                "phase": "api_ready",
                "message": match mode {
                    ApiMode::Local => "The afmail API is available on this machine.",
                    ApiMode::Lan => "OpenAPI is available on the trusted local network; keep the bearer credential private.",
                },
                "api_url": api_url,
                "openapi_url": format!("{api_url}/openapi.json"),
                "schema_index_url": format!("{api_url}/schemas/index.json"),
                "mode": api_mode_name(mode),
                "port": address.port(),
            }))
            .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
            .build(),
        )?;
        let app = server::router(server::ApiState::new(root, token));
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|error| AppError::io("serve afmail API", &error))?;
        Ok(json!({
            "mode": api_mode_name(mode),
            "api_url": api_url,
            "stopped": true,
        }))
    })
}

async fn shutdown_signal() {
    let _result = tokio::signal::ctrl_c().await;
}

fn resolve_access_token(explicit: Option<&str>) -> Result<String, AppError> {
    let token = match explicit {
        Some(token) => token.to_string(),
        None => std::env::var("AFMAIL_API_ACCESS_TOKEN_SECRET").map_err(|_| {
            AppError::new(
                "api_access_token_required",
                "set AFMAIL_API_ACCESS_TOKEN_SECRET or pass --access-token-secret",
            )
            .with_hint(
                "Use a random bearer-safe ASCII credential of at least 32 characters; prefer the environment variable so it is not exposed in argv.",
            )
        })?,
    };
    let bearer_safe = token.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')
    });
    if !(32..=512).contains(&token.len()) || !bearer_safe {
        return Err(AppError::new(
            "api_access_token_invalid",
            "the API access token must contain 32 through 512 bearer-safe ASCII characters",
        ));
    }
    Ok(token)
}

fn api_mode_name(mode: ApiMode) -> &'static str {
    match mode {
        ApiMode::Local => "local",
        ApiMode::Lan => "lan",
    }
}

fn primary_lan_ipv4() -> Result<Ipv4Addr, AppError> {
    agent_first_ui::primary_lan_ipv4().map_err(|error| {
        AppError::new(
            "api_lan_unavailable",
            format!("could not select an IPv4 address for LAN API serving: {error}"),
        )
        .with_hint("Connect this machine to a trusted IPv4 local network and retry.")
    })
}

fn export_contract(directory: &Path, force: bool) -> Result<Value, AppError> {
    let openapi_path = directory.join("openapi.json");
    let schemas_directory = directory.join("schemas");
    let index_path = schemas_directory.join("index.json");
    let schemas = schema::standalone_schemas();
    let mut targets = vec![openapi_path.clone(), index_path.clone()];
    targets.extend(
        schemas
            .keys()
            .map(|filename| schemas_directory.join(filename)),
    );
    if !force && let Some(existing) = targets.iter().find(|path| path.exists()) {
        return Err(AppError::new(
            "api_contract_exists",
            format!(
                "generated API contract already exists: {}",
                display_path(existing)
            ),
        )
        .with_hint("Repeat `afmail api export --force` to replace generated contract files."));
    }
    if force && schemas_directory.is_dir() {
        remove_stale_schema_files(&schemas_directory, schemas.keys().map(String::as_str))?;
    }
    crate::util::write_json_pretty(&openapi_path, &schema::openapi_document())?;
    crate::util::write_json_pretty(&index_path, &schema::schema_index())?;
    for (filename, value) in &schemas {
        crate::util::write_json_pretty(&schemas_directory.join(filename), value)?;
    }
    Ok(json!({
        "openapi_path": display_path(&openapi_path),
        "schema_index_path": display_path(&index_path),
        "schema_count": schemas.len(),
        "schema_directory_path": display_path(&schemas_directory),
    }))
}

fn remove_stale_schema_files<'a>(
    directory: &Path,
    current_names: impl Iterator<Item = &'a str>,
) -> Result<(), AppError> {
    let current_names = current_names.collect::<BTreeSet<_>>();
    let entries = std::fs::read_dir(directory)
        .map_err(|error| AppError::io("read schema export directory", &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| AppError::io("read schema export entry", &error))?;
        let path = entry.path();
        let filename = entry.file_name().to_string_lossy().to_string();
        if path.is_file()
            && filename.ends_with(".schema.json")
            && !current_names.contains(filename.as_str())
        {
            std::fs::remove_file(&path)
                .map_err(|error| AppError::io("remove stale generated schema", &error))?;
        }
    }
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{export_contract, resolve_access_token};

    #[test]
    fn access_token_validation_does_not_echo_the_token() {
        let token = "short-secret";
        let error = resolve_access_token(Some(token)).expect_err("reject short token");
        assert_eq!(error.error_code, "api_access_token_invalid");
        assert!(!error.message.contains(token));

        let non_ascii = "密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥密钥";
        let error = resolve_access_token(Some(non_ascii)).expect_err("reject non-ASCII token");
        assert_eq!(error.error_code, "api_access_token_invalid");
        assert!(!error.message.contains(non_ascii));
    }

    #[test]
    fn export_requires_force_before_replacing_contract_files() {
        let temp = tempfile::tempdir().expect("create temp dir");
        export_contract(temp.path(), false).expect("first export");
        let error = export_contract(temp.path(), false).expect_err("reject replacement");
        assert_eq!(error.error_code, "api_contract_exists");
        export_contract(temp.path(), true).expect("forced replacement");
    }
}
