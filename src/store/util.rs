use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ManagedBlockChange {
    pub(super) created: bool,
    pub(super) updated: bool,
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

pub(super) fn case_archived_error(case_uid: &str) -> AppError {
    AppError::new(
        "case_archived",
        format!("case {case_uid} is archived"),
    )
    .with_hint(format!(
        "Use `afmail archive case show {case_uid}` to inspect it, or `afmail archive case restore {case_uid} --group GROUP --reason TEXT` before editing."
    ))
    .with_details(json!({
        "case_uid": case_uid,
        "suggested_commands": [
            format!("afmail archive case show {case_uid}"),
            format!("afmail archive case restore {case_uid} --group GROUP --reason TEXT")
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

pub(super) fn managed_block_template_parts(
    rendered: &str,
    begin_marker: &str,
    end_marker: &str,
    path: &Path,
) -> Result<(String, String)> {
    let begin_pos = rendered.find(begin_marker).ok_or_else(|| {
        AppError::new(
            "managed_block_missing",
            format!(
                "{} template is missing the afmail managed BEGIN marker",
                path_to_string(path)
            ),
        )
    })?;
    let after_begin = begin_pos + begin_marker.len();
    let end_rel = rendered[after_begin..].find(end_marker).ok_or_else(|| {
        AppError::new(
            "managed_block_missing",
            format!(
                "{} template is missing the afmail managed END marker",
                path_to_string(path)
            ),
        )
    })?;
    let end_pos = after_begin + end_rel;
    Ok((
        rendered[..begin_pos].to_string(),
        trim_surrounding_line_endings(&rendered[after_begin..end_pos]).to_string(),
    ))
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

pub(super) fn case_messages_json_path(case_path: &Path) -> PathBuf {
    case_data_dir(case_path).join("messages.json")
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
    let mut case: CaseFrontmatter =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse case metadata", &e))?;
    if case.kind.is_empty() {
        case.kind = "case".to_string();
    }
    if case.kind != "case" {
        return Err(AppError::new(
            "case_metadata_invalid",
            format!("invalid case metadata kind: {}", rel_path(case_path, &path)),
        ));
    }
    Ok(case)
}

pub(super) fn write_case_file(case_path: &Path, case: &CaseFrontmatter) -> Result<()> {
    let mut normalized = case.clone();
    normalized.kind = "case".to_string();
    write_json_pretty(&case_json_path(case_path), &normalized)
}

pub(super) fn case_status(case_path: &Path) -> Result<String> {
    Ok(read_case_file(case_path)?.status)
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
    if let (Some(start), Some(end)) = (trimmed.rfind('<'), trimmed.rfind('>')) {
        if start < end {
            return trimmed[start + 1..end].trim().to_ascii_lowercase();
        }
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
            'a' => validate_archive_uid(&uid).is_ok(),
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

pub(super) fn archive_dir_name(archive_uid: &str, name: &str) -> String {
    format!("{archive_uid}-{}", human_slug(name))
}

pub(super) fn human_slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.trim().chars() {
        let keep = !ch.is_control() && !matches!(ch, '/' | '\\' | ':');
        if keep && !ch.is_whitespace() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug
    }
}

pub(super) fn parse_case_ref(value: &str) -> Result<String> {
    parse_uid_ref(value, 'c', "case_ref").and_then(|uid| {
        validate_case_uid(&uid)?;
        Ok(uid)
    })
}

pub(super) fn parse_archive_ref(value: &str) -> Result<String> {
    parse_uid_ref(value, 'a', "archive_ref").and_then(|uid| {
        validate_archive_uid(&uid)?;
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

pub(super) fn validate_archive_uid(value: &str) -> Result<()> {
    validate_uid("archive_uid", value, 'a')
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

pub(super) fn archive_uid_from_dir_name(value: &str) -> Option<String> {
    value
        .split('-')
        .next()
        .filter(|uid| validate_archive_uid(uid).is_ok())
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
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: must be a single path segment"),
        ));
    }
    Ok(())
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

pub(super) fn config_value_for_output(key: &str, value: Value) -> Value {
    if config_key_is_secret(key) && !value.is_null() {
        json!("***")
    } else {
        value
    }
}

pub(super) fn config_key_is_secret(key: &str) -> bool {
    key.rsplit('.')
        .next()
        .is_some_and(|leaf| leaf.ends_with("_secret"))
}

impl Workspace {
    pub(crate) fn append_audit_event(
        &self,
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
        obj.insert("event_id".to_string(), json!(new_event_id()));
        obj.insert("created_rfc3339".to_string(), json!(now_rfc3339()));
        obj.insert("actor".to_string(), json!("cli"));
        obj.insert("kind".to_string(), json!(kind));
        obj.insert("targets".to_string(), Value::Array(targets));
        if let Some(reason) = reason {
            obj.insert("reason".to_string(), json!(reason));
        }
        let line = serde_json::to_string(&Value::Object(obj))
            .map_err(|e| AppError::json("serialize audit event", &e))?;
        let log_path = self.root.join(".afmail/logs/events.jsonl");
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
            .map_err(|e| AppError::io("append audit log", &e))
    }

    pub(super) fn read_audit_events(&self) -> Result<Vec<Value>> {
        self.require_workspace()?;
        let path = self.root.join(".afmail/logs/events.jsonl");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&path).map_err(|e| AppError::io("open audit log", &e))?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|e| AppError::io("read audit log", &e))?;
            if line.trim().is_empty() {
                continue;
            }
            events.push(
                serde_json::from_str::<Value>(&line)
                    .map_err(|e| AppError::json("parse audit event", &e))?,
            );
        }
        Ok(events)
    }

    pub(super) fn checked_reason<'a>(&self, reason: Option<&'a str>) -> Result<Option<&'a str>> {
        let reason = reason.map(str::trim).filter(|value| !value.is_empty());
        let config = MailConfig::load(&self.root)?;
        match (config.audit.reason_mode, reason) {
            (ReasonMode::Required, None) => Err(AppError::new(
                "reason_required",
                "add --reason \"<why>\" or set audit.reason_mode=optional in config to skip",
            )
            .with_hint("Repeat the command with `--reason TEXT`, or set `audit.reason_mode` to `optional` for this workspace.")
            .with_details(json!({
                "required_option": "--reason",
                "config_key": "audit.reason_mode"
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

    pub(super) fn require_workspace(&self) -> Result<()> {
        if self.root.join(".afmail").is_dir() {
            Ok(())
        } else {
            Err(AppError::new(
                "workspace_not_found",
                "no .afmail directory found at workspace root",
            ))
        }
    }

    pub(super) fn first_related_message_date(&self, message_id: &str) -> Result<String> {
        let mut message_ids = self.related_message_ids(message_id)?;
        message_ids.push(message_id.to_string());
        let mut times = Vec::new();
        for id in message_ids {
            let message = self.read_message_by_id(&id)?;
            if let Some(time) = message_time(&message) {
                if let Ok(parsed) = DateTime::parse_from_rfc3339(&time) {
                    times.push(parsed);
                }
            }
        }
        times.sort();
        let offset = self.workspace_date_offset()?;
        Ok(times
            .first()
            .map(|time| time.with_timezone(&offset).format("%Y%m%d").to_string())
            .unwrap_or_else(|| workspace_local_date(&offset)))
    }

    pub(super) fn message_date(&self, message_id: &str) -> Result<String> {
        let message = self.read_message_by_id(message_id)?;
        let offset = self.workspace_date_offset()?;
        Ok(message_time(&message)
            .and_then(|time| DateTime::parse_from_rfc3339(&time).ok())
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

    pub(super) fn next_archive_uid(&self, date: &str) -> Result<String> {
        next_uid_for_date('a', date, self.archive_message_category_ids()?.into_iter())
    }

    pub(super) fn message_path(&self, message_id: &str) -> PathBuf {
        self.root
            .join("messages")
            .join(format!("{message_id}.json"))
    }
}
