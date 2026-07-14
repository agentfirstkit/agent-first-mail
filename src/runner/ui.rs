use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_first_data::{CliEmitter, OutputFormat};

use super::output::{emit_result, output_write_failed_exit_code};
use crate::cli::{UiAction, UiCommand};
use crate::error::{AppError, Result};
use crate::store::Workspace;
use crate::types::CommandResult;
use crate::workspace_lock::{LockMode, WorkspaceLock};

const AFMAIL_PACK: &str = include_str!("../../afui/afmail.pack.json");

pub(super) fn run_ui_command(command: UiCommand, output: OutputFormat) -> i32 {
    let started = std::time::Instant::now();
    if matches!(command.action, Some(UiAction::Snapshot)) {
        let result = snapshot_with_lock().and_then(|message| {
            serde_json::to_value(message)
                .map(CommandResult::succeeded)
                .map_err(|error| AppError::json("serialize AFUI snapshot", &error))
        });
        return emit_terminal(result, output, started.elapsed().as_millis() as u64);
    }
    let result = match command.action {
        Some(UiAction::Bridge) => return run_bridge(),
        None => open_web_host(command),
        Some(UiAction::Snapshot) => unreachable!(),
    };
    match result {
        Ok(UiRunResult::ProcessExit(code)) => code,
        Err(err) => emit_terminal(Err(err), output, started.elapsed().as_millis() as u64),
    }
}

fn emit_terminal(result: Result<CommandResult>, output: OutputFormat, duration_ms: u64) -> i32 {
    let stdout = std::io::stdout();
    let mut emitter = CliEmitter::new(stdout.lock(), output).with_strict_protocol();
    match emit_result(&mut emitter, result, duration_ms) {
        Ok(code) => code,
        Err(err) => output_write_failed_exit_code(&err),
    }
}

fn run_bridge() -> i32 {
    match super::ui_bridge::run_owner_bridge() {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

enum UiRunResult {
    ProcessExit(i32),
}

fn open_web_host(command: UiCommand) -> Result<UiRunResult> {
    let afui_bin = resolve_afui_bin(command.afui_bin.as_deref())?;
    let message = snapshot_with_lock()?;
    let snapshot_path = write_temp_snapshot(&message)?;
    let pack_path = match write_temp_pack() {
        Ok(path) => path,
        Err(err) => {
            let _ = fs::remove_file(&snapshot_path);
            return Err(err);
        }
    };
    let owner_spec = owner_sidecar_spec()?;
    let args = web_host_args(
        &command,
        &snapshot_path,
        Some(&pack_path),
        Some(&owner_spec),
    );
    let status = ProcessCommand::new(&afui_bin)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let _ = fs::remove_file(&snapshot_path);
    let _ = fs::remove_file(&pack_path);
    let status = status.map_err(|e| AppError::io("run afui web host", &e))?;
    Ok(UiRunResult::ProcessExit(status.code().unwrap_or(1)))
}

fn web_host_args(
    command: &UiCommand,
    snapshot_path: &Path,
    pack_path: Option<&Path>,
    owner_sidecar: Option<&OsString>,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("web"),
        OsString::from("serve"),
        OsString::from("--host"),
        OsString::from(command.host.as_str()),
        OsString::from("--port"),
        OsString::from(command.port.to_string()),
    ];
    if !command.no_termail {
        args.push(OsString::from("--enable-terminals"));
        args.push(OsString::from("--context-terminal"));
        args.push(OsString::from("termail"));
    }
    if !command.no_open {
        args.push(OsString::from("--open"));
    }
    if let Some(pack_path) = pack_path {
        args.push(OsString::from("--pack"));
        args.push(pack_path.as_os_str().to_os_string());
    }
    if let Some(owner_sidecar) = owner_sidecar {
        args.push(OsString::from("--owner-sidecar"));
        args.push(owner_sidecar.clone());
    }
    args.push(snapshot_path.as_os_str().to_os_string());
    args
}

fn owner_sidecar_spec() -> Result<OsString> {
    let program = env::current_exe().map_err(|error| AppError::io("current executable", &error))?;
    let cwd = env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let spec = serde_json::json!({
        "owner": "afmail",
        "program": program,
        "args": ["ui", "bridge"],
        "cwd": cwd,
    });
    let encoded = serde_json::to_string(&spec)
        .map_err(|error| AppError::json("serialize AFUI owner sidecar spec", &error))?;
    Ok(OsString::from(encoded))
}

fn snapshot_with_lock() -> Result<agent_first_ui::AgentFirstUiMessage> {
    let cwd = env::current_dir().map_err(|e| AppError::io("current dir", &e))?;
    let workspace = Workspace::discover(&cwd)?;
    let _lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    crate::ui::workspace_snapshot(&workspace)
}

fn write_temp_snapshot(message: &agent_first_ui::AgentFirstUiMessage) -> Result<PathBuf> {
    let mut path = env::temp_dir();
    path.push(format!(
        "afmail-ui-{}-{}.afui.json",
        std::process::id(),
        timestamp_nanos()
    ));
    let body = serde_json::to_string_pretty(message)
        .map_err(|e| AppError::json("serialize temporary AFUI snapshot", &e))?;
    fs::write(&path, body).map_err(|e| AppError::io("write temporary AFUI snapshot", &e))?;
    Ok(path)
}

fn write_temp_pack() -> Result<PathBuf> {
    let mut path = env::temp_dir();
    path.push(format!(
        "afmail-ui-{}-{}.pack.json",
        std::process::id(),
        timestamp_nanos()
    ));
    fs::write(&path, AFMAIL_PACK).map_err(|e| AppError::io("write temporary AFUI pack", &e))?;
    Ok(path)
}

fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn resolve_afui_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return resolve_explicit_bin(path);
    }
    if let Some(path) = sibling_afui_bin() {
        return Ok(path);
    }
    if let Some(path) = env::var_os("AFUI_BIN").and_then(|value| resolve_bin_name(&value)) {
        return Ok(path);
    }
    if let Some(path) = resolve_bin_name("afui") {
        return Ok(path);
    }
    Err(afui_not_found_error())
}

pub(crate) fn afui_host_available() -> bool {
    resolve_afui_bin(None).is_ok()
}

fn resolve_explicit_bin(path: &Path) -> Result<PathBuf> {
    if path.components().count() > 1 {
        if is_file(path) {
            return Ok(path.to_path_buf());
        }
    } else if let Some(found) = resolve_bin_name(path.as_os_str()) {
        return Ok(found);
    }
    Err(afui_not_found_error())
}

fn sibling_afui_bin() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(format!("afui{}", env::consts::EXE_SUFFIX));
    is_file(&candidate).then_some(candidate)
}

fn resolve_bin_name(name: impl AsRef<std::ffi::OsStr>) -> Option<PathBuf> {
    let name = name.as_ref();
    let path = Path::new(name);
    if path.components().count() > 1 {
        return is_file(path).then(|| path.to_path_buf());
    }
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        let candidate = dir.join(name);
        if is_file(&candidate) {
            return Some(candidate);
        }
        let candidate = dir.join(format!(
            "{}{}",
            Path::new(name).display(),
            env::consts::EXE_SUFFIX
        ));
        if is_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_file(path: &Path) -> bool {
    path.is_file()
}

fn afui_not_found_error() -> AppError {
    AppError::new("afui_not_found", "afui host binary was not found").with_hint(
        "Install afui or run `afmail ui snapshot > mail.afui.json`, then serve it with `afui web serve mail.afui.json --enable-terminals`.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_as_strings(command: &UiCommand) -> Vec<String> {
        web_host_args(
            command,
            Path::new("snapshot.afui.json"),
            Some(Path::new("afmail.pack.json")),
            Some(&OsString::from(
                r#"{"owner":"afmail","program":"afmail","args":["ui","bridge"]}"#,
            )),
        )
        .into_iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect()
    }

    #[test]
    fn default_web_host_args_open_browser_with_termail_and_owner_bridge() {
        let args = args_as_strings(&UiCommand {
            action: None,
            no_open: false,
            port: 0,
            host: "127.0.0.1".to_string(),
            afui_bin: None,
            no_termail: false,
        });
        assert!(!args.contains(&"--appearance".to_string()));
        assert!(!args.contains(&"dockview".to_string()));
        assert!(args.contains(&"termail".to_string()));
        assert!(args.contains(&"--enable-terminals".to_string()));
        assert!(args.contains(&"--context-terminal".to_string()));
        assert!(args.contains(&"--owner-sidecar".to_string()));
        assert!(args.contains(&"--open".to_string()));
        assert!(args.contains(&"--pack".to_string()));
        assert!(args.contains(&"afmail.pack.json".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("snapshot.afui.json"));
    }

    #[test]
    fn no_termail_disables_companion_and_no_open_omits_browser() {
        let args = args_as_strings(&UiCommand {
            action: None,
            no_open: true,
            port: 8787,
            host: "127.0.0.1".to_string(),
            afui_bin: None,
            no_termail: true,
        });
        assert!(!args.contains(&"--enable-terminals".to_string()));
        assert!(!args.contains(&"--context-terminal".to_string()));
        assert!(!args.contains(&"--open".to_string()));
        assert!(args.contains(&"8787".to_string()));
    }
}
