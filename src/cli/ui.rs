use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct UiCommand {
    /// UI action. With none, afmail delegates to `afui web serve --appearance dockview`.
    #[command(subcommand)]
    pub action: Option<UiAction>,

    /// Do not open a browser when delegating to the AFUI Web Host.
    #[arg(long)]
    pub no_open: bool,

    /// Web Host bind port. Defaults to 0, letting the OS choose a free port.
    #[arg(long, default_value_t = 0)]
    pub port: u16,

    /// Web Host bind address.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Explicit `afui` binary path or command name for `afmail ui`.
    #[arg(long)]
    pub afui_bin: Option<PathBuf>,

    /// Enable the host-owned local terminal capability (off by default).
    #[arg(long)]
    pub terminal: bool,
}

#[derive(Subcommand, Debug)]
pub enum UiAction {
    /// Emit a complete AFUI snapshot for this workspace.
    Snapshot,
}
