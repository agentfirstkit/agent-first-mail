mod scenario;
mod server;

use agent_first_data::{CliEmitter, CliEmitterError};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::io::Write;
use std::time::Instant;

use crate::cli::{Command, DemoAction, DemoScenario};
use crate::config::{IdentityConfig, MailConfig};
use crate::error::{AppError, Result};
use crate::store::Workspace;
use crate::templates::{TemplateKey, TemplateRenderer};
use crate::types::CommandResult;

use scenario::{DEMO_IDENTITY_EMAIL, DEMO_IDENTITY_NAME, DEMO_USERNAME};

type DemoRunResult<T> = std::result::Result<T, DemoRunError>;

pub(crate) enum DemoRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for DemoRunError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl From<CliEmitterError> for DemoRunError {
    fn from(value: CliEmitterError) -> Self {
        Self::Output(value)
    }
}

pub(crate) fn is_demo_serve_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Demo {
            action: DemoAction::Serve { .. }
        }
    )
}

pub(crate) fn init(
    cwd: &std::path::Path,
    path: &str,
    scenario: DemoScenario,
    imap_port: u16,
    smtp_port: u16,
) -> Result<Value> {
    server::ports_are_bindable(imap_port, smtp_port)?;
    let password_secret = demo_password_secret()?;
    let (workspace, mut result) = Workspace::init_empty_command(cwd, path)?;
    let mut config = MailConfig::load(workspace.root())?;
    config.imap.host = Some("127.0.0.1".to_string());
    config.imap.port = imap_port;
    config.imap.tls = false;
    config.imap.username = Some(DEMO_USERNAME.to_string());
    config.imap.password_secret = Some(password_secret.clone());
    config.smtp.host = Some("127.0.0.1".to_string());
    config.smtp.port = smtp_port;
    config.smtp.starttls = false;
    config.smtp.tls_wrapper = false;
    config.smtp.username = Some(DEMO_USERNAME.to_string());
    config.smtp.password_secret = Some(password_secret);
    config.identities = vec![IdentityConfig {
        identity: "founder".to_string(),
        name: DEMO_IDENTITY_NAME.to_string(),
        email: DEMO_IDENTITY_EMAIL.to_string(),
        default: true,
    }];
    config.write(workspace.root())?;

    let agent_prompt = "Open this afmail workspace, pull the mail, triage it, and draft replies to anything that needs action. Show me the push plan before confirming.";
    let workspace_ref = result
        .get("workspace_ref")
        .and_then(Value::as_str)
        .unwrap_or(path)
        .to_string();
    let handoff = DemoHandoff {
        workspace_ref: &workspace_ref,
        agent_prompt,
    };
    let mut renderer = TemplateRenderer::new(workspace.root(), config.template_language());
    let handoff_text = renderer.render(TemplateKey::DemoHandoff, &handoff)?;

    let Value::Object(result) = &mut result else {
        return Err(AppError::new(
            "internal",
            "workspace init returned a non-object result",
        ));
    };
    append_changed_file(result, ".afmail/config.json");
    result.insert("scenario".to_string(), json!(scenario.as_str()));
    result.insert(
        "message_count".to_string(),
        json!(scenario::message_count(scenario)),
    );
    result.insert(
        "imap_endpoint".to_string(),
        json!(format!("127.0.0.1:{imap_port}")),
    );
    result.insert(
        "smtp_endpoint".to_string(),
        json!(format!("127.0.0.1:{smtp_port}")),
    );
    result.insert("demo_serve_command".to_string(), json!("afmail demo serve"));
    result.insert("agent_prompt".to_string(), json!(agent_prompt));
    result.insert("handoff_text".to_string(), json!(handoff_text));
    result.insert(
        "next_steps".to_string(),
        json!([
            format!("Run `afmail demo serve` with workdir set to {workspace_ref}."),
            "Run `afmail pull` after the demo_ready event.".to_string(),
            "Use ordinary afmail triage, review, case, notification, draft, and push commands.".to_string(),
            "Preview with bare `afmail push`; apply only with `afmail push --confirm PLAN_ID` after review.".to_string(),
        ]),
    );
    Ok(Value::Object(result.clone()))
}

pub(crate) fn run_serve_command<W: Write>(
    command: &Command,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> DemoRunResult<CommandResult> {
    let Command::Demo {
        action: DemoAction::Serve { verbose },
    } = command
    else {
        return Err(AppError::new(
            "demo_dispatch_error",
            "run_serve_command requires `afmail demo serve`",
        )
        .into());
    };
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    workspace.require_workspace()?;
    let config = MailConfig::load(workspace.root())?;
    let imap = config.require_imap()?;
    let smtp = config.require_smtp()?;
    validate_demo_server_config(&imap, &smtp)?;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let servers = server::start(
        imap.port,
        smtp.port,
        DemoScenario::FounderInbox,
        &imap.username,
        &imap.password_secret,
        *verbose,
        event_tx,
    )?;
    let imap_endpoint = servers.imap_address.to_string();
    let smtp_endpoint = servers.smtp_address.to_string();
    emitter.emit(
        agent_first_data::json_progress(json!({
            "phase": "demo_ready",
            "message": "The local demo IMAP and SMTP servers are ready.",
            "scenario": DemoScenario::FounderInbox.as_str(),
            "message_count": scenario::message_count(DemoScenario::FounderInbox),
            "imap_endpoint": imap_endpoint,
            "smtp_endpoint": smtp_endpoint,
        }))
        .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
        .build(),
    )?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| AppError::io("create demo server runtime", &error))?;
    let loop_result: DemoRunResult<()> = runtime.block_on(async {
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal
                        .map_err(|error| AppError::io("wait for demo server shutdown", &error))
                        .map_err(DemoRunError::App)?;
                    break Ok::<(), DemoRunError>(());
                }
                event = event_rx.recv() => {
                    let Some(event) = event else {
                        break Err(DemoRunError::App(AppError::new(
                            "demo_server_stopped",
                            "the demo mail server stopped unexpectedly",
                        )));
                    };
                    emitter.emit(
                        agent_first_data::json_log(event)
                            .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
                            .build(),
                    ).map_err(DemoRunError::Output)?;
                }
            }
        }
    });
    servers.stop();
    loop_result?;
    Ok(CommandResult::succeeded(json!({
        "scenario": DemoScenario::FounderInbox.as_str(),
        "imap_endpoint": imap_endpoint,
        "smtp_endpoint": smtp_endpoint,
        "stopped": true,
    })))
}

fn validate_demo_server_config(
    imap: &crate::config::ImapConfig,
    smtp: &crate::config::SmtpConfig,
) -> Result<()> {
    if !crate::util::is_loopback_host(&imap.host)
        || !crate::util::is_loopback_host(&smtp.host)
        || imap.tls
        || smtp.starttls
        || smtp.tls_wrapper
    {
        return Err(AppError::new(
            "demo_config_invalid",
            "demo serve requires plaintext IMAP and SMTP endpoints on a loopback host",
        )
        .with_hint("Run `afmail demo init` to create a correctly isolated demo workspace."));
    }
    if smtp.username.as_deref() != Some(imap.username.as_str())
        || smtp.password_secret.as_deref() != Some(imap.password_secret.as_str())
    {
        return Err(AppError::new(
            "demo_config_invalid",
            "demo IMAP and SMTP credentials must match",
        ));
    }
    Ok(())
}

fn demo_password_secret() -> Result<String> {
    let mut bytes = [0_u8; 24];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        AppError::new(
            "demo_credential_generation_failed",
            "could not generate the demo mail server credential",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn append_changed_file(result: &mut Map<String, Value>, path: &str) {
    let changed_files = result
        .entry("changed_files".to_string())
        .or_insert_with(|| json!([]));
    if let Value::Array(changed_files) = changed_files
        && !changed_files.iter().any(|value| value == path)
    {
        changed_files.push(json!(path));
    }
}

#[derive(Serialize)]
struct DemoHandoff<'a> {
    workspace_ref: &'a str,
    agent_prompt: &'a str,
}

#[cfg(test)]
mod tests {
    use super::{demo_password_secret, validate_demo_server_config};
    use crate::config::{ImapConfig, SmtpConfig};

    #[test]
    fn generated_demo_credentials_are_bearer_safe_and_nontrivial() {
        let secret = demo_password_secret().expect("credential");
        assert_eq!(secret.len(), 48);
        assert!(secret.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn demo_server_refuses_non_loopback_or_tls_configuration() {
        let imap = ImapConfig {
            host: "mail.example.com".to_string(),
            port: 143,
            tls: false,
            tls_root_cert_path: None,
            allow_unbounded_body_fetch: false,
            username: "founder".to_string(),
            password_secret: "secret".to_string(),
            mailboxes: Vec::new(),
        };
        let smtp = SmtpConfig {
            host: "127.0.0.1".to_string(),
            port: 25,
            starttls: false,
            tls_wrapper: false,
            username: Some("founder".to_string()),
            password_secret: Some("secret".to_string()),
        };
        let error = validate_demo_server_config(&imap, &smtp)
            .expect_err("non-loopback IMAP must be rejected");
        assert_eq!(error.error_code, "demo_config_invalid");
    }
}
