pub fn error_hint(argv: &[String]) -> String {
    let tokens = command_tokens(argv);
    if tokens.is_empty() {
        return "try: afmail --help".to_string();
    }
    match tokens[0].as_str() {
        "case" => nested_hint("afmail case", tokens.get(1), CASE_ACTIONS),
        "message" => {
            if tokens.get(1).is_some_and(|token| token == "attachment") {
                nested_hint(
                    "afmail message attachment",
                    tokens.get(2),
                    MESSAGE_ATTACHMENT_ACTIONS,
                )
            } else {
                nested_hint("afmail message", tokens.get(1), MESSAGE_ACTIONS)
            }
        }
        "archive" => match tokens.get(1).map(String::as_str) {
            Some("message") => nested_hint(
                "afmail archive message",
                tokens.get(2),
                ARCHIVE_MESSAGE_ACTIONS,
            ),
            Some("case") => nested_hint("afmail archive case", tokens.get(2), ARCHIVE_CASE_ACTIONS),
            Some("list") => nested_hint("afmail archive list", tokens.get(2), ARCHIVE_LIST_ACTIONS),
            Some(_) => "try: afmail archive --help".to_string(),
            None => "try: afmail archive --help".to_string(),
        },
        "config" => nested_hint("afmail config", tokens.get(1), CONFIG_ACTIONS),
        "remote" => nested_hint("afmail remote", tokens.get(1), REMOTE_ACTIONS),
        "push" => nested_hint("afmail push", tokens.get(1), PUSH_ACTIONS),
        "doctor" => nested_hint("afmail doctor", tokens.get(1), DOCTOR_ACTIONS),
        "skill" => nested_hint("afmail skill", tokens.get(1), SKILL_ACTIONS),
        "triage" => nested_hint("afmail triage", tokens.get(1), TRIAGE_ACTIONS),
        "render" => nested_hint("afmail render", tokens.get(1), RENDER_ACTIONS),
        "log" => nested_hint("afmail log", tokens.get(1), LOG_ACTIONS),
        #[cfg(feature = "ui")]
        "ui" => nested_hint("afmail ui", tokens.get(1), UI_ACTIONS),
        command if ROOT_COMMANDS.contains(&command) => format!("try: afmail {command} --help"),
        _ => "try: afmail --help".to_string(),
    }
}

fn nested_hint(prefix: &str, action: Option<&String>, known_actions: &[&str]) -> String {
    if let Some(action) = action {
        if known_actions.contains(&action.as_str()) {
            return format!("try: {prefix} {action} --help");
        }
    }
    format!("try: {prefix} --help")
}

fn command_tokens(argv: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if arg == "--output" || arg == "--log" {
            let _ = iter.next();
            continue;
        }
        if arg == "--verbose" || arg.starts_with("--output=") || arg.starts_with("--log=") {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

#[cfg(feature = "ui")]
const ROOT_COMMANDS: &[&str] = &[
    "init", "pull", "config", "remote", "push", "status", "doctor", "purge", "skill", "triage",
    "message", "case", "archive", "render", "log", "ui",
];
#[cfg(not(feature = "ui"))]
const ROOT_COMMANDS: &[&str] = &[
    "init", "pull", "config", "remote", "push", "status", "doctor", "purge", "skill", "triage",
    "message", "case", "archive", "render", "log",
];
const CASE_ACTIONS: &[&str] = &[
    "create", "list", "show", "add", "move", "rename", "notes", "archive", "reopen", "tag",
    "trash", "untag", "draft", "merge",
];
const MESSAGE_ACTIONS: &[&str] = &["show", "spam", "trash", "restore", "attachment"];
const MESSAGE_ATTACHMENT_ACTIONS: &[&str] = &["fetch"];
const ARCHIVE_MESSAGE_ACTIONS: &[&str] = &[
    "add",
    "create",
    "show",
    "restore",
    "move",
    "rename",
    "set-summary",
    "notes",
];
const ARCHIVE_CASE_ACTIONS: &[&str] = &["show", "restore", "trash", "rename", "notes"];
const ARCHIVE_LIST_ACTIONS: &[&str] = &["cases", "messages"];
const CONFIG_ACTIONS: &[&str] = &["show", "get", "set"];
const REMOTE_ACTIONS: &[&str] = &["test", "folders"];
const PUSH_ACTIONS: &[&str] = &["list"];
const DOCTOR_ACTIONS: &[&str] = &["repair"];
const SKILL_ACTIONS: &[&str] = &["status", "install", "uninstall"];
const TRIAGE_ACTIONS: &[&str] = &["list"];
const RENDER_ACTIONS: &[&str] = &["refresh", "templates"];
const LOG_ACTIONS: &[&str] = &["list", "tail", "message", "case", "archive"];
#[cfg(feature = "ui")]
const UI_ACTIONS: &[&str] = &["snapshot"];

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("afmail".to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect()
    }

    #[test]
    fn hints_nested_command_help() {
        assert_eq!(
            error_hint(&argv(&["case", "draft", "--bad"])),
            "try: afmail case draft --help"
        );
        assert_eq!(
            error_hint(&argv(&["message", "attachment", "fetch", "--bad"])),
            "try: afmail message attachment fetch --help"
        );
    }

    #[test]
    fn skips_global_options_when_finding_command() {
        assert_eq!(
            error_hint(&argv(&[
                "--output",
                "yaml",
                "--log=startup",
                "archive",
                "message",
                "rename",
            ])),
            "try: afmail archive message rename --help"
        );
    }
}
