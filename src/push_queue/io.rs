use super::*;

pub(super) fn find_outbound_item(
    root: &Path,
    case_uid: &str,
    draft_name: &str,
) -> Result<Option<PushItem>> {
    for item in read_items(root)? {
        if item.outbound().is_some_and(|outbound| {
            outbound.case_uid == case_uid && outbound.draft_name == draft_name
        }) {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

pub(super) fn read_items(root: &Path) -> Result<Vec<PushItem>> {
    let dir = root.join(".afmail/push");
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
    items.sort_by(|a, b| a.created_rfc3339.cmp(&b.created_rfc3339));
    Ok(items)
}

pub(super) fn write_item(root: &Path, item: &PushItem) -> Result<()> {
    let dir = root.join(".afmail/push");
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
    if let Some(outbound) = item.outbound() {
        let path = safe_relative_path(root, &outbound.eml_path)?;
        if path.exists() {
            fs::remove_file(path).map_err(|e| AppError::io("remove push eml", &e))?;
        }
    }
    Ok(())
}

pub(super) fn push_path(root: &Path, push_id: &str) -> PathBuf {
    root.join(".afmail/push").join(format!("{push_id}.json"))
}

pub(super) fn read_item_eml(root: &Path, item: &PushItem) -> Result<Vec<u8>> {
    let outbound = item
        .outbound()
        .ok_or_else(|| AppError::new("invalid_request", "push item has no eml_path"))?;
    fs::read(safe_relative_path(root, &outbound.eml_path)?)
        .map_err(|e| AppError::io("read push eml", &e))
}

pub(super) fn find_case_path(root: &Path, case_uid: &str) -> Result<PathBuf> {
    if let Some(candidate) = find_case_path_any(root, case_uid)? {
        if candidate.starts_with(root.join("cases")) {
            return Ok(candidate);
        }
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
    let cases_dir = root.join("cases");
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
    let archive_dir = root.join("archive/cases");
    if archive_dir.is_dir() {
        for case in
            fs::read_dir(&archive_dir).map_err(|e| AppError::io("read archived cases", &e))?
        {
            let case = case.map_err(|e| AppError::io("read archived cases", &e))?;
            if case.path().is_dir() {
                out.push(case.path());
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

pub(super) fn unique_push_id(root: &Path) -> String {
    let base = format!(
        "push_{}",
        crate::store::now_rfc3339().replace([':', '-'], "")
    );
    let dir = root.join(".afmail/push");
    if !dir.join(format!("{base}.json")).exists() {
        return base;
    }
    for i in 1..1000 {
        let candidate = format!("{base}_{i}");
        if !dir.join(format!("{candidate}.json")).exists() {
            return candidate;
        }
    }
    base
}

pub(super) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| AppError::io("create directory", &e))
}

pub(super) fn safe_relative_path(root: &Path, value: &str) -> Result<PathBuf> {
    let rel = Path::new(value);
    let safe = !rel.is_absolute()
        && rel
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if safe {
        Ok(root.join(rel))
    } else {
        Err(AppError::new(
            "invalid_request",
            format!("unsafe push path: {value}"),
        ))
    }
}

pub(super) fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(path_to_string)
        .unwrap_or_else(|_| path_to_string(path))
}
