use super::{Cli, Command};
use agent_first_data::{LogFilters, OutputFormat, cli_parse_log_filters, cli_parse_output};
use clap::{CommandFactory, Parser};

pub struct ParsedArgs {
    pub command: Command,
    pub output: OutputFormat,
    pub log: LogFilters,
    pub workspace: Option<String>,
    pub request_id: Option<String>,
}

pub fn parse_args() -> Result<ParsedArgs, String> {
    let cli = Cli::try_parse().map_err(|e| e.to_string())?;
    let _stream_redirect_args = (&cli.stdout_file, &cli.stderr_file);
    let output = cli_parse_output(&cli.output)?;
    let log = normalize_log_filters(&cli.log, cli.verbose)?;
    match cli.command {
        Some(command) => {
            if cli.workspace.is_some() && matches!(command, Command::Init { .. }) {
                return Err(
                    "--workspace selects an existing workspace and cannot be used with init"
                        .to_string(),
                );
            }
            Ok(ParsedArgs {
                command,
                output,
                log,
                workspace: cli.workspace,
                request_id: cli.request_id,
            })
        }
        None => Err("no command provided; try: afmail --help".to_string()),
    }
}

fn normalize_log_filters(entries: &[String], verbose: bool) -> Result<LogFilters, String> {
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
    for entry in cli_parse_log_filters(entries).as_slice() {
        if !is_supported_log_filter(entry) {
            return Err(format!(
                "--log unsupported category '{entry}'; expected one of: startup, request, progress, retry"
            ));
        }
        if !filters.contains(entry) {
            filters.push(entry.to_string());
        }
    }
    Ok(LogFilters::new(filters))
}

fn is_supported_log_filter(value: &str) -> bool {
    matches!(value, "startup" | "request" | "progress" | "retry")
}

pub fn command() -> clap::Command {
    Cli::command()
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;
    use serde_json::Value;

    const README: &str = include_str!("../../README.md");
    const SKILL: &str = include_str!("../../skills/agent-first-mail.md");
    const FILE_FORMATS: &str = include_str!("../../docs/file-formats.md");
    const SPORE_CORE: &str = include_str!("../../spore.core.json");

    #[test]
    fn readme_main_journey_afmail_commands_parse() {
        let commands = fenced_commands(README);
        assert!(!commands.is_empty());
        assert_commands_parse("README.md", &commands);
    }

    #[test]
    fn embedded_skill_afmail_commands_parse() {
        let commands = inline_commands(SKILL);
        assert!(
            commands.len() > 20,
            "expected broad skill command coverage, got {commands:?}"
        );
        assert_commands_parse("skills/agent-first-mail.md", &commands);
    }

    #[test]
    fn file_format_json_fixtures_parse() {
        let blocks = fenced_json_blocks(FILE_FORMATS);
        assert!(
            blocks.len() > 8,
            "expected file-format JSON fixtures, got {}",
            blocks.len()
        );
        for (index, block) in blocks.iter().enumerate() {
            let parsed = serde_json::from_str::<Value>(block);
            assert!(
                parsed.is_ok(),
                "docs/file-formats.md JSON block #{index} must parse: {block}"
            );
        }
    }

    #[test]
    fn manifest_intent_covers_cli_capability_inventory() {
        let manifest = serde_json::from_str::<Value>(SPORE_CORE);
        assert!(manifest.is_ok(), "spore.core.json must parse");
        let Ok(manifest) = manifest else {
            return;
        };
        let intent = manifest["intent"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .to_lowercase()
            })
            .unwrap_or_default();

        for required in [
            "pull",
            "triage",
            "case",
            "notification",
            "contact",
            "draft",
            "push",
            "audit",
            "queue",
            "remote",
            "attachment",
        ] {
            assert!(
                intent.contains(required),
                "spore.core.json intent must mention CLI capability '{required}'"
            );
        }
    }

    fn assert_commands_parse(source: &str, commands: &[String]) {
        for command in commands {
            let argv = tokenize_command(command);
            assert!(
                argv.len() > 1,
                "{source} command should include an afmail subcommand: {command}"
            );
            let parsed = Cli::try_parse_from(&argv);
            assert!(
                parsed.is_ok(),
                "{source} command must parse with Clap: {command}\nargv: {argv:?}\nerror: {parsed:?}"
            );
        }
    }

    fn fenced_commands(text: &str) -> Vec<String> {
        let mut commands = Vec::new();
        let mut in_shell_fence = false;
        let mut pending = String::new();

        for raw_line in text.lines() {
            let trimmed = raw_line.trim();
            if trimmed.starts_with("```") {
                in_shell_fence = matches!(trimmed, "```bash" | "```sh" | "```text");
                continue;
            }
            if !in_shell_fence {
                continue;
            }

            let Some(line) = command_part(raw_line) else {
                continue;
            };
            if pending.is_empty() && !line.trim_start().starts_with("afmail ") {
                continue;
            }

            let line = line.trim();
            if let Some(prefix) = line.strip_suffix('\\') {
                pending.push_str(prefix.trim_end());
                pending.push(' ');
                continue;
            }

            pending.push_str(line);
            if command_is_parseable_contract(&pending) {
                commands.push(normalize_placeholders(&pending));
            }
            pending.clear();
        }
        commands
    }

    fn inline_commands(text: &str) -> Vec<String> {
        let mut commands = Vec::new();
        for segment in text.split('`').skip(1).step_by(2) {
            let command = segment.trim();
            if command.starts_with("afmail ") && command_is_parseable_contract(command) {
                commands.push(normalize_placeholders(command));
            }
        }
        commands.sort();
        commands.dedup();
        commands
    }

    fn fenced_json_blocks(text: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut in_json_fence = false;
        let mut current = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "```json" {
                in_json_fence = true;
                current.clear();
                continue;
            }
            if in_json_fence && trimmed == "```" {
                in_json_fence = false;
                blocks.push(current.join("\n"));
                continue;
            }
            if in_json_fence {
                current.push(line);
            }
        }
        blocks
    }

    fn command_is_parseable_contract(command: &str) -> bool {
        let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
        !command.contains("...")
            && !command.contains('|')
            && !command.contains('[')
            && !command.contains(']')
            && !command.contains(" --help")
            && !command.starts_with("afmail --help")
            && normalized != "afmail case draft send"
            && normalized != "afmail case draft upload"
            && normalized != "afmail message attachment fetch"
            && normalized != "afmail case show"
            && normalized != "afmail case archive"
            && normalized != "afmail contact"
            && normalized != "afmail message show"
            && normalized != "afmail notification show"
    }

    fn normalize_placeholders(command: &str) -> String {
        command
            .replace("PLAN_ID", "push_plan_placeholder")
            .replace("purge_plan_...", "purge_plan_placeholder")
            .replace("<REVIEWED_PLAN_ID>", "reviewed_plan_id")
            .replace("MESSAGE_ID", "message_inbox_1")
            .replace("NOTIFICATION_REF", "n20260711001")
            .replace("DRAFT_NAME", "reply.md")
            .replace("CASE_REF", "c20260711001")
            .replace("CATEGORY", "support")
            .replace("GROUP", "support")
            .replace("TEXT", "reason")
            .replace("REF", "c20260711001")
            .replace("NAME", "Name")
            .replace("N", "10")
            .replace("PATH", "file.txt")
    }

    fn command_part(line: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = line.chars().peekable();
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while let Some(ch) = chars.next() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                out.push(ch);
                continue;
            }
            if let Some(active) = quote {
                if ch == active {
                    quote = None;
                }
                out.push(ch);
                continue;
            }
            if ch == '"' || ch == '\'' {
                quote = Some(ch);
                out.push(ch);
                continue;
            }
            if ch == '#' && chars.peek().is_some() {
                break;
            }
            out.push(ch);
        }
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn tokenize_command(command: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut token = String::new();
        let mut quote: Option<char> = None;
        let mut escaped = false;

        for ch in command.chars() {
            if escaped {
                token.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if let Some(active) = quote {
                if ch == active {
                    quote = None;
                } else {
                    token.push(ch);
                }
                continue;
            }
            if ch == '"' || ch == '\'' {
                quote = Some(ch);
                continue;
            }
            if ch.is_whitespace() {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                continue;
            }
            token.push(ch);
        }
        if !token.is_empty() {
            tokens.push(token);
        }
        tokens
    }
}
