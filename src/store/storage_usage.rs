use super::util::read_dir_paths;
use crate::error::{AppError, Result};
use crate::layout;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub(super) fn workspace_storage_usage(root: &Path) -> Result<Value> {
    let raw_bytes =
        direct_child_file_bytes(&root.join(layout::afmail_path(layout::AFMAIL_MESSAGES)))?;
    let cache_bytes = recursive_file_bytes(&root.join(layout::MESSAGES))?
        + file_bytes(&root.join(layout::afmail_path(layout::AFMAIL_RENDER_CACHE)))?;
    let attachment_bytes = inbound_attachment_bytes(root)?
        + recursive_named_dir_bytes(&root.join(layout::CASES), "files")?
        + recursive_named_dir_bytes(&root.join(layout::ARCHIVED_CASES), "files")?;
    let view_bytes = generated_view_bytes(root)?;
    let audit_bytes =
        recursive_file_bytes(&root.join(crate::layout::afmail_path(crate::layout::AFMAIL_LOGS)))?;
    let queue_bytes =
        recursive_file_bytes(&root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH)))?
            + recursive_file_bytes(
                &root.join(crate::layout::afmail_path(crate::layout::AFMAIL_REQUESTS)),
            )?
            + recursive_file_bytes(&root.join(crate::layout::afmail_path(
                crate::layout::AFMAIL_TRANSACTIONS,
            )))?
            + file_bytes(&root.join(layout::afmail_path(layout::AFMAIL_PROGRESS)))?;
    let total_bytes =
        raw_bytes + cache_bytes + attachment_bytes + view_bytes + audit_bytes + queue_bytes;
    Ok(json!({
        "raw_bytes": raw_bytes,
        "cache_bytes": cache_bytes,
        "attachment_bytes": attachment_bytes,
        "view_bytes": view_bytes,
        "audit_bytes": audit_bytes,
        "queue_bytes": queue_bytes,
        "total_bytes": total_bytes,
    }))
}

fn generated_view_bytes(root: &Path) -> Result<u64> {
    let mut total = 0u64;
    for dir in [layout::TRIAGE, layout::SPAM, layout::TRASH, layout::DELETED] {
        total += direct_child_ext_bytes(&root.join(dir), "md")?;
    }
    total += recursive_named_file_bytes(&root.join(layout::CASES), "case.md")?;
    total += recursive_named_dir_bytes(&root.join(layout::CASES), "views")?;
    total += recursive_named_file_bytes(&root.join(layout::ARCHIVED_CASES), "case.md")?;
    total += recursive_named_dir_bytes(&root.join(layout::ARCHIVED_CASES), "views")?;
    total += recursive_named_file_bytes(&root.join(layout::NOTIFICATIONS), "notification.md")?;
    total += recursive_named_dir_bytes(&root.join(layout::NOTIFICATIONS), "views")?;
    Ok(total)
}

fn inbound_attachment_bytes(root: &Path) -> Result<u64> {
    let dir = root.join(".afmail/messages");
    if !dir.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for path in read_dir_paths(&dir, "read message attachment dirs")? {
        if path.is_dir()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".files"))
        {
            total += recursive_file_bytes(&path)?;
        }
    }
    Ok(total)
}

fn recursive_named_dir_bytes(root: &Path, dirname: &str) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for path in read_dir_paths(root, "read storage directories")? {
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(dirname) {
                total += recursive_file_bytes(&path)?;
            } else {
                total += recursive_named_dir_bytes(&path, dirname)?;
            }
        }
    }
    Ok(total)
}

fn recursive_named_file_bytes(root: &Path, filename: &str) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for path in read_dir_paths(root, "read storage files")? {
        if path.is_dir() {
            total += recursive_named_file_bytes(&path, filename)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some(filename) {
            total += file_bytes(&path)?;
        }
    }
    Ok(total)
}

fn direct_child_ext_bytes(path: &Path, ext: &str) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for path in read_dir_paths(path, "read storage files")? {
        if path.is_file() && path.extension().and_then(|value| value.to_str()) == Some(ext) {
            total += file_bytes(&path)?;
        }
    }
    Ok(total)
}

fn direct_child_file_bytes(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for path in read_dir_paths(path, "read storage files")? {
        if path.is_file() {
            total += file_bytes(&path)?;
        }
    }
    Ok(total)
}

fn recursive_file_bytes(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    if path.is_file() {
        return file_bytes(path);
    }
    let mut total = 0u64;
    for child in read_dir_paths(path, "read storage files")? {
        total += recursive_file_bytes(&child)?;
    }
    Ok(total)
}

fn file_bytes(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|e| AppError::io("read storage file metadata", &e))
}
