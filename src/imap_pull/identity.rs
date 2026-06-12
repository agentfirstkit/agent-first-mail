use super::*;

#[derive(Clone, Debug)]
pub(super) struct SavedMessage {
    pub(super) message_id: String,
    pub(super) rfc822_message_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RemoteLocationUpdate {
    pub(super) flags_updated: bool,
    pub(super) location_restored: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ImapKey {
    pub(super) mailbox: String,
    pub(super) uid_validity: u64,
    pub(super) uid: u64,
}

#[derive(Default)]
pub(super) struct RemoteIndex {
    pub(super) imap_keys: HashSet<ImapKey>,
    pub(super) imap_locations: HashMap<ImapKey, String>,
    pub(super) rfc822_ids: HashMap<String, String>,
    pub(super) rfc822_cases: HashMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CaseSuggestion {
    pub(super) case_uids: Vec<String>,
    pub(super) reason: Option<String>,
}

impl RemoteIndex {
    pub(super) fn suggest_case(&self, raw_eml: &[u8]) -> CaseSuggestion {
        let mut case_uids = BTreeSet::new();
        for message_id in reply_header_message_ids(raw_eml) {
            if let Some(ids) = self.rfc822_cases.get(&message_id) {
                case_uids.extend(ids.iter().cloned());
            }
        }
        let case_uids = case_uids.into_iter().collect::<Vec<_>>();
        let reason = if case_uids.is_empty() {
            None
        } else {
            Some("In-Reply-To/References matched local case message-id".to_string())
        };
        CaseSuggestion { case_uids, reason }
    }
}

pub(super) fn save_remote_message(
    root: &Path,
    remote: RemoteMessage,
    suggestion: &CaseSuggestion,
    target: &PullTarget,
    offset: FixedOffset,
    mail_config: &MailConfig,
) -> Result<SavedMessage> {
    let message_id = stable_message_id(root, &remote, offset);
    let mut parsed = parse_inbound_message(
        message_id.clone(),
        &remote.raw_eml,
        ImapRef {
            mailbox_name: remote.mailbox.clone(),
            uid_validity: remote.uid_validity,
            uid: remote.uid,
        },
    )?;
    if let Some(state) = parsed.message.remote.as_mut() {
        if let Some(location) = state.locations.first_mut() {
            location.mailbox_id = Some(target.id.clone());
            location.flags = remote.flags.clone();
        }
    }
    apply_pull_target(&mut parsed.message, target);
    parsed.conversation = crate::store::render_message_section_with_config(
        root,
        &parsed.message,
        &parsed.body_text,
        mail_config,
    )?;
    let messages_dir = root.join(".afmail/messages");
    create_dir_all(&messages_dir)?;
    let workspace = crate::store::Workspace::at(root);
    let cache_path = root.join("messages").join(format!("{message_id}.json"));
    if messages_dir.join(format!("{message_id}.eml")).exists() || cache_path.exists() {
        let existing = workspace.read_message_by_id(&message_id)?;
        if existing.rfc822_message_id != parsed.message.rfc822_message_id {
            return Err(AppError::new(
                "message_id_conflict",
                format!(
                    "message id {message_id} already exists with a different rfc822_message_id"
                ),
            ));
        }
    }
    write_bytes_atomic(
        &messages_dir.join(format!("{message_id}.eml")),
        &remote.raw_eml,
        "write eml",
    )?;
    workspace.write_message_artifacts(&parsed.message)?;
    if target.import_as == PullImportAs::Triage {
        create_dir_all(&root.join("triage"))?;
        write_string_atomic(
            &root.join("triage").join(format!("{message_id}.md")),
            &crate::store::render_triage_view(
                root,
                mail_config.template_language(),
                &parsed.message,
                &parsed.conversation,
                suggestion.case_uids.clone(),
                suggestion.reason.clone(),
                Vec::new(),
            )?,
        )?;
    }
    Ok(SavedMessage {
        message_id,
        rfc822_message_id: parsed.message.rfc822_message_id,
    })
}

pub(super) fn apply_pull_target(message: &mut MessageFile, target: &PullTarget) {
    message.workspace.status = target.import_as.as_str().to_string();
    message.direction = Some(target.direction.as_str().to_string());
    if target.direction == MailDirection::Outbound {
        if message.sent_rfc3339.is_none() {
            message.sent_rfc3339 = message.received_rfc3339.clone();
        }
        message.received_rfc3339 = None;
    }
}

pub(super) fn load_existing_remote_index(root: &Path) -> Result<RemoteIndex> {
    let mut out = RemoteIndex::default();
    let cases_by_message_id = load_case_refs_by_message_id(root)?;
    let dir = root.join(".afmail/messages");
    if !dir.exists() {
        return Ok(out);
    }
    let workspace = crate::store::Workspace::at(root);
    for entry in fs::read_dir(dir).map_err(|e| AppError::io("read messages", &e))? {
        let entry = entry.map_err(|e| AppError::io("read messages", &e))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("eml") {
            continue;
        }
        let Some(message_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let message = workspace.read_message_by_id(message_id)?;
        if let Some(rfc822_id) = message.rfc822_message_id.clone() {
            let key = normalize_message_id(&rfc822_id);
            out.rfc822_ids
                .insert(key.clone(), message.message_id.clone());
            if let Some(case_uids) = cases_by_message_id.get(&message.message_id) {
                out.rfc822_cases
                    .entry(key)
                    .or_default()
                    .extend(case_uids.iter().cloned());
            }
        }
        if let Some(remote) = message.remote {
            for location in remote.locations {
                if location.missing_rfc3339.is_some() {
                    continue;
                }
                if let (Some(uid_validity), Some(uid)) = (location.uid_validity, location.uid) {
                    let key = ImapKey {
                        mailbox: location.mailbox_name,
                        uid_validity,
                        uid,
                    };
                    out.imap_keys.insert(key.clone());
                    out.imap_locations
                        .entry(key)
                        .or_insert_with(|| message.message_id.clone());
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn load_case_refs_by_message_id(
    root: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let cases_dir = root.join("cases");
    if !cases_dir.exists() {
        return Ok(out);
    }
    for group in fs::read_dir(&cases_dir).map_err(|e| AppError::io("read cases", &e))? {
        let group = group.map_err(|e| AppError::io("read cases", &e))?;
        if !group.path().is_dir() {
            continue;
        }
        for case in fs::read_dir(group.path()).map_err(|e| AppError::io("read case group", &e))? {
            let case = case.map_err(|e| AppError::io("read case group", &e))?;
            let case_path = case.path();
            if !case_path.is_dir() {
                continue;
            }
            let messages_path = case_path.join("data").join("messages.json");
            if !messages_path.is_file() {
                continue;
            }
            let data = fs::read_to_string(&messages_path)
                .map_err(|e| AppError::io("read case messages", &e))?;
            let value: Value = serde_json::from_str(&data)
                .map_err(|e| AppError::json("parse case messages", &e))?;
            let case_uid = value
                .get("case_uid")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    case_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(ToString::to_string)
                })
                .unwrap_or_default();
            for message_id in value
                .get("message_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                out.entry(message_id.to_string())
                    .or_default()
                    .insert(case_uid.clone());
            }
        }
    }
    Ok(out)
}

pub(super) fn add_remote_location_from_envelope(
    root: &Path,
    message_id: &str,
    remote: &RemoteEnvelope,
    mailbox_id: Option<&str>,
) -> Result<RemoteLocationUpdate> {
    add_remote_location_parts(
        root,
        message_id,
        &remote.mailbox,
        remote.uid_validity,
        remote.uid,
        &remote.flags,
        mailbox_id,
    )
}

pub(super) fn add_remote_location_parts(
    root: &Path,
    message_id: &str,
    mailbox: &str,
    uid_validity: u64,
    uid: u64,
    flags: &[String],
    mailbox_id: Option<&str>,
) -> Result<RemoteLocationUpdate> {
    let workspace = crate::store::Workspace::at(root);
    let mut message = workspace.read_message_by_id(message_id)?;
    let location = RemoteLocation {
        mailbox_id: mailbox_id.map(ToString::to_string),
        mailbox_name: mailbox.to_string(),
        uid_validity: Some(uid_validity),
        uid: Some(uid),
        flags: flags.to_vec(),
        observed_rfc3339: crate::store::now_rfc3339(),
        missing_rfc3339: None,
    };
    let state = message.remote.get_or_insert_with(|| RemoteState {
        locations: Vec::new(),
    });
    let mut update = RemoteLocationUpdate::default();
    if let Some(existing) = state.locations.iter_mut().find(|existing| {
        existing.mailbox_name == location.mailbox_name
            && existing.uid_validity == location.uid_validity
            && existing.uid == location.uid
    }) {
        if existing.mailbox_id.is_none() {
            existing.mailbox_id = mailbox_id.map(ToString::to_string);
        }
        if existing.missing_rfc3339.take().is_some() {
            update.location_restored = true;
        }
        update.flags_updated = set_location_flags(existing, flags);
    } else {
        state.locations.push(location);
    }
    workspace.persist_message_remote(&message)?;
    workspace.write_message_materialized_cache(&message)?;
    Ok(update)
}

pub(super) fn update_remote_location_flags_from_envelope(
    root: &Path,
    message_id: &str,
    remote: &RemoteEnvelope,
) -> Result<RemoteLocationUpdate> {
    update_remote_location_flags_parts(
        root,
        message_id,
        &remote.mailbox,
        remote.uid_validity,
        remote.uid,
        &remote.flags,
    )
}

pub(super) fn update_remote_location_flags_parts(
    root: &Path,
    message_id: &str,
    mailbox: &str,
    uid_validity: u64,
    uid: u64,
    flags: &[String],
) -> Result<RemoteLocationUpdate> {
    let workspace = crate::store::Workspace::at(root);
    let mut message = workspace.read_message_by_id(message_id)?;
    let mut update = RemoteLocationUpdate::default();
    let mut changed = false;
    let state = message.remote.get_or_insert_with(|| RemoteState {
        locations: Vec::new(),
    });
    if let Some(location) = state.locations.iter_mut().find(|location| {
        location.mailbox_name == mailbox
            && location.uid_validity == Some(uid_validity)
            && location.uid == Some(uid)
    }) {
        if location.missing_rfc3339.take().is_some() {
            update.location_restored = true;
            changed = true;
        }
        update.flags_updated = set_location_flags(location, flags);
        changed |= update.flags_updated;
    } else {
        state.locations.push(RemoteLocation {
            mailbox_id: None,
            mailbox_name: mailbox.to_string(),
            uid_validity: Some(uid_validity),
            uid: Some(uid),
            flags: flags.to_vec(),
            observed_rfc3339: crate::store::now_rfc3339(),
            missing_rfc3339: None,
        });
        update.flags_updated = !flags.is_empty();
        changed = true;
    }
    if changed {
        workspace.persist_message_remote(&message)?;
        workspace.write_message_materialized_cache(&message)?;
    }
    Ok(update)
}

pub(super) fn set_location_flags(location: &mut RemoteLocation, flags: &[String]) -> bool {
    let flags = canonical_flags(flags.iter().cloned());
    if location.flags == flags {
        return false;
    }
    location.flags = flags;
    true
}

pub(super) fn rfc822_message_id(raw_eml: &[u8]) -> Option<String> {
    MessageParser::default()
        .parse(raw_eml)
        .and_then(|message| message.message_id().map(ToString::to_string))
}

pub(super) fn reply_header_message_ids(raw_eml: &[u8]) -> Vec<String> {
    let Some(message) = MessageParser::default().parse(raw_eml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    out.extend(header_message_ids(message.in_reply_to()));
    out.extend(header_message_ids(message.references()));
    out.sort();
    out.dedup();
    out
}

pub(super) fn header_message_ids(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(text) => extract_message_ids(text.as_ref()),
        HeaderValue::TextList(items) => items
            .iter()
            .flat_map(|item| extract_message_ids(item.as_ref()))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn extract_message_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        let id = normalize_message_id(&after_start[..end]);
        if !id.is_empty() {
            ids.push(id);
        }
        rest = &after_start[end + 1..];
    }
    if ids.is_empty() {
        ids.extend(
            text.split_whitespace()
                .map(normalize_message_id)
                .filter(|id| !id.is_empty()),
        );
    }
    ids.sort();
    ids.dedup();
    ids
}

pub(super) fn normalize_message_id(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| matches!(ch, '<' | '>' | ',' | ';'))
        .trim()
        .to_ascii_lowercase()
}

pub(super) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| AppError::io("create directory", &e))
}

pub(super) fn unique_message_id(root: &Path, base: &str) -> String {
    let durable_dir = root.join(".afmail/messages");
    let cache_dir = root.join("messages");
    if !durable_dir.join(format!("{base}.eml")).exists()
        && !durable_dir.join(format!("{base}.state.json")).exists()
        && !durable_dir.join(format!("{base}.remote.json")).exists()
        && !cache_dir.join(format!("{base}.json")).exists()
    {
        return base.to_string();
    }
    for i in 1..1000 {
        let candidate = format!("{base}_{i}");
        if !durable_dir.join(format!("{candidate}.eml")).exists()
            && !durable_dir.join(format!("{candidate}.state.json")).exists()
            && !durable_dir
                .join(format!("{candidate}.remote.json"))
                .exists()
            && !cache_dir.join(format!("{candidate}.json")).exists()
        {
            return candidate;
        }
    }
    format!("{base}_{}", crate::store::now_rfc3339().replace(':', ""))
}

pub(super) fn stable_message_id(
    root: &Path,
    remote: &RemoteMessage,
    offset: FixedOffset,
) -> String {
    let parsed = MessageParser::default().parse(&remote.raw_eml);
    // Date prefix from the message's own (immutable) Date header, rendered in the
    // workspace's configured offset. Absent Date -> no prefix.
    let date_prefix = parsed
        .as_ref()
        .and_then(|message| message.date())
        .map(|date| date.to_rfc3339())
        .and_then(|rfc3339| DateTime::parse_from_rfc3339(&rfc3339).ok())
        .map(|date| date.with_timezone(&offset).format("%Y%m%d").to_string());
    let hash = parsed
        .as_ref()
        .and_then(|message| message.message_id())
        .map(normalize_message_id)
        .filter(|normalized| !normalized.is_empty())
        .map(|normalized| fnv1a_hex(&normalized));
    if let Some(hash) = hash {
        return match date_prefix {
            Some(date) => format!("message_{date}_{hash}"),
            None => format!("message_{hash}"),
        };
    }
    // No usable Message-ID: freeze a location-derived id (cannot dedup across folders).
    let location = format!(
        "{}_{}_{}",
        slug_segment(&remote.mailbox),
        remote.uid_validity,
        remote.uid
    );
    let base = match date_prefix {
        Some(date) => format!("message_{date}_{location}"),
        None => format!("message_{location}"),
    };
    unique_message_id(root, &base)
}

pub(super) fn fnv1a_hex(value: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub(super) fn slug_segment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}
