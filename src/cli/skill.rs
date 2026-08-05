#[derive(Debug, Clone)]
pub enum SkillAction {
    /// Show whether the Agent-First Mail skill is installed and valid.
    Status(SkillTargetArgs),
    /// Install the Agent-First Mail skill.
    Install(SkillWriteArgs),
    /// Remove an afmail-managed Agent-First Mail skill.
    Uninstall(SkillWriteArgs),
}

#[derive(Debug, Clone)]
pub struct SkillTargetArgs {
    /// Agent to manage. Defaults to all personal skill targets.
    pub agent: SkillAgentSelection,
    /// Skill scope. Workspace scope installs under the current workspace's skill directory.
    pub scope: SkillScope,
    /// Directory that contains skill folders. Requires an explicit single agent.
    pub skills_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SkillWriteArgs {
    pub target: SkillTargetArgs,
    /// Overwrite or remove an unmanaged Agent-First Mail skill at the target path.
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillAgentSelection {
    /// Manage every agent that supports the requested scope.
    All,
    /// Manage the Codex skill under $CODEX_HOME/skills or .codex/skills.
    Codex,
    /// Manage the Claude Code skill under ~/.claude/skills or .claude/skills.
    ClaudeCode,
    /// Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills.
    Opencode,
    /// Manage the Hermes skill under $HERMES_HOME/skills or ~/.hermes/skills.
    Hermes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillScope {
    /// Install under the user-level skills directory.
    Personal,
    /// Install under the current workspace's skills directory.
    Workspace,
}
