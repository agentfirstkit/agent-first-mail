use crate::cli::PurgeAction;
use crate::error::{AppError, Result};

pub(super) enum ResolvedPurgeAction {
    Spam {
        older_than_days: u64,
        confirm: Option<String>,
    },
    Trash {
        older_than_days: u64,
        confirm: Option<String>,
    },
    Deleted {
        older_than_days: u64,
        confirm: Option<String>,
    },
    Discards {
        older_than_days: u64,
        confirm: Option<String>,
    },
}

pub(super) fn resolve_purge_action(
    action: Option<PurgeAction>,
    root_older_than_days: Option<u64>,
    confirm: Option<String>,
) -> Result<ResolvedPurgeAction> {
    match action {
        Some(PurgeAction::Spam { older_than_days }) => Ok(ResolvedPurgeAction::Spam {
            older_than_days: resolve_purge_older_than_days(root_older_than_days, older_than_days)?,
            confirm,
        }),
        Some(PurgeAction::Trash { older_than_days }) => Ok(ResolvedPurgeAction::Trash {
            older_than_days: resolve_purge_older_than_days(root_older_than_days, older_than_days)?,
            confirm,
        }),
        Some(PurgeAction::Deleted { older_than_days }) => Ok(ResolvedPurgeAction::Deleted {
            older_than_days: resolve_purge_older_than_days(root_older_than_days, older_than_days)?,
            confirm,
        }),
        None => Ok(ResolvedPurgeAction::Discards {
            older_than_days: root_older_than_days.unwrap_or(30),
            confirm,
        }),
    }
}

fn resolve_purge_older_than_days(root: Option<u64>, child: Option<u64>) -> Result<u64> {
    match (root, child) {
        (Some(_), Some(_)) => Err(AppError::new(
            "invalid_request",
            "--older-than-days cannot be specified both before and after the purge target",
        )),
        (Some(days), None) | (None, Some(days)) => Ok(days),
        (None, None) => Ok(30),
    }
}
