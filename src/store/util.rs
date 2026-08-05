use super::*;
use agent_first_slug::{EmptyOutputPolicy, SlugConfig};

/// Removes a directory on drop unless `commit()` was called first.
/// Use this to ensure newly-created workspace directories are cleaned up
/// if any subsequent operation in the same command fails.
pub(super) struct DirGuard {
    path: PathBuf,
    committed: bool,
}

impl DirGuard {
    pub(super) fn new(path: PathBuf) -> Self {
        DirGuard {
            path,
            committed: false,
        }
    }

    pub(super) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        if !self.committed && self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ManagedBlockChange {
    pub(super) created: bool,
    pub(super) updated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileWriteOutcome {
    Written,
    Unchanged,
}

pub(super) fn path_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub(super) fn audit_target(kind: &str, id: &str) -> Value {
    json!({"kind": kind, "id": id})
}

pub(super) fn event_targets_id(event: &Value, kind: &str, id: &str) -> bool {
    event
        .get("targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets.iter().any(|target| {
                target.get("kind").and_then(Value::as_str) == Some(kind)
                    && target.get("id").and_then(Value::as_str) == Some(id)
            })
        })
        .unwrap_or(false)
}

pub(super) fn take_last<T>(mut values: Vec<T>, limit: usize) -> Vec<T> {
    if values.len() <= limit {
        return values;
    }
    values.split_off(values.len() - limit)
}

pub(super) struct AuditLogRead {
    pub(super) events: Vec<Value>,
    pub(super) warnings: Vec<Value>,
}

pub(super) fn stable_text_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

pub(super) fn new_event_id() -> String {
    let now = Utc::now();
    let nanos = now.timestamp_nanos_opt().unwrap_or_default();
    format!(
        "event_{}_{}",
        now.format("%Y%m%dT%H%M%SZ"),
        nanos.unsigned_abs()
    )
}

pub(super) fn case_archived_error(case_uid: &str, group: Option<&str>) -> AppError {
    let unarchive_command = match group {
        Some(group) => format!("afmail case unarchive {case_uid} --group {group} --reason TEXT"),
        None => format!("afmail case unarchive {case_uid} --group GROUP --reason TEXT"),
    };
    AppError::new(
        "case_archived",
        format!("case {case_uid} is archived"),
    )
    .with_hint(format!(
        "Use `afmail case show {case_uid}` to inspect it, or `{unarchive_command}` before active-only edits."
    ))
    .with_details(json!({
        "case_uid": case_uid,
        "archived_group": group,
        "suggested_command": unarchive_command.as_str(),
        "suggested_commands": [
            format!("afmail case show {case_uid}"),
            unarchive_command
        ]
    }))
}

pub(super) fn write_json_if_missing(path: &Path, value: &Value) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    write_json_pretty(path, value)
}

pub(super) fn write_string_if_missing(path: &Path, value: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    write_string(path, value)
}

pub(super) fn write_string_if_changed(path: &Path, value: &str) -> Result<FileWriteOutcome> {
    if path.is_file() {
        match fs::read_to_string(path) {
            Ok(existing) if existing == value => return Ok(FileWriteOutcome::Unchanged),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {}
            Err(err) => return Err(AppError::io("read existing file", &err)),
        }
    }
    write_string(path, value)?;
    Ok(FileWriteOutcome::Written)
}

pub(super) fn write_json_pretty_if_changed<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<FileWriteOutcome> {
    let data = serde_json::to_string_pretty(value)
        .map_err(|e| AppError::json("serialize json", &e))?
        + "\n";
    write_string_if_changed(path, &data)
}

pub(super) fn ensure_managed_block_file(
    path: &Path,
    begin_marker: &str,
    end_marker: &str,
    default_prefix: &str,
    block_body: &str,
) -> Result<ManagedBlockChange> {
    let block = render_managed_block(begin_marker, end_marker, block_body);
    if !path.exists() {
        write_string(path, &compose_managed_block_file(default_prefix, &block))?;
        return Ok(ManagedBlockChange {
            created: true,
            updated: false,
        });
    }

    let existing = read_to_string(path, "read managed block file")?;
    let updated = if let Some(begin_pos) = existing.find(begin_marker) {
        let after_begin = begin_pos + begin_marker.len();
        let end_rel = existing[after_begin..].find(end_marker).ok_or_else(|| {
            AppError::new(
                "managed_block_invalid",
                format!(
                    "{} has an afmail managed BEGIN marker without a matching END marker",
                    path_to_string(path)
                ),
            )
        })?;
        let end_pos = after_begin + end_rel;
        let after_end = end_pos + end_marker.len();
        let replace_end = consume_one_line_ending(&existing, after_end);
        format!(
            "{}{}{}",
            &existing[..begin_pos],
            block,
            &existing[replace_end..]
        )
    } else {
        append_managed_block(&existing, &block)
    };

    if updated == existing {
        return Ok(ManagedBlockChange {
            created: false,
            updated: false,
        });
    }
    write_string(path, &updated)?;
    Ok(ManagedBlockChange {
        created: false,
        updated: true,
    })
}

pub(super) fn compose_managed_block_file(default_prefix: &str, block: &str) -> String {
    let mut out = default_prefix.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    out.push_str(block);
    out
}

pub(super) fn append_managed_block(existing: &str, block: &str) -> String {
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    out.push_str(block);
    out
}

pub(super) fn render_managed_block(
    begin_marker: &str,
    end_marker: &str,
    block_body: &str,
) -> String {
    let body = trim_surrounding_line_endings(block_body);
    let mut out = String::new();
    out.push_str(begin_marker);
    out.push('\n');
    out.push_str(body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(end_marker);
    out.push('\n');
    out
}

pub(super) fn trim_surrounding_line_endings(mut value: &str) -> &str {
    loop {
        if let Some(rest) = value.strip_prefix("\r\n") {
            value = rest;
        } else if let Some(rest) = value.strip_prefix('\n') {
            value = rest;
        } else {
            break;
        }
    }
    loop {
        if let Some(rest) = value.strip_suffix("\r\n") {
            value = rest;
        } else if let Some(rest) = value.strip_suffix('\n') {
            value = rest;
        } else {
            break;
        }
    }
    value
}

pub(super) fn consume_one_line_ending(value: &str, index: usize) -> usize {
    let suffix = &value[index..];
    if suffix.starts_with("\r\n") {
        index + 2
    } else if suffix.starts_with('\n') {
        index + 1
    } else {
        index
    }
}

pub(super) fn read_to_string(path: &Path, context: &str) -> Result<String> {
    fs::read_to_string(path).map_err(|e| AppError::io(context, &e))
}

/// Read a file that a caller is about to overwrite, distinguishing "not there
/// yet" from a real read failure — an edit-in-place path needs the current
/// bytes, and a create path must not be turned into an error by their absence.
pub(super) fn read_optional_to_string(path: &Path, context: &str) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::io(context, &err)),
    }
}

pub(super) fn read_existing_notes(root: &Path, path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(notes_missing_error(root, path))
        }
        Err(err) => Err(AppError::io("read notes", &err)),
    }
}

pub(super) fn notes_missing_error(root: &Path, path: &Path) -> AppError {
    AppError::new(
        "notes_missing",
        format!("notes.md is missing at {}", rel_path(root, path)),
    )
    .with_hint("Use the matching `notes replace REF --text TEXT` command to recreate notes.md.")
    .with_details(json!({
        "notes_path": rel_path(root, path),
        "suggested_command": "afmail case notes replace CASE_REF --text TEXT"
    }))
}

pub(super) fn write_string(path: &Path, data: &str) -> Result<()> {
    write_string_atomic(path, data)
}

pub(super) fn write_string_new(path: &Path, data: &str) -> Result<()> {
    if path.exists() {
        return Err(AppError::new(
            "store_error",
            format!("file already exists: {}", path_to_string(path)),
        ));
    }
    write_string(path, data)
}

pub(super) fn remove_file(path: &Path) -> Result<()> {
    fs::remove_file(path).map_err(|e| AppError::io("remove file", &e))
}

pub(super) fn remove_dir_all(path: &Path) -> Result<()> {
    fs::remove_dir_all(path).map_err(|e| AppError::io("remove directory", &e))
}

pub(super) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| AppError::io("create directory", &e))
}

pub(super) fn read_dir(path: &Path, context: &str) -> Result<Vec<fs::DirEntry>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| AppError::io(context, &e))? {
        out.push(entry.map_err(|e| AppError::io(context, &e))?);
    }
    Ok(out)
}

pub(super) fn count_files_with_ext(path: &Path, ext: &str) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(read_dir(path, "count files")?
        .into_iter()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some(ext))
        .count())
}

pub(super) fn ensure_no_name_conflicts(dest: &Path, source: &Path, label: &str) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for entry in read_dir(source, "read merge source")? {
        let name = entry.file_name();
        if dest.join(&name).exists() {
            return Err(AppError::new(
                "merge_conflict",
                format!("{label} name conflict: {}", name.to_string_lossy()),
            ));
        }
    }
    Ok(())
}

pub(super) fn move_children(source: &Path, dest: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    create_dir_all(dest)?;
    for entry in read_dir(source, "read children")? {
        let target = dest.join(entry.file_name());
        fs::rename(entry.path(), target).map_err(|e| AppError::io("move child", &e))?;
    }
    Ok(())
}

pub(super) fn unique_dest_path(dest_dir: &Path, filename: &str) -> PathBuf {
    let safe = filename;
    let candidate = dest_dir.join(safe);
    if !candidate.exists() {
        return candidate;
    }
    for i in 1..1000 {
        let next = dest_dir.join(format!("{i}-{safe}"));
        if !next.exists() {
            return next;
        }
    }
    dest_dir.join(format!("{}-{safe}", now_rfc3339().replace(':', "")))
}

pub(super) fn merge_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|item| item == value) {
        values.push(value.to_string());
        values.sort();
    }
}

pub(super) fn case_data_dir(case_path: &Path) -> PathBuf {
    case_path.join("data")
}

pub(super) fn case_json_path(case_path: &Path) -> PathBuf {
    case_data_dir(case_path).join("case.json")
}

pub(super) fn case_drafts_json_path(case_path: &Path) -> PathBuf {
    case_data_dir(case_path).join("drafts.json")
}

pub(super) fn case_views_messages_dir(case_path: &Path) -> PathBuf {
    case_path.join("views").join("messages")
}

pub(super) fn case_message_view_path(case_path: &Path, message_id: &str) -> PathBuf {
    case_views_messages_dir(case_path).join(format!("{message_id}.md"))
}

pub(super) fn read_case_file(case_path: &Path) -> Result<CaseFrontmatter> {
    let path = case_json_path(case_path);
    let data = read_to_string(&path, "read case metadata")?;
    let case: CaseFrontmatter =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse case metadata", &e))?;
    if case.schema_name != CASE_SCHEMA_NAME
        || case.schema_version != SUGGESTIBLE_COLLECTION_SCHEMA_VERSION
    {
        return Err(AppError::new(
            "case_metadata_invalid",
            format!(
                "invalid case metadata schema: {}",
                rel_path(case_path, &path)
            ),
        ));
    }
    validate_case_collection(&case, case_path)?;
    validate_case_membership(&case, case_path)?;
    validate_case_status(&case, case_path)?;
    super::suggestions::validate_case_suggestion_pair(
        case.agent_suggestion.as_ref(),
        case.human_suggestion.as_ref(),
    )
    .map_err(|error| {
        AppError::new(
            "case_metadata_invalid",
            format!(
                "invalid suggestion metadata in {}: {error}",
                rel_path(case_path, &path)
            ),
        )
    })?;
    Ok(case)
}

pub(super) fn write_case_file(case_path: &Path, case: &CaseFrontmatter) -> Result<()> {
    if case.schema_name != CASE_SCHEMA_NAME
        || case.schema_version != SUGGESTIBLE_COLLECTION_SCHEMA_VERSION
    {
        return Err(AppError::new(
            "case_metadata_invalid",
            format!(
                "invalid case metadata schema: {}",
                rel_path(case_path, &case_json_path(case_path))
            ),
        ));
    }
    validate_case_collection(case, case_path)?;
    validate_case_membership(case, case_path)?;
    validate_case_status(case, case_path)?;
    super::suggestions::validate_case_suggestion_pair(
        case.agent_suggestion.as_ref(),
        case.human_suggestion.as_ref(),
    )?;
    write_json_pretty(&case_json_path(case_path), case)
}

pub(super) fn validate_case_collection(case: &CaseFrontmatter, case_path: &Path) -> Result<()> {
    let expected_uid = case_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| parse_case_ref(name).ok());
    let content_issues = message_collection_content_issues(case);
    if validate_case_uid(&case.collection_uid).is_ok()
        && expected_uid.as_deref() == Some(case.collection_uid.as_str())
        && content_issues.is_empty()
    {
        return Ok(());
    }
    Err(AppError::new(
        "case_metadata_invalid",
        format!(
            "invalid canonical case collection: {}",
            path_to_string(&case_json_path(case_path))
        ),
    )
    .with_details(json!({
        "case_path": path_to_string(case_path),
        "expected_case_uid": expected_uid,
        "case_uid": case.collection_uid,
        "content_issues": content_issues,
    })))
}

pub(super) fn validate_case_membership(case: &CaseFrontmatter, case_path: &Path) -> Result<()> {
    let Some(message_id) = case.duplicate_message_id() else {
        return Ok(());
    };
    Err(AppError::new(
        "case_metadata_invalid",
        format!(
            "case {} contains duplicate canonical membership for {message_id}",
            case.collection_uid
        ),
    )
    .with_details(json!({
        "case_uid": case.collection_uid,
        "message_id": message_id,
        "case_path": path_to_string(case_path),
    })))
}

pub(super) fn validate_case_status(case: &CaseFrontmatter, case_path: &Path) -> Result<()> {
    let valid = match case.status.as_str() {
        "" | "archiving" => case.archived_rfc3339.is_none(),
        "archived" => case.archived_rfc3339.is_some(),
        _ => false,
    };
    if valid {
        return Ok(());
    }
    Err(AppError::new(
        "case_metadata_invalid",
        format!(
            "case {} has inconsistent canonical status {:?} and archived_rfc3339",
            case.collection_uid, case.status
        ),
    )
    .with_details(json!({
        "case_uid": case.collection_uid,
        "status": case.status,
        "archived_rfc3339": case.archived_rfc3339,
        "case_path": path_to_string(case_path),
    })))
}

pub(super) fn merge_flags(existing: &mut Vec<String>, flags: &[String]) -> bool {
    let mut merged = existing.clone();
    merged.extend(flags.iter().cloned());
    merged = canonical_flags(merged);
    if *existing == merged {
        return false;
    }
    *existing = merged;
    true
}

pub(super) fn remove_flags(existing: &mut Vec<String>, flags: &[String]) -> bool {
    let remove = flags
        .iter()
        .map(|flag| flag.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let updated = existing
        .iter()
        .filter(|flag| !remove.contains(&flag.to_ascii_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    if *existing == updated {
        return false;
    }
    *existing = updated;
    true
}

pub(super) fn json_contains_any_id(value: &Value, ids: &BTreeSet<String>) -> bool {
    match value {
        Value::String(text) => ids.contains(text),
        Value::Array(items) => items.iter().any(|item| json_contains_any_id(item, ids)),
        Value::Object(map) => map.values().any(|item| json_contains_any_id(item, ids)),
        _ => false,
    }
}

pub(super) fn email_address(value: &str) -> String {
    let trimmed = value.trim();
    if let (Some(start), Some(end)) = (trimmed.rfind('<'), trimmed.rfind('>'))
        && start < end
    {
        return trimmed[start + 1..end].trim().to_ascii_lowercase();
    }
    trimmed.to_ascii_lowercase()
}

pub(super) fn workspace_local_date(offset: &FixedOffset) -> String {
    Utc::now()
        .with_timezone(offset)
        .format("%Y%m%d")
        .to_string()
}

pub(super) fn next_uid_for_date(
    prefix: char,
    date: &str,
    existing: impl Iterator<Item = String>,
) -> Result<String> {
    if date.len() != 8 || !date.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid UID date: {date}"),
        ));
    }
    let mut max_seq = 0u64;
    for uid in existing {
        let valid = match prefix {
            'c' => validate_case_uid(&uid).is_ok(),
            'n' => validate_notification_uid(&uid).is_ok(),
            'p' => validate_contact_uid(&uid).is_ok(),
            _ => false,
        };
        if !valid || uid.get(1..9) != Some(date) {
            continue;
        }
        let seq = uid
            .get(9..)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        max_seq = max_seq.max(seq);
    }
    Ok(format!("{prefix}{date}{:03}", max_seq + 1))
}

pub(super) fn case_dir_name(case_uid: &str, name: &str) -> String {
    format!("{case_uid}-{}", human_slug(name))
}

pub(super) fn notification_dir_name(notification_uid: &str, name: &str) -> String {
    format!("{notification_uid}-{}", human_slug(name))
}

pub(super) fn human_slug(value: &str) -> String {
    let config = SlugConfig {
        empty_output_policy: EmptyOutputPolicy::UseFallbackSlug("untitled".to_string()),
        ..SlugConfig::default()
    };
    agent_first_slug::slugify(value, &config)
        .map(|result| result.slug)
        .unwrap_or_else(|_| "untitled".to_string())
}

pub(super) fn validate_contact_uid(value: &str) -> Result<()> {
    validate_uid("contact_uid", value, 'p')
}

pub(super) fn parse_contact_ref(value: &str) -> Result<String> {
    parse_uid_ref(value, 'p', "contact_ref").and_then(|uid| {
        validate_contact_uid(&uid)?;
        Ok(uid)
    })
}

pub(super) fn contact_uid_from_file_name(name: &str) -> Option<String> {
    name.split('-')
        .next()
        .filter(|uid| validate_contact_uid(uid).is_ok())
        .map(ToString::to_string)
}

pub(super) fn contact_file_name(uid: &str, display_name: &str) -> String {
    format!("{uid}-{}", human_slug(display_name))
}

pub(super) fn extract_from_display_name(from_header: &str) -> String {
    let trimmed = from_header.trim();
    if let (Some(start), Some(_end)) = (trimmed.rfind('<'), trimmed.rfind('>')) {
        let name = trimmed[..start].trim().trim_matches('"').trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    trimmed.to_string()
}

pub(super) fn parse_case_ref(value: &str) -> Result<String> {
    parse_uid_ref(value, 'c', "case_ref").and_then(|uid| {
        validate_case_uid(&uid)?;
        Ok(uid)
    })
}

pub(super) fn parse_notification_ref(value: &str) -> Result<String> {
    parse_uid_ref(value, 'n', "notification_ref").and_then(|uid| {
        validate_notification_uid(&uid)?;
        Ok(uid)
    })
}

pub(super) fn parse_uid_ref(value: &str, prefix: char, label: &str) -> Result<String> {
    validate_id(label, value)?;
    let Some(raw_uid) = value.split('-').next() else {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: {value}"),
        ));
    };
    if value.contains('-')
        && value
            .split_once('-')
            .is_some_and(|(_, suffix)| suffix.is_empty())
    {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: suffix must not be empty"),
        ));
    }
    if !raw_uid.starts_with(prefix) {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: expected {prefix}YYYYMMDDNNN"),
        ));
    }
    Ok(raw_uid.to_string())
}

pub(super) fn validate_case_uid(value: &str) -> Result<()> {
    validate_uid("case_uid", value, 'c')
}

pub(super) fn validate_notification_uid(value: &str) -> Result<()> {
    validate_uid("notification_uid", value, 'n')
}

pub(super) fn validate_uid(label: &str, value: &str, prefix: char) -> Result<()> {
    let bytes = value.as_bytes();
    let valid =
        bytes.len() >= 12 && bytes[0] == prefix as u8 && bytes[1..].iter().all(u8::is_ascii_digit);
    if !valid {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: expected {prefix}YYYYMMDDNNN"),
        ));
    }
    Ok(())
}

pub(super) fn message_collection_content_issues(collection: &MessageCollection) -> Vec<String> {
    let mut issues = Vec::new();
    if validate_name("collection_name", &collection.collection_name).is_err() {
        issues.push("collection_name is empty or path-unsafe".to_string());
    }
    for (field, value) in [
        ("created_rfc3339", collection.created_rfc3339.as_deref()),
        ("updated_rfc3339", collection.updated_rfc3339.as_deref()),
    ] {
        if value.is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err()) {
            issues.push(format!("{field} is not RFC 3339"));
        }
    }
    if collection
        .archived_rfc3339
        .as_deref()
        .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
    {
        issues.push("archived_rfc3339 is not RFC 3339".to_string());
    }
    let mut tags = BTreeSet::new();
    for tag in &collection.tags {
        if validate_id("tag", tag).is_err() {
            issues.push(format!("invalid tag: {tag}"));
        } else if !tags.insert(tag.as_str()) {
            issues.push(format!("duplicate tag: {tag}"));
        }
    }
    for item in &collection.items {
        if validate_id("message_id", &item.message_id).is_err() {
            issues.push(format!("invalid message_id: {}", item.message_id));
        }
        if DateTime::parse_from_rfc3339(&item.added_rfc3339).is_err() {
            issues.push(format!(
                "invalid added_rfc3339 for message {}",
                item.message_id
            ));
        }
        if item
            .summary
            .as_deref()
            .is_some_and(|summary| summary.is_empty() || summary.trim() != summary)
        {
            issues.push(format!(
                "non-canonical summary for message {}",
                item.message_id
            ));
        }
    }
    issues
}

pub(super) fn notification_uid_from_dir_name(value: &str) -> Option<String> {
    value
        .split('-')
        .next()
        .filter(|uid| validate_notification_uid(uid).is_ok())
        .map(ToString::to_string)
}

pub(super) fn validate_name(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: must not be empty"),
        ));
    }
    validate_id(label, &human_slug(value))
}

pub(super) fn validate_id(label: &str, value: &str) -> Result<()> {
    crate::slug::validate_path_segment(label, value)
}

pub(super) fn validate_file_name(label: &str, value: &str) -> Result<()> {
    validate_id(label, value)?;
    if !value.ends_with(".md") {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: must be a markdown file name"),
        ));
    }
    Ok(())
}

pub(super) fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(path_to_string)
        .unwrap_or_else(|_| path_to_string(path))
}

/// Parse `key=value` CLI tokens into ordered field pairs for `config add`.
pub(super) fn parse_field_assignments(fields: &[String]) -> Result<Vec<(String, String)>> {
    fields
        .iter()
        .map(|field| {
            field
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .ok_or_else(|| {
                    AppError::new(
                        "invalid_request",
                        format!("config add fields must be key=value, got: {field}"),
                    )
                })
        })
        .collect()
}

/// Mask a single config value read out from under its key.
///
/// The emitter's redactor walks *field names*, and these land under the payload
/// key `value`, which tells it nothing — so this is the one place afmail must
/// make the secret decision itself. A null stays null: an unset secret is not a
/// configured one, and reporting it as `"***"` would send a caller looking for
/// a credential that was never set.
pub(super) fn config_value_for_output(key: &str, value: Value) -> Value {
    if config_key_is_secret(key) && !value.is_null() {
        json!("***")
    } else {
        value
    }
}

/// Whether a dot-path config key addresses a secret.
///
/// Splitting the path is afmail's business; deciding what a secret *name* looks
/// like is not. Deferring the suffix rule to afdata keeps this from drifting
/// away from the convention every other redaction path in the process follows.
pub(super) fn config_key_is_secret(key: &str) -> bool {
    key.rsplit('.')
        .next()
        .is_some_and(|leaf| agent_first_data::Redactor::new().is_secret_name(leaf))
}

pub(super) fn config_write_error(err: agent_first_data::document::DocumentError) -> AppError {
    AppError::new("config_invalid", format!("write config: {err}"))
}

fn trim_ascii_whitespace(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &value[start..end]
}

fn parse_audit_line(line: &[u8], line_number: usize) -> Result<Value> {
    let text = std::str::from_utf8(line).map_err(|e| {
        AppError::new(
            "store_error",
            format!("parse audit event line {line_number}: invalid UTF-8: {e}"),
        )
    })?;
    serde_json::from_str::<Value>(text).map_err(|e| {
        AppError::new(
            "store_error",
            format!("parse audit event line {line_number}: {e}"),
        )
    })
}

fn audit_log_warning(path: &Path, line_number: usize, err: &AppError) -> Value {
    json!({
        "code": "audit_log_warning",
        "warning_code": "audit_log_truncated_tail",
        "path": path_to_string(path),
        "line_number": line_number,
        "skipped_line_count": 1,
        "error_code": err.error_code,
        "error": err.message.as_str(),
    })
}

impl Workspace {
    pub(crate) fn append_audit_event(
        &self,
        kind: &str,
        targets: Vec<Value>,
        reason: Option<&str>,
        fields: Value,
    ) -> Result<()> {
        self.append_audit_event_record(
            &new_event_id(),
            &now_rfc3339(),
            kind,
            targets,
            reason,
            fields,
        )
    }

    pub(crate) fn append_audit_event_with_identity(
        &self,
        event_id: &str,
        created_rfc3339: &str,
        kind: &str,
        targets: Vec<Value>,
        reason: Option<&str>,
        fields: Value,
    ) -> Result<bool> {
        let existing = self.read_audit_log()?;
        if !existing.warnings.is_empty() {
            return Err(AppError::new(
                "audit_log_recovery_required",
                "audit log has a truncated tail; refusing to append after damaged bytes",
            )
            .with_hint("Inspect `afmail log tail` and repair the audit log before retrying the pending push audit.")
            .with_details(json!({
                "event_id": event_id,
                "warning_count": existing.warnings.len(),
                "warnings": existing.warnings,
            })));
        }
        if existing
            .events
            .iter()
            .any(|event| event.get("event_id").and_then(Value::as_str) == Some(event_id))
        {
            return Ok(false);
        }
        self.append_audit_event_record(event_id, created_rfc3339, kind, targets, reason, fields)?;
        if std::env::var("AFMAIL_TEST_FAIL_PUSH_AUDIT_AFTER_APPEND")
            .is_ok_and(|value| value == kind)
        {
            return Err(AppError::new(
                "push_audit_after_append_test_failure",
                format!("test failure after appending {kind} audit event"),
            ));
        }
        Ok(true)
    }

    fn append_audit_event_record(
        &self,
        event_id: &str,
        created_rfc3339: &str,
        kind: &str,
        targets: Vec<Value>,
        reason: Option<&str>,
        fields: Value,
    ) -> Result<()> {
        let mut obj = match fields {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        obj.insert("code".to_string(), json!("afmail_event"));
        obj.insert("event_id".to_string(), json!(event_id));
        obj.insert("created_rfc3339".to_string(), json!(created_rfc3339));
        obj.insert("actor".to_string(), json!("cli"));
        obj.insert("kind".to_string(), json!(kind));
        obj.insert("targets".to_string(), Value::Array(targets));
        if let Some(reason) = reason {
            obj.insert("reason".to_string(), json!(reason));
        }
        let line = serde_json::to_string(&Value::Object(obj))
            .map_err(|e| AppError::json("serialize audit event", &e))?;
        let log_path = self
            .root
            .join(crate::layout::afmail_path(crate::layout::AFMAIL_AUDIT_LOG));
        if let Some(parent) = log_path.parent() {
            create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| AppError::io("open audit log", &e))?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_data())
            .map_err(|e| AppError::io("append audit log", &e))
    }

    pub(super) fn read_audit_log(&self) -> Result<AuditLogRead> {
        self.require_workspace()?;
        let path = self
            .root
            .join(crate::layout::afmail_path(crate::layout::AFMAIL_AUDIT_LOG));
        if !path.exists() {
            return Ok(AuditLogRead {
                events: Vec::new(),
                warnings: Vec::new(),
            });
        }
        let data = fs::read(&path).map_err(|e| AppError::io("read audit log", &e))?;
        let lines = data.split(|byte| *byte == b'\n').collect::<Vec<_>>();
        let last_non_empty_index = lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, line)| !trim_ascii_whitespace(line).is_empty())
            .map(|(index, _)| index);
        let mut events = Vec::new();
        let mut warnings = Vec::new();
        for (index, raw_line) in lines.iter().enumerate() {
            let line = trim_ascii_whitespace(raw_line);
            if line.is_empty() {
                continue;
            }
            match parse_audit_line(line, index + 1) {
                Ok(event) => events.push(event),
                Err(err) if Some(index) == last_non_empty_index => {
                    warnings.push(audit_log_warning(&path, index + 1, &err));
                }
                Err(err) => return Err(err),
            }
        }
        Ok(AuditLogRead { events, warnings })
    }

    pub(super) fn checked_reason<'a>(&self, reason: Option<&'a str>) -> Result<Option<&'a str>> {
        let reason = reason.map(str::trim).filter(|value| !value.is_empty());
        let config = MailConfig::load(&self.root)?;
        match (config.audit.reason_mode, reason) {
            (ReasonMode::Required, None) => Err(AppError::new(
                "reason_required",
                "add --reason \"<why>\" to record why this mutation is needed",
            )
            .with_hint("Repeat the command with `--reason TEXT`.")
            .with_details(json!({
                "required_option": "--reason",
                "suggested_args": ["--reason", "TEXT"]
            }))),
            (_, reason) => Ok(reason),
        }
    }

    pub(super) fn template_language(&self) -> Result<TemplateLanguage> {
        Ok(MailConfig::load(&self.root)?.template_language())
    }

    pub(super) fn workspace_date_offset(&self) -> Result<FixedOffset> {
        Ok(MailConfig::load(&self.root)?.resolved_timezone_offset())
    }

    pub(crate) fn require_workspace(&self) -> Result<()> {
        let afmail = self.root.join(layout::AFMAIL);
        if !afmail.is_dir() {
            return Err(AppError::new(
                "workspace_not_found",
                "no .afmail directory found at workspace root",
            ));
        }
        crate::workspace_identity::load_workspace_identity(&self.root)?;
        Ok(())
    }

    pub(super) fn first_related_message_date(&self, message_id: &str) -> Result<String> {
        let mut message_ids = self.related_message_ids(message_id)?;
        message_ids.push(message_id.to_string());
        let mut times = Vec::new();
        for id in message_ids {
            let message = self.read_message_by_id(&id)?;
            if let Some(time) = message_time(&message)
                && let Ok(parsed) = DateTime::parse_from_rfc3339(&time)
            {
                times.push(parsed);
            }
        }
        times.sort();
        let offset = self.workspace_date_offset()?;
        Ok(times
            .first()
            .map(|time| time.with_timezone(&offset).format("%Y%m%d").to_string())
            .unwrap_or_else(|| workspace_local_date(&offset)))
    }

    pub(super) fn next_case_uid(&self, date: &str) -> Result<String> {
        next_uid_for_date(
            'c',
            date,
            self.all_case_entries()?
                .into_iter()
                .map(|(case_uid, _)| case_uid),
        )
    }

    pub(super) fn next_notification_uid(&self, date: &str) -> Result<String> {
        next_uid_for_date('n', date, self.notification_ids()?.into_iter())
    }

    pub(super) fn message_path(&self, message_id: &str) -> PathBuf {
        self.root
            .join(layout::MESSAGES)
            .join(format!("{message_id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::human_slug;

    #[test]
    fn human_slug_keeps_unicode_letters_and_digits() {
        assert_eq!(
            human_slug("現在的Nobody，未來的Somebody！"),
            "現在的nobody-未來的somebody"
        );
        assert_eq!(
            human_slug("牙好，胃口就好，身体倍儿棒，吃嘛嘛香。"),
            "牙好-胃口就好-身体倍儿棒-吃嘛嘛香"
        );
        assert_eq!(human_slug("お元気ですか？"), "お元気ですか");
    }

    #[test]
    fn human_slug_collapses_path_unsafe_characters() {
        assert_eq!(human_slug("Ubuntu 16.04"), "ubuntu-16-04");
        assert_eq!(human_slug(" A:B/C\\D  😀  "), "a-b-c-d");
        assert_eq!(human_slug("!!!"), "untitled");
    }
}
