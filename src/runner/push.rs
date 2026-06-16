use crate::error::{AppError, Result};

pub(super) fn push_confirmed(dry_run: bool, confirm: bool) -> Result<bool> {
    if dry_run && confirm {
        return Err(AppError::new(
            "invalid_request",
            "--dry-run cannot be used with --confirm",
        ));
    }
    Ok(confirm)
}
