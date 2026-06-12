use super::{Cli, Command};
use agent_first_data::{cli_parse_log_filters, cli_parse_output, OutputFormat};
use clap::{CommandFactory, Parser};

pub struct ParsedArgs {
    pub command: Command,
    pub output: OutputFormat,
    pub log: Vec<String>,
}

pub fn parse_args() -> Result<ParsedArgs, String> {
    let cli = Cli::try_parse().map_err(|e| e.to_string())?;
    let output = cli_parse_output(&cli.output)?;
    let log = normalize_log_filters(&cli.log, cli.verbose)?;
    match cli.command {
        Some(command) => Ok(ParsedArgs {
            command,
            output,
            log,
        }),
        None => Err("no command provided; try: afmail --help".to_string()),
    }
}

fn normalize_log_filters(entries: &[String], verbose: bool) -> Result<Vec<String>, String> {
    let mut filters = if verbose {
        vec![
            "startup".to_string(),
            "request".to_string(),
            "progress".to_string(),
            "retry".to_string(),
        ]
    } else {
        Vec::new()
    };
    for entry in cli_parse_log_filters(entries) {
        if !is_supported_log_filter(&entry) {
            return Err(format!(
                "--log unsupported category '{entry}'; expected one of: startup, request, progress, retry"
            ));
        }
        if !filters.contains(&entry) {
            filters.push(entry);
        }
    }
    Ok(filters)
}

fn is_supported_log_filter(value: &str) -> bool {
    matches!(value, "startup" | "request" | "progress" | "retry")
}

pub fn command() -> clap::Command {
    Cli::command()
}
