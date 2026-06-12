use crate::cli::PushAction;
use crate::error::{AppError, Result};

pub(super) fn push_confirmed(
    root_dry_run: bool,
    root_confirm: bool,
    child_dry_run: bool,
    child_confirm: bool,
) -> Result<bool> {
    let dry_run = root_dry_run || child_dry_run;
    let confirm = root_confirm || child_confirm;
    if dry_run && confirm {
        return Err(AppError::new(
            "invalid_request",
            "--dry-run cannot be used with --confirm",
        ));
    }
    Ok(confirm)
}

pub(super) fn push_has_confirm(root_confirm: bool, action: &Option<PushAction>) -> bool {
    root_confirm
        || matches!(
            action,
            Some(PushAction::Drafts { confirm: true, .. })
                | Some(PushAction::DraftsSend { confirm: true, .. })
                | Some(PushAction::Archive { confirm: true, .. })
                | Some(PushAction::Spam { confirm: true, .. })
                | Some(PushAction::Trash { confirm: true, .. })
        )
}

pub(super) fn push_has_dry_run(root_dry_run: bool, action: &Option<PushAction>) -> bool {
    root_dry_run
        || matches!(
            action,
            Some(PushAction::Drafts { dry_run: true, .. })
                | Some(PushAction::DraftsSend { dry_run: true, .. })
                | Some(PushAction::Archive { dry_run: true, .. })
                | Some(PushAction::Spam { dry_run: true, .. })
                | Some(PushAction::Trash { dry_run: true, .. })
        )
}
