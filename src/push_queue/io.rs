use super::*;
use crate::layout;
use ring::rand::{SecureRandom, SystemRandom};

pub(super) fn find_draft_item(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Option<PushItem>> {
    for item in read_items(root)? {
        if item
            .draft()
            .is_some_and(|draft| draft.case_uid == case_uid && draft.draft_name == draft_name)
        {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

pub(super) fn find_message_item(root: &Path, message_id: &str) -> Result<Option<PushItem>> {
    for item in read_items(root)? {
        if item
            .message()
            .is_some_and(|message| message.message_id == message_id)
        {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

pub(super) fn read_items(root: &Path) -> Result<Vec<PushItem>> {
    let dir = root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| AppError::io("read push queue", &e))? {
        let entry = entry.map_err(|e| AppError::io("read push queue", &e))?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let data =
            fs::read_to_string(entry.path()).map_err(|e| AppError::io("read push item", &e))?;
        out.push(PushItem::parse_json(&data)?);
    }
    Ok(out)
}

pub(super) fn sorted_items(root: &Path) -> Result<Vec<PushItem>> {
    let mut items = read_items(root)?;
    items.sort_by(|a, b| {
        (a.enqueue_seq, a.push_id.as_str()).cmp(&(b.enqueue_seq, b.push_id.as_str()))
    });
    Ok(items)
}

/// Allocates the next enqueue sequence number for a new push item: max(enqueue_seq
/// over all items currently in the queue) + 1, starting at 1 for an empty queue.
/// Enqueueing requires the exclusive workspace lock (see workspace_lock.rs); this
/// max+1 allocation is race-free only because callers hold that lock while enqueuing.
pub(super) fn next_enqueue_seq(root: &Path) -> Result<u64> {
    let max = read_items(root)?
        .into_iter()
        .map(|item| item.enqueue_seq)
        .max()
        .unwrap_or(0);
    Ok(max + 1)
}

pub(super) fn write_item(root: &Path, item: &PushItem) -> Result<()> {
    let dir = root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH));
    create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(item)
        .map_err(|e| AppError::json("serialize push item", &e))?;
    write_string_atomic(&dir.join(format!("{}.json", item.push_id)), &(data + "\n"))
}

pub(super) fn delete_item(root: &Path, item: &PushItem) -> Result<()> {
    let json_path = push_path(root, &item.push_id);
    if json_path.exists() {
        fs::remove_file(json_path).map_err(|e| AppError::io("remove push json", &e))?;
    }
    Ok(())
}

pub(super) fn push_path(root: &Path, push_id: &str) -> PathBuf {
    root.join(crate::layout::afmail_path(crate::layout::AFMAIL_PUSH))
        .join(format!("{push_id}.json"))
}

pub(super) fn find_case_path(root: &Path, case_uid: &str) -> Result<PathBuf> {
    if let Some(candidate) = find_case_path_any(root, case_uid)?
        && (candidate.starts_with(root.join(layout::CASES))
            || candidate.starts_with(root.join(layout::ARCHIVED_CASES)))
    {
        return Ok(candidate);
    }
    Err(AppError::new(
        "case_not_found",
        format!("case not found: {case_uid}"),
    ))
}

pub(super) fn find_case_path_any(root: &Path, case_uid: &str) -> Result<Option<PathBuf>> {
    for dir in case_search_roots(root)? {
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if dir_case_uid(name) == Some(case_uid) && dir.join("data/case.json").is_file() {
            return Ok(Some(dir));
        }
    }
    Ok(None)
}

pub(super) fn case_search_roots(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let cases_dir = root.join(layout::CASES);
    if cases_dir.is_dir() {
        for group in fs::read_dir(&cases_dir).map_err(|e| AppError::io("read cases", &e))? {
            let group = group.map_err(|e| AppError::io("read cases", &e))?;
            if group.path().is_dir() {
                for case in
                    fs::read_dir(group.path()).map_err(|e| AppError::io("read cases", &e))?
                {
                    let case = case.map_err(|e| AppError::io("read cases", &e))?;
                    if case.path().is_dir() {
                        out.push(case.path());
                    }
                }
            }
        }
    }
    let archive_dir = root.join(layout::ARCHIVED_CASES);
    if archive_dir.is_dir() {
        for group in
            fs::read_dir(&archive_dir).map_err(|e| AppError::io("read archived cases", &e))?
        {
            let group = group.map_err(|e| AppError::io("read archived cases", &e))?;
            if group.path().is_dir() {
                for case in fs::read_dir(group.path())
                    .map_err(|e| AppError::io("read archived cases", &e))?
                {
                    let case = case.map_err(|e| AppError::io("read archived cases", &e))?;
                    if case.path().is_dir() {
                        out.push(case.path());
                    }
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn dir_case_uid(name: &str) -> Option<&str> {
    let uid = name.split('-').next()?;
    let bytes = uid.as_bytes();
    if bytes.len() >= 10 && bytes[0] == b'c' && bytes[1..].iter().all(u8::is_ascii_digit) {
        Some(uid)
    } else {
        None
    }
}

pub(super) fn message_push_id(message_id: &str) -> String {
    format!("push_message_{message_id}")
}

pub(super) fn new_draft_push_id(root: &Path) -> Result<String> {
    for _ in 0..8 {
        let mut bytes = [0_u8; 16];
        SystemRandom::new().fill(&mut bytes).map_err(|_| {
            AppError::new(
                "push_id_generation_failed",
                "secure random generation failed for outbound push identity",
            )
        })?;
        let suffix = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let push_id = format!("push_draft_{suffix}");
        if !push_path(root, &push_id).exists() {
            return Ok(push_id);
        }
    }
    Err(AppError::new(
        "push_id_generation_failed",
        "could not allocate a unique outbound push identity",
    ))
}

pub(super) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| AppError::io("create directory", &e))
}

pub(super) fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(path_to_string)
        .unwrap_or_else(|_| path_to_string(path))
}
