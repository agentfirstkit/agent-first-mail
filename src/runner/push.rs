use crate::error::{AppError, Result};

pub(super) fn push_plan_confirmation(
    dry_run: bool,
    confirm_plan_id: Option<&str>,
) -> Result<Option<&str>> {
    if dry_run && confirm_plan_id.is_some() {
        return Err(AppError::new(
            "invalid_request",
            "--dry-run cannot be used with --confirm",
        ));
    }
    Ok(confirm_plan_id)
}
