use crate::cli::PurgeAction;

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

/// The purge target now lives in the command path and `--older-than-days` is
/// declared once on each of those commands, so there is nothing left to
/// reconcile: the value is whatever the caller passed, or the default window.
pub(super) fn resolve_purge_action(
    action: Option<PurgeAction>,
    older_than_days: Option<u64>,
    confirm: Option<String>,
) -> ResolvedPurgeAction {
    let older_than_days = older_than_days.unwrap_or(30);
    match action {
        Some(PurgeAction::Spam) => ResolvedPurgeAction::Spam {
            older_than_days,
            confirm,
        },
        Some(PurgeAction::Trash) => ResolvedPurgeAction::Trash {
            older_than_days,
            confirm,
        },
        Some(PurgeAction::Deleted) => ResolvedPurgeAction::Deleted {
            older_than_days,
            confirm,
        },
        None => ResolvedPurgeAction::Discards {
            older_than_days,
            confirm,
        },
    }
}
