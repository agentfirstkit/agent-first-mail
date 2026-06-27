use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_first_data::OutputFormat;

use super::output::emit_result;
use crate::cli::{UiAction, UiCommand};
use crate::error::{AppError, Result};
use crate::store::Workspace;
use crate::workspace_lock::{LockMode, WorkspaceLock};

pub(super) fn run_ui_command(command: UiCommand, output: OutputFormat) -> i32 {
    let started = std::time::Instant::now();
    let result = match command.action {
        Some(UiAction::Snapshot) => emit_snapshot(),
        None => open_web_host(command),
    };
    match result {
        Ok(UiRunResult::SnapshotWritten) => 0,
        Ok(UiRunResult::ProcessExit(code)) => code,
        Err(err) => emit_result(Err(err), output, started.elapsed().as_millis() as u64),
    }
}

enum UiRunResult {
    SnapshotWritten,
    ProcessExit(i32),
}

fn emit_snapshot() -> Result<UiRunResult> {
    let message = snapshot_with_lock()?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &message)
        .map_err(|e| AppError::json("serialize AFUI snapshot", &e))?;
    writeln!(handle).map_err(|e| AppError::io("write AFUI snapshot", &e))?;
    Ok(UiRunResult::SnapshotWritten)
}

fn open_web_host(command: UiCommand) -> Result<UiRunResult> {
    let afui_bin = resolve_afui_bin(command.afui_bin.as_deref())?;
    let message = snapshot_with_lock()?;
    let snapshot_path = write_temp_snapshot(&message)?;
    let args = web_host_args(&command, &snapshot_path);
    let status = ProcessCommand::new(&afui_bin)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let _ = fs::remove_file(&snapshot_path);
    let status = status.map_err(|e| AppError::io("run afui web host", &e))?;
    Ok(UiRunResult::ProcessExit(status.code().unwrap_or(1)))
}

fn web_host_args(command: &UiCommand, snapshot_path: &Path) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("web"),
        OsString::from("serve"),
        OsString::from("--host"),
        OsString::from(command.host.as_str()),
        OsString::from("--port"),
        OsString::from(command.port.to_string()),
    ];
    if command.terminal {
        args.push(OsString::from("--enable-terminals"));
    }
    if !command.no_open {
        args.push(OsString::from("--open"));
    }
    args.push(snapshot_path.as_os_str().to_os_string());
    args
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
        web_host_args(command, Path::new("snapshot.afui.json"))
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn default_web_host_args_open_browser_without_terminal_or_appearance() {
        let args = args_as_strings(&UiCommand {
            action: None,
            no_open: false,
            port: 0,
            host: "127.0.0.1".to_string(),
            afui_bin: None,
            terminal: false,
        });
        assert!(!args.contains(&"--appearance".to_string()));
        assert!(!args.contains(&"dockview".to_string()));
        assert!(!args.contains(&"termail".to_string()));
        assert!(!args.contains(&"--enable-terminals".to_string()));
        assert!(args.contains(&"--open".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("snapshot.afui.json"));
    }

    #[test]
    fn terminal_flag_enables_terminal_and_no_open_omits_browser() {
        let args = args_as_strings(&UiCommand {
            action: None,
            no_open: true,
            port: 8787,
            host: "127.0.0.1".to_string(),
            afui_bin: None,
            terminal: true,
        });
        assert!(args.contains(&"--enable-terminals".to_string()));
        assert!(!args.contains(&"--open".to_string()));
        assert!(args.contains(&"8787".to_string()));
    }
}
