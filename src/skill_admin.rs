//! `afmail skill` subcommand. Installs/uninstalls/reports status of the embedded
//! Agent Skill across Codex, Claude Code, and opencode via the shared
//! `agent_first_data::skill` admin — the same implementation every spore uses.

use crate::cli::{SkillAction, SkillAgentSelection, SkillScope, SkillTargetArgs};
use crate::error::{AppError, Result};
use agent_first_data::skill::{
    self, SkillAction as AfAction, SkillAgentSelection as AfSelection, SkillError, SkillOptions,
    SkillScope as AfScope, SkillSpec,
};
use serde_json::Value;

/// The embedded skill this binary installs.
const SPEC: SkillSpec = SkillSpec {
    name: "agent-first-mail",
    source: include_str!("../skills/agent-first-mail.md"),
    title: "Agent-First Mail",
    marker_slug: "afmail",
};

pub fn handle_action(action: SkillAction) -> Result<Value> {
    let (af_action, options) = split_action(action);
    let report = skill::run_skill_admin(&SPEC, af_action, &options).map_err(to_app_error)?;
    serde_json::to_value(&report).map_err(|e| {
        AppError::new(
            "internal_error",
            format!("failed to serialize skill report: {e}"),
        )
    })
}

fn split_action(action: SkillAction) -> (AfAction, SkillOptions) {
    match action {
        SkillAction::Status(target) => (AfAction::Status, options(target, false)),
        SkillAction::Install(write) => (AfAction::Install, options(write.target, write.force)),
        SkillAction::Uninstall(write) => (AfAction::Uninstall, options(write.target, write.force)),
    }
}

fn options(target: SkillTargetArgs, force: bool) -> SkillOptions {
    SkillOptions {
        agent: convert_agent(target.agent),
        scope: convert_scope(target.scope),
        skills_dir: target.skills_dir,
        force,
    }
}

fn convert_agent(agent: SkillAgentSelection) -> AfSelection {
    match agent {
        SkillAgentSelection::All => AfSelection::All,
        SkillAgentSelection::Codex => AfSelection::Codex,
        SkillAgentSelection::ClaudeCode => AfSelection::ClaudeCode,
        SkillAgentSelection::Opencode => AfSelection::Opencode,
    }
}

fn convert_scope(scope: SkillScope) -> AfScope {
    match scope {
        SkillScope::Personal => AfScope::Personal,
        SkillScope::Project => AfScope::Project,
    }
}

fn to_app_error(err: SkillError) -> AppError {
    let mut out = AppError::new("invalid_request", err.message);
    if let Some(hint) = err.hint {
        out = out.with_hint(hint);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::SkillWriteArgs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_skills_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "afmail_skill_{name}_{}_{}",
            std::process::id(),
            suffix
        ))
    }

    fn target_args(dir: &Path, agent: SkillAgentSelection) -> SkillTargetArgs {
        SkillTargetArgs {
            agent,
            scope: SkillScope::Personal,
            skills_dir: Some(dir.to_string_lossy().to_string()),
        }
    }

    fn write_args(dir: &Path, agent: SkillAgentSelection, force: bool) -> SkillWriteArgs {
        SkillWriteArgs {
            target: target_args(dir, agent),
            force,
        }
    }

    #[test]
    fn install_status_uninstall_opencode_skill() {
        let dir = temp_skills_dir("opencode");
        let install = handle_action(SkillAction::Install(write_args(
            &dir,
            SkillAgentSelection::Opencode,
            false,
        )));
        assert!(install.is_ok());
        let skill_path = dir.join("agent-first-mail").join("SKILL.md");
        assert!(skill_path.is_file());

        let status = handle_action(SkillAction::Status(target_args(
            &dir,
            SkillAgentSelection::Opencode,
        )));
        assert!(status.is_ok());
        if let Ok(value) = status {
            assert_eq!(value["code"], "skill_status");
            assert_eq!(value["skill"], "agent-first-mail");
            assert_eq!(value["installed_all"], true);
            assert_eq!(value["valid_all"], true);
            assert_eq!(value["current_all"], true);
            assert_eq!(value["targets"][0]["agent"], "opencode");
        }

        let removed = handle_action(SkillAction::Uninstall(write_args(
            &dir,
            SkillAgentSelection::Opencode,
            false,
        )));
        assert!(removed.is_ok());
        assert!(!skill_path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_unmanaged_skill_without_force() {
        let dir = temp_skills_dir("unmanaged");
        let skill_dir = dir.join("agent-first-mail");
        let skill_path = skill_dir.join("SKILL.md");
        assert!(std::fs::create_dir_all(&skill_dir).is_ok());
        assert!(
            std::fs::write(&skill_path, "---\nname: custom\ndescription: custom\n---\n").is_ok()
        );

        let install = handle_action(SkillAction::Install(write_args(
            &dir,
            SkillAgentSelection::Codex,
            false,
        )));
        assert!(install.is_err());
        assert!(skill_path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
