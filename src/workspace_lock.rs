use crate::error::{AppError, Result};
use fs4::{FileExt, TryLockError};
use std::fs::{File, OpenOptions};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Debug)]
pub(crate) struct WorkspaceLock {
    file: File,
}

impl WorkspaceLock {
    pub(crate) fn acquire(root: &Path, mode: LockMode) -> Result<Self> {
        match Self::try_acquire(root, mode)? {
            Some(lock) => Ok(lock),
            None => Err(workspace_locked_error()),
        }
    }

    pub(crate) fn try_acquire(root: &Path, mode: LockMode) -> Result<Option<Self>> {
        let dir = root.join(".afmail");
        std::fs::create_dir_all(&dir).map_err(|e| AppError::io("create lock directory", &e))?;
        let path = dir.join("workspace.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| AppError::io("open workspace lock", &e))?;
        let acquired = match mode {
            LockMode::Shared => FileExt::try_lock_shared(&file),
            LockMode::Exclusive => FileExt::try_lock(&file),
        };
        if matches!(acquired, Err(TryLockError::WouldBlock)) {
            return Ok(None);
        }
        acquired.map_err(|e| AppError::io("lock workspace", &std::io::Error::from(e)))?;
        Ok(Some(Self { file }))
    }
}

fn workspace_locked_error() -> AppError {
    AppError::retryable(
        "workspace_locked",
        "another afmail command is using this workspace",
    )
    .with_hint("Wait for the running afmail command to finish, then retry this command.")
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
