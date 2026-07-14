pub fn error_hint(argv: &[String]) -> String {
    let tokens = command_tokens(argv);
    if tokens.is_empty() {
        return "try: afmail --help".to_string();
    }
    match tokens[0].as_str() {
        "case" => nested_hint("afmail case", tokens.get(1), CASE_ACTIONS),
        "contact" => nested_hint("afmail contact", tokens.get(1), CONTACT_ACTIONS),
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
        "notification" => nested_hint("afmail notification", tokens.get(1), NOTIFICATION_ACTIONS),
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
    if let Some(action) = action
        && known_actions.contains(&action.as_str())
    {
        return format!("try: {prefix} {action} --help");
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
        if arg == "--output"
            || arg == "--log"
            || arg == "--stdout-file"
            || arg == "--stderr-file"
            || arg == "--workspace"
        {
            let _ = iter.next();
            continue;
        }
        if arg == "--verbose"
            || arg.starts_with("--output=")
            || arg.starts_with("--log=")
            || arg.starts_with("--stdout-file=")
            || arg.starts_with("--stderr-file=")
            || arg.starts_with("--workspace=")
        {
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
    "init",
    "pull",
    "config",
    "remote",
    "push",
    "status",
    "doctor",
    "purge",
    "skill",
    "triage",
    "message",
    "case",
    "contact",
    "notification",
    "render",
    "log",
    "ui",
];
#[cfg(not(feature = "ui"))]
const ROOT_COMMANDS: &[&str] = &[
    "init",
    "pull",
    "config",
    "remote",
    "push",
    "status",
    "doctor",
    "purge",
    "skill",
    "triage",
    "message",
    "case",
    "contact",
    "notification",
    "render",
    "log",
];
const CASE_ACTIONS: &[&str] = &[
    "create",
    "list",
    "show",
    "add",
    "remove",
    "move",
    "rename",
    "notes",
    "archive",
    "unarchive",
    "tag",
    "trash",
    "delete",
    "untag",
    "draft",
    "merge",
];
const CONTACT_ACTIONS: &[&str] = &[
    "create", "list", "show", "move", "rename", "email", "phone", "tag", "untag", "notes",
    "delete", "extract",
];
const MESSAGE_ACTIONS: &[&str] = &["show", "spam", "trash", "unspam", "untrash", "attachment"];
const MESSAGE_ATTACHMENT_ACTIONS: &[&str] = &["fetch"];
const NOTIFICATION_ACTIONS: &[&str] = &[
    "add",
    "create",
    "delete",
    "list",
    "show",
    "remove",
    "move",
    "rename",
    "set-summary",
    "notes",
];
const CONFIG_ACTIONS: &[&str] = &["show", "get", "set", "add", "remove"];
const REMOTE_ACTIONS: &[&str] = &["test", "folders"];
const PUSH_ACTIONS: &[&str] = &["list"];
const DOCTOR_ACTIONS: &[&str] = &["repair"];
const SKILL_ACTIONS: &[&str] = &["status", "install", "uninstall"];
const TRIAGE_ACTIONS: &[&str] = &["list"];
const RENDER_ACTIONS: &[&str] = &["refresh", "templates"];
const LOG_ACTIONS: &[&str] = &["list", "tail", "message", "case", "notification"];
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
                "notification",
                "rename",
            ])),
            "try: afmail notification rename --help"
        );
    }

    #[test]
    fn hints_track_renamed_and_added_commands() {
        // `contact` is a real root command; a typo'd subcommand must point at
        // its own help, not fall through to the generic top-level hint.
        assert_eq!(
            error_hint(&argv(&["contact", "zzz"])),
            "try: afmail contact --help"
        );
        assert_eq!(
            error_hint(&argv(&["contact", "rename", "--bad"])),
            "try: afmail contact rename --help"
        );
        // `log` and `notification` action tables follow the real subcommands:
        // log gained `notification` (was `archive`), notification gained `list`.
        assert_eq!(
            error_hint(&argv(&["log", "notification", "--bad"])),
            "try: afmail log notification --help"
        );
        assert_eq!(
            error_hint(&argv(&["notification", "list", "--bad"])),
            "try: afmail notification list --help"
        );
    }
}
