use clap::{Args, Subcommand, ValueEnum};

#[derive(Subcommand, Debug, Clone)]
pub enum SkillAction {
    /// Show whether the Agent-First Mail skill is installed and valid.
    Status(SkillTargetArgs),
    /// Install the Agent-First Mail skill.
    Install(SkillWriteArgs),
    /// Remove an afmail-managed Agent-First Mail skill.
    Uninstall(SkillWriteArgs),
}

#[derive(Args, Debug, Clone)]
pub struct SkillTargetArgs {
    /// Agent to manage. Defaults to all personal skill targets.
    #[arg(long = "agent", value_enum, default_value_t = SkillAgentSelection::All)]
    pub agent: SkillAgentSelection,
    /// Skill scope. Project scope is supported for Claude Code and opencode, not Codex.
    #[arg(long = "scope", value_enum, default_value_t = SkillScope::Personal)]
    pub scope: SkillScope,
    /// Directory that contains skill folders. Requires an explicit single --agent.
    #[arg(long = "skills-dir")]
    pub skills_dir: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct SkillWriteArgs {
    #[command(flatten)]
    pub target: SkillTargetArgs,
    /// Overwrite or remove an unmanaged Agent-First Mail skill at the target path.
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SkillAgentSelection {
    /// Manage every agent that supports the requested scope.
    All,
    /// Manage the Codex local skill under $CODEX_HOME/skills.
    Codex,
    /// Manage the Claude Code skill under ~/.claude/skills or .claude/skills.
    #[value(name = "claude-code", alias = "claude")]
    ClaudeCode,
    /// Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills.
    Opencode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SkillScope {
    /// Install under the user-level skills directory.
    Personal,
    /// Install under the current project's skills directory.
    Project,
}
