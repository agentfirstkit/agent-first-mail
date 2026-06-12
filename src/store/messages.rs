use super::*;

pub(super) fn message_json_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let dir = root.join("messages");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = read_dir(&dir, "read messages")?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MessageStateFile {
    schema_name: String,
    schema_version: u64,
    message_id: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archive_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archived_rfc3339: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    updated_rfc3339: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MessageRemoteFile {
    schema_name: String,
    schema_version: u64,
    message_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    locations: Vec<RemoteLocation>,
}

#[derive(Debug, Serialize)]
struct MessageDispositionResult {
    code: &'static str,
    message_id: String,
    special_use: String,
    message_ids: Vec<String>,
    location_count: usize,
    queued_location_count: usize,
    queued: bool,
    push_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct MessageArchiveResult {
    code: &'static str,
    message_id: String,
    archive_uid: String,
    path: String,
    special_use: String,
    eligible_message_ids: Vec<String>,
    location_count: usize,
    queued_location_count: usize,
    queued: bool,
    push_ids: Vec<String>,
    push_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct MessageCacheRebuildTotals {
    pub(super) rebuilt_count: usize,
    pub(super) removed_text_cache_count: usize,
}

impl MessageStateFile {
    fn from_message(message: &MessageFile) -> Self {
        Self {
            schema_name: "message_state".to_string(),
            schema_version: 1,
            message_id: message.message_id.clone(),
            status: message.workspace.status.clone(),
            archive_uid: message.workspace.archive_uid.clone(),
            archived_rfc3339: message.workspace.archived_rfc3339.clone(),
            origin: message.workspace.origin.clone(),
            updated_rfc3339: now_rfc3339(),
        }
    }

    fn workspace(&self) -> Result<WorkspaceState> {
        let status = MessageStatus::parse(&self.status)?.as_str().to_string();
        Ok(WorkspaceState {
            status,
            archive_uid: self.archive_uid.clone(),
            archived_rfc3339: self.archived_rfc3339.clone(),
            origin: self.origin.clone(),
            remote_sync: None,
            push: None,
        })
    }
}

impl MessageRemoteFile {
    fn from_message(message: &MessageFile) -> Option<Self> {
        let remote = message.remote.as_ref()?;
        if remote.locations.is_empty() {
            return None;
        }
        Some(Self {
            schema_name: "message_remote".to_string(),
            schema_version: 1,
            message_id: message.message_id.clone(),
            locations: remote.locations.clone(),
        })
    }

    fn remote_state(&self) -> RemoteState {
        RemoteState {
            locations: self.locations.clone(),
        }
    }
}

fn message_eml_path(root: &Path, message_id: &str) -> PathBuf {
    root.join(".afmail/messages")
        .join(format!("{message_id}.eml"))
}

fn message_state_path(root: &Path, message_id: &str) -> PathBuf {
    root.join(".afmail/messages")
        .join(format!("{message_id}.state.json"))
}

fn message_remote_path(root: &Path, message_id: &str) -> PathBuf {
    root.join(".afmail/messages")
        .join(format!("{message_id}.remote.json"))
}

pub(super) fn message_state_updated_rfc3339(
    root: &Path,
    message_id: &str,
) -> Result<Option<String>> {
    Ok(read_message_state_file(root, message_id)?.map(|state| state.updated_rfc3339))
}

fn read_message_state_file(root: &Path, message_id: &str) -> Result<Option<MessageStateFile>> {
    let path = message_state_path(root, message_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = read_to_string(&path, "read message state")?;
    let state: MessageStateFile =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse message state", &e))?;
    if state.schema_name != "message_state"
        || state.schema_version != 1
        || state.message_id != message_id
    {
        return Err(AppError::new(
            "message_state_invalid",
            format!("invalid message state sidecar: {}", rel_path(root, &path)),
        ));
    }
    MessageStatus::parse(&state.status)?;
    Ok(Some(state))
}

fn write_message_state_file(root: &Path, state: &MessageStateFile) -> Result<()> {
    let path = message_state_path(root, &state.message_id);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let mut normalized = state.clone();
    normalized.schema_name = "message_state".to_string();
    normalized.schema_version = 1;
    write_json_pretty(&path, &normalized)
}

fn read_message_remote_file(root: &Path, message_id: &str) -> Result<Option<MessageRemoteFile>> {
    let path = message_remote_path(root, message_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = read_to_string(&path, "read message remote")?;
    let remote: MessageRemoteFile =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse message remote", &e))?;
    if remote.schema_name != "message_remote"
        || remote.schema_version != 1
        || remote.message_id != message_id
    {
        return Err(AppError::new(
            "message_remote_invalid",
            format!("invalid message remote sidecar: {}", rel_path(root, &path)),
        ));
    }
    Ok(Some(remote))
}

fn write_message_remote_file(root: &Path, remote: &MessageRemoteFile) -> Result<()> {
    let path = message_remote_path(root, &remote.message_id);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let mut normalized = remote.clone();
    normalized.schema_name = "message_remote".to_string();
    normalized.schema_version = 1;
    write_json_pretty(&path, &normalized)
}

fn default_message_workspace() -> WorkspaceState {
    WorkspaceState {
        status: "triage".to_string(),
        archive_uid: None,
        archived_rfc3339: None,
        origin: None,
        remote_sync: None,
        push: None,
    }
}

pub(super) fn purge_message_artifacts(root: &Path, message_id: &str) -> Result<()> {
    validate_id("message_id", message_id)?;
    let message_dir = root.join(".afmail/messages");
    for path in [
        root.join("messages").join(format!("{message_id}.json")),
        message_dir.join(format!("{message_id}.json")),
        message_dir.join(format!("{message_id}.eml")),
        message_dir.join(format!("{message_id}.state.json")),
        message_dir.join(format!("{message_id}.remote.json")),
        message_dir.join(format!("{message_id}.txt")),
    ] {
        if path.exists() {
            remove_file(&path)?;
        }
    }
    let files_dir = message_dir.join(format!("{message_id}.files"));
    if files_dir.exists() {
        remove_dir_all(&files_dir)?;
    }
    Ok(())
}

pub(super) fn attachment_metadata_values(attachments: &[AttachmentRef]) -> Vec<Value> {
    attachments
        .iter()
        .map(|attachment| {
            json!({
                "part_id": attachment.part_id.as_str(),
                "filename": attachment.filename.as_str(),
                "saved_filename": saved_filename_for_attachment(attachment),
                "content_type": attachment.content_type.as_str(),
                "size_bytes": attachment.size_bytes,
                "fetched": attachment.fetched,
                "file_path": attachment.file_path.as_deref().unwrap_or(""),
                "storage": if attachment.fetched { "message_cache" } else { "" },
            })
        })
        .collect()
}

pub(super) fn read_message(path: &Path) -> Result<MessageFile> {
    let data = read_to_string(path, "read message json")?;
    let message: MessageFile =
        serde_json::from_str(&data).map_err(|e| AppError::json("parse message json", &e))?;
    if message.schema_name != "message" || message.schema_version != 1 {
        return Err(AppError::new(
            "message_cache_invalid",
            format!("unsupported message cache schema: {}", path_to_string(path)),
        ));
    }
    MessageStatus::parse(&message.workspace.status)?;
    if let Some(direction) = message.direction.as_deref() {
        MailDirection::parse(direction)?;
    }
    Ok(message)
}

pub(super) fn normalize_rfc822_message_id(value: &str) -> Option<String> {
    let normalized = value
        .trim()
        .trim_matches(|ch| matches!(ch, '<' | '>' | ',' | ';'))
        .trim()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub(super) fn rfc822_message_id_candidates(value: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        if let Some(id) = normalize_rfc822_message_id(&after_start[..end]) {
            ids.push(id);
        }
        rest = &after_start[end + 1..];
    }
    if ids.is_empty() {
        ids.extend(
            value
                .split_whitespace()
                .filter_map(normalize_rfc822_message_id),
        );
    }
    ids.sort();
    ids.dedup();
    ids
}

pub(super) fn message_reply_header_ids(message: &MessageFile) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(in_reply_to) = &message.in_reply_to {
        ids.extend(rfc822_message_id_candidates(in_reply_to));
    }
    for reference in &message.references {
        ids.extend(rfc822_message_id_candidates(reference));
    }
    ids.sort();
    ids.dedup();
    ids
}

impl Workspace {
    pub(crate) fn persist_message_state(&self, message: &MessageFile) -> Result<()> {
        let state = MessageStateFile::from_message(message);
        write_message_state_file(&self.root, &state)
    }

    pub(crate) fn persist_message_remote(&self, message: &MessageFile) -> Result<()> {
        let path = message_remote_path(&self.root, &message.message_id);
        if let Some(remote) = MessageRemoteFile::from_message(message) {
            write_message_remote_file(&self.root, &remote)
        } else if path.exists() {
            remove_file(&path)
        } else {
            Ok(())
        }
    }

    pub(crate) fn write_message_materialized_cache(&self, message: &MessageFile) -> Result<()> {
        let mut message = message.clone();
        message.schema_name = "message".to_string();
        message.schema_version = 1;
        if message.eml_path.is_none() {
            message.eml_path = Some(format!(".afmail/messages/{}.eml", message.message_id));
        }
        let path = self.message_path(&message.message_id);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        write_json_pretty(&path, &message)
    }

    pub(super) fn write_message_cache(&self, message: &MessageFile) -> Result<()> {
        self.persist_message_state(message)?;
        self.write_message_materialized_cache(message)
    }

    pub(crate) fn write_message_artifacts(&self, message: &MessageFile) -> Result<()> {
        self.persist_message_state(message)?;
        self.persist_message_remote(message)?;
        self.write_message_materialized_cache(message)
    }

    fn materialize_message_cache_if_needed(&self, message_id: &str) -> Result<bool> {
        if !self.message_cache_needs_materialize(message_id)? {
            return Ok(false);
        }
        self.materialize_message_cache(message_id)?;
        Ok(true)
    }

    fn materialize_message_cache(&self, message_id: &str) -> Result<MessageFile> {
        validate_id("message_id", message_id)?;
        let eml_path = message_eml_path(&self.root, message_id);
        let raw = fs::read(&eml_path).map_err(|e| AppError::io("read message eml", &e))?;
        let prior = read_message(&self.message_path(message_id)).ok();
        let state = read_message_state_file(&self.root, message_id)?;
        let remote =
            read_message_remote_file(&self.root, message_id)?.map(|file| file.remote_state());
        let workspace = state
            .as_ref()
            .map(MessageStateFile::workspace)
            .transpose()?
            .unwrap_or_else(default_message_workspace);
        let direction = self.infer_materialized_direction(&raw, prior.as_ref(), remote.as_ref());
        let mut parsed = crate::mail::parse_message_with_options(
            message_id.to_string(),
            &raw,
            crate::mail::MessageParseOptions {
                direction,
                workspace,
                remote,
                received_rfc3339: prior
                    .as_ref()
                    .and_then(|message| message.received_rfc3339.clone()),
                sent_rfc3339: prior
                    .as_ref()
                    .and_then(|message| message.sent_rfc3339.clone()),
                attachments: prior
                    .as_ref()
                    .map(|message| message.attachments.clone())
                    .unwrap_or_default(),
            },
        )?;
        self.apply_fetched_attachment_files(message_id, &mut parsed.message.attachments);
        self.apply_materialized_workspace_overlays(&mut parsed.message)?;
        self.write_message_materialized_cache(&parsed.message)?;
        Ok(parsed.message)
    }

    fn message_cache_needs_materialize(&self, message_id: &str) -> Result<bool> {
        let cache_path = self.message_path(message_id);
        if !cache_path.exists() {
            return Ok(true);
        }
        let data = match read_to_string(&cache_path, "read message cache") {
            Ok(data) => data,
            Err(_) => return Ok(true),
        };
        let value: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => return Ok(true),
        };
        if value.get("schema_name").and_then(Value::as_str) != Some("message")
            || value.get("schema_version").and_then(Value::as_u64) != Some(1)
        {
            return Ok(true);
        }
        let cache_modified = match fs::metadata(&cache_path).and_then(|meta| meta.modified()) {
            Ok(time) => time,
            Err(_) => return Ok(true),
        };
        for input in [
            message_eml_path(&self.root, message_id),
            message_state_path(&self.root, message_id),
            message_remote_path(&self.root, message_id),
        ] {
            if !input.exists() {
                continue;
            }
            if let Ok(input_modified) = fs::metadata(&input).and_then(|meta| meta.modified()) {
                if input_modified > cache_modified {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn infer_materialized_direction(
        &self,
        raw: &[u8],
        prior: Option<&MessageFile>,
        remote: Option<&RemoteState>,
    ) -> Option<String> {
        if let Some(direction) = prior.and_then(|message| message.direction.clone()) {
            return Some(direction);
        }
        if let Some(remote) = remote {
            if let Ok(config) = MailConfig::load(&self.root) {
                for location in &remote.locations {
                    if let Some(mailbox_id) = location.mailbox_id.as_deref() {
                        if let Ok(action) = config.pull_action(mailbox_id) {
                            return Some(action.direction.as_str().to_string());
                        }
                    }
                }
            }
        }
        let local_message_id = mail_parser::MessageParser::default()
            .parse(raw)
            .and_then(|message| message.message_id().map(ToString::to_string))
            .is_some_and(|message_id| {
                message_id
                    .trim()
                    .trim_matches(|ch| matches!(ch, '<' | '>'))
                    .ends_with("@afmail.local")
            });
        Some(if local_message_id {
            "outbound".to_string()
        } else {
            "inbound".to_string()
        })
    }

    fn apply_fetched_attachment_files(&self, message_id: &str, attachments: &mut [AttachmentRef]) {
        let files_dir = self
            .root
            .join(".afmail/messages")
            .join(format!("{message_id}.files"));
        if !files_dir.is_dir() {
            return;
        }
        for attachment in attachments {
            if attachment.fetched
                && attachment
                    .file_path
                    .as_deref()
                    .is_some_and(|path| self.root.join(path).is_file())
            {
                continue;
            }
            let candidate = files_dir.join(safe_attachment_filename(
                &attachment.filename,
                &attachment.part_id,
            ));
            if candidate.is_file() {
                attachment.fetched = true;
                attachment.file_path = Some(rel_path(&self.root, &candidate));
            }
        }
    }

    fn apply_materialized_workspace_overlays(&self, message: &mut MessageFile) -> Result<()> {
        if let Some((archive_uid, archived_rfc3339)) = self
            .direct_archive_state_by_message()?
            .get(&message.message_id)
        {
            message.workspace.status = "archived".to_string();
            message.workspace.archive_uid = Some(archive_uid.clone());
            message.workspace.archived_rfc3339 = Some(archived_rfc3339.clone());
            message.workspace.origin = None;
        }
        let cases = CaseIndex::build(self)?;
        message.workspace.status = self.derived_message_status(message, &cases)?;
        message.workspace.push = self.pending_push_state_for_message(&message.message_id)?;
        Ok(())
    }

    fn pending_push_state_for_message(
        &self,
        message_id: &str,
    ) -> Result<Option<WorkspacePushState>> {
        let mut state = WorkspacePushState::default();
        for item in crate::push_queue::pending_items(&self.root)? {
            if !item.message_ids().iter().any(|id| id == message_id) {
                continue;
            }
            state.pending.push(WorkspacePendingPush {
                push_id: item.push_id.clone(),
                kind: item.display_kind(),
                queued_rfc3339: item.created_rfc3339.clone(),
                last_error: item.last_error.clone(),
            });
        }
        state.pending.sort_by(|a, b| a.push_id.cmp(&b.push_id));
        if state.pending.is_empty() && state.last_completed_rfc3339.is_none() {
            Ok(None)
        } else {
            Ok(Some(state))
        }
    }

    pub(super) fn rebuild_message_cache_from_eml(&self) -> Result<MessageCacheRebuildTotals> {
        let messages_dir = self.root.join(".afmail/messages");
        if !messages_dir.exists() {
            return Ok(MessageCacheRebuildTotals::default());
        }
        let mut totals = MessageCacheRebuildTotals::default();
        let mut eml_paths = read_dir(&messages_dir, "read message eml cache")?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("eml"))
            .collect::<Vec<_>>();
        eml_paths.sort();

        for eml_path in eml_paths {
            let Some(message_id) = eml_path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            validate_id("message_id", message_id)?;
            if self.materialize_message_cache_if_needed(message_id)? {
                totals.rebuilt_count += 1;
            }
        }

        for entry in read_dir(&messages_dir, "read message cache")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("txt") {
                remove_file(&path)?;
                totals.removed_text_cache_count += 1;
            }
        }
        Ok(totals)
    }

    fn direct_archive_state_by_message(&self) -> Result<BTreeMap<String, (String, String)>> {
        let mut out = BTreeMap::new();
        for archive_uid in self.archive_message_category_ids()? {
            let data = self.read_archive_messages(&archive_uid)?;
            for item in data.items {
                out.insert(
                    item.message_id,
                    (archive_uid.clone(), item.archived_rfc3339),
                );
            }
        }
        Ok(out)
    }

    pub fn message_show(&self, message_id: &str) -> Result<Value> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let config = MailConfig::load(&self.root)?;
        let message = self.read_message_by_id(message_id)?;
        let body_text = self.message_body_text(&message)?;
        let flags = message_remote_flags(&message);
        let unread = !flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
        let flagged = flags
            .iter()
            .any(|flag| flag.eq_ignore_ascii_case("\\Flagged"));
        let push = message.workspace.push.clone();
        Ok(json!({
            "code": "message_show",
            "message_id": message.message_id.as_str(),
            "from": message.from.as_deref().unwrap_or(""),
            "to": &message.to,
            "cc": &message.cc,
            "bcc": &message.bcc,
            "reply_to": &message.reply_to,
            "sender": message.sender.as_deref().unwrap_or(""),
            "subject": message.subject.as_deref().unwrap_or(""),
            "direction": message.direction.as_deref().unwrap_or(""),
            "received_rfc3339": message.received_rfc3339.as_deref().unwrap_or(""),
            "sent_rfc3339": message.sent_rfc3339.as_deref().unwrap_or(""),
            "body_text": body_text,
            "attachment_count": message.attachments.len(),
            "attachments": attachment_metadata_values(&message.attachments),
            "mailbox_ids": message_mailbox_ids(&message, &config),
            "flags": flags,
            "unread": unread,
            "flagged": flagged,
            "remote_missing": message_remote_missing(&message),
            "remote_missing_since_rfc3339": message_remote_missing_since_rfc3339(&message),
            "remote_effect_pending": message_remote_effect_pending(&message),
            "push": push,
            "view_path": self.message_existing_view_path(&message)?,
            "json_path": format!("messages/{message_id}.json"),
            "message": message_template_value(&message)?,
        }))
    }

    pub fn spam_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let config = crate::config::MailConfig::load(&self.root)?;
        validate_id("message_id", message_id)?;
        self.ensure_no_related_conversation(message_id)?;
        let message_ids = vec![message_id.to_string()];
        self.ensure_message_ids_unreferenced(&message_ids, None)?;
        let locations = self.message_remote_locations(&message_ids)?;
        let transaction = self.begin_transaction(
            "message_spam",
            vec![
                format!("messages/{message_id}.json"),
                ".afmail/push".to_string(),
            ],
        )?;
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            "message.spam",
            &message_ids,
            &locations,
            &config.actions.message_spam.steps,
            None,
        )?;
        self.update_messages_workspace(&message_ids, "spam")?;
        if let Some(item) = &item {
            self.record_pending_push_item(item)?;
        }
        self.refresh_disposition_views()?;
        transaction.commit()?;
        self.append_audit_event(
            "message_spam_marked",
            vec![audit_target("message", message_id)],
            reason,
            json!({"message_id": message_id, "special_use": SpecialUseKind::Junk.as_str()}),
        )?;
        serde_json::to_value(MessageDispositionResult {
            code: "message_spam_marked",
            message_id: message_id.to_string(),
            special_use: SpecialUseKind::Junk.as_str().to_string(),
            message_ids,
            location_count: locations.len(),
            queued_location_count: locations.len(),
            queued: item.is_some(),
            push_id: item.as_ref().map(|item| item.push_id.clone()),
        })
        .map_err(|e| AppError::json("serialize message disposition result", &e))
    }

    pub fn unspam_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.restore_local_message_disposition(
            message_id,
            "spam",
            "message.spam",
            "message_unspammed",
            "message_unspammed",
            reason,
        )
    }

    pub fn archive_message(
        &self,
        message_id: &str,
        archive_ref: &str,
        summary: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let (archive_uid, archive_dir) = self.resolve_archive_message_category(archive_ref)?;
        self.ensure_no_related_conversation(message_id)?;
        let transaction = self.begin_transaction(
            "message_archive",
            vec![
                format!("messages/{message_id}.json"),
                format!("archive/notifications/{archive_uid}"),
                ".afmail/push".to_string(),
            ],
        )?;
        let archived_rfc3339 = self.set_direct_message_archive(message_id, &archive_uid)?;
        self.upsert_archive_message_item(&archive_uid, message_id, summary, &archived_rfc3339)?;
        self.refresh_archive_message_category(&archive_uid)?;
        let queue = self.queue_archive_for_archived_messages(&[message_id.to_string()], None)?;
        transaction.commit()?;
        self.append_audit_event(
            "message_archived",
            vec![
                audit_target("message", message_id),
                audit_target("archive", &archive_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "archive_uid": archive_uid,
                "summary": summary,
                "to_path": format!("{}/views/messages/{message_id}.md", rel_path(&self.root, &archive_dir)),
            }),
        )?;
        serde_json::to_value(MessageArchiveResult {
            code: "message_archived",
            message_id: message_id.to_string(),
            archive_uid,
            path: rel_path(&self.root, &archive_dir),
            special_use: SpecialUseKind::Archive.as_str().to_string(),
            eligible_message_ids: queue.eligible_message_ids,
            location_count: queue.location_count,
            queued_location_count: queue.queued_location_count,
            queued: !queue.items.is_empty(),
            push_ids: queue
                .items
                .iter()
                .map(|item| item.push_id.clone())
                .collect::<Vec<_>>(),
            push_id: queue.items.first().map(|item| item.push_id.clone()),
        })
        .map_err(|e| AppError::json("serialize message archive result", &e))
    }

    pub fn trash_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let config = crate::config::MailConfig::load(&self.root)?;
        validate_id("message_id", message_id)?;
        self.ensure_no_related_conversation(message_id)?;
        let message_ids = vec![message_id.to_string()];
        self.ensure_message_ids_unreferenced(&message_ids, None)?;
        let locations = self.message_remote_locations(&message_ids)?;
        let transaction = self.begin_transaction(
            "message_trash",
            vec![
                format!("messages/{message_id}.json"),
                ".afmail/push".to_string(),
            ],
        )?;
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            "message.trash",
            &message_ids,
            &locations,
            &config.actions.message_trash.steps,
            None,
        )?;
        self.update_messages_workspace(&message_ids, "trashed")?;
        if let Some(item) = &item {
            self.record_pending_push_item(item)?;
        }
        self.refresh_disposition_views()?;
        transaction.commit()?;
        self.append_audit_event(
            "message_trashed",
            vec![audit_target("message", message_id)],
            reason,
            json!({"message_id": message_id, "special_use": SpecialUseKind::Trash.as_str()}),
        )?;
        serde_json::to_value(MessageDispositionResult {
            code: "message_trashed",
            message_id: message_id.to_string(),
            special_use: SpecialUseKind::Trash.as_str().to_string(),
            message_ids,
            location_count: locations.len(),
            queued_location_count: locations.len(),
            queued: item.is_some(),
            push_id: item.as_ref().map(|item| item.push_id.clone()),
        })
        .map_err(|e| AppError::json("serialize message disposition result", &e))
    }

    pub fn untrash_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.restore_local_message_disposition(
            message_id,
            "trashed",
            "message.trash",
            "message_untrashed",
            "message_untrashed",
            reason,
        )
    }

    pub fn unarchive_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let message = self.read_message_by_id(message_id)?;
        let archive_uid = message.workspace.archive_uid.clone().ok_or_else(|| {
            AppError::new(
                "invalid_request",
                format!("message {message_id} is not directly archived"),
            )
        })?;
        self.restore_direct_archive_message(
            &archive_uid,
            message_id,
            reason,
            "message_unarchived",
            "message_unarchived",
        )
    }

    pub(super) fn message_existing_view_path(
        &self,
        message: &MessageFile,
    ) -> Result<Option<String>> {
        let message_id = message.message_id.as_str();
        let triage_path = self.root.join("triage").join(format!("{message_id}.md"));
        if triage_path.is_file() {
            return Ok(Some(rel_path(&self.root, &triage_path)));
        }
        for dir in ["spam", "trash", "deleted"] {
            let path = self.root.join(dir).join(format!("{message_id}.md"));
            if path.is_file() {
                return Ok(Some(rel_path(&self.root, &path)));
            }
        }
        for (case_uid, case_path) in self.all_case_entries()? {
            let messages_path = case_messages_json_path(&case_path);
            let case_messages = read_case_messages(&messages_path, &case_uid);
            if case_messages
                .as_ref()
                .map(|messages| messages.message_ids.iter().any(|id| id == message_id))
                .unwrap_or(false)
            {
                let message_view = case_message_view_path(&case_path, message_id);
                if message_view.is_file() {
                    return Ok(Some(rel_path(&self.root, &message_view)));
                }
                let case_view = case_path.join("case.md");
                if case_view.is_file() {
                    return Ok(Some(rel_path(&self.root, &case_view)));
                }
            }
        }
        if let Some(archive_uid) = message.workspace.archive_uid.as_deref() {
            let archive_view = self.archive_message_view_path(archive_uid, message_id);
            if archive_view.is_file() {
                return Ok(Some(rel_path(&self.root, &archive_view)));
            }
        }
        Ok(None)
    }

    pub(super) fn message_body_text(&self, message: &MessageFile) -> Result<String> {
        Ok(message.body_text.clone())
    }

    pub(super) fn message_conversation_for_dir(
        &self,
        message: &MessageFile,
        output_dir: Option<&Path>,
    ) -> Result<String> {
        let config = MailConfig::load(&self.root)?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, config.template_language());
        self.message_conversation_with_renderer(message, &config, &mut renderer, output_dir)
    }

    pub(super) fn message_conversation_with_renderer(
        &self,
        message: &MessageFile,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        output_dir: Option<&Path>,
    ) -> Result<String> {
        let body_text = self.message_body_text(message)?;
        renderer.render(
            TemplateKey::MessageSection,
            &message_section_context(
                Some(&self.root),
                message,
                &body_text,
                config.template_language(),
                config
                    .smtp
                    .from
                    .as_deref()
                    .or(config.imap.username.as_deref()),
                output_dir,
            )?,
        )
    }

    pub(super) fn rfc822_message_id_index(&self) -> Result<BTreeMap<String, String>> {
        self.rebuild_message_cache_from_eml()?;
        let mut index = BTreeMap::new();
        for path in message_json_paths(&self.root)? {
            let message = read_message(&path)?;
            if let Some(normalized) = message
                .rfc822_message_id
                .as_deref()
                .and_then(normalize_rfc822_message_id)
            {
                index
                    .entry(normalized)
                    .or_insert_with(|| message.message_id.clone());
            }
        }
        Ok(index)
    }

    pub(super) fn related_message_ids(&self, message_id: &str) -> Result<Vec<String>> {
        validate_id("message_id", message_id)?;
        let current = self.read_message_by_id(message_id)?;
        let rfc822_index = self.rfc822_message_id_index()?;
        let mut related = BTreeSet::new();

        for header_id in message_reply_header_ids(&current) {
            if let Some(local_message_id) = rfc822_index.get(&header_id) {
                if local_message_id != message_id {
                    related.insert(local_message_id.clone());
                }
            }
        }

        let Some(current_rfc822_id) = current
            .rfc822_message_id
            .as_deref()
            .and_then(normalize_rfc822_message_id)
        else {
            return Ok(related.into_iter().collect());
        };

        for path in message_json_paths(&self.root)? {
            let other = read_message(&path)?;
            if other.message_id == message_id {
                continue;
            }
            if message_reply_header_ids(&other)
                .iter()
                .any(|header_id| header_id == &current_rfc822_id)
            {
                related.insert(other.message_id);
            }
        }

        Ok(related.into_iter().collect())
    }

    pub(super) fn ensure_no_related_conversation(&self, message_id: &str) -> Result<()> {
        let related_message_ids = self.related_message_ids(message_id)?;
        if related_message_ids.is_empty() {
            return Ok(());
        }
        let mut suggested_commands = vec![format!(
            "afmail case create --name NAME --message {message_id} --reason TEXT"
        )];
        for related_id in &related_message_ids {
            suggested_commands.push(format!(
                "afmail case add CASE_REF {related_id} --reason TEXT"
            ));
        }
        suggested_commands.push("afmail case archive CASE_REF --reason TEXT".to_string());
        Err(AppError::new(
            "message_has_related_conversation_use_case",
            "message has RFC-header-confirmed related conversation",
        )
        .with_hint(
            "Create a case for the conversation, add the related messages, then archive the case.",
        )
        .with_details(json!({
            "message_id": message_id,
            "related_message_ids": related_message_ids,
            "suggested_commands": suggested_commands
        })))
    }

    pub(super) fn refresh_messages_after_ref_change(&self, message_ids: &[String]) -> Result<()> {
        for message_id in message_ids {
            self.refresh_message_after_ref_change(message_id)?;
        }
        Ok(())
    }

    pub(super) fn refresh_read_views_after_message_change(&self, message_id: &str) -> Result<()> {
        validate_id("message_id", message_id)?;
        let message = self.read_message_by_id(message_id)?;
        let cases = CaseIndex::build(self)?;
        if self.triage_candidate(&message, &cases)? {
            self.write_triage_view(&message)?;
        } else {
            self.remove_triage_view_for_message(message_id)?;
        }
        self.refresh_all_case_message_views()?;
        self.refresh_archive_indexes()
    }

    pub(super) fn refresh_message_after_ref_change(&self, message_id: &str) -> Result<()> {
        validate_id("message_id", message_id)?;
        let mut msg = self.read_message_by_id(message_id)?;
        let cases = CaseIndex::build(self)?;
        msg.workspace.status = self.derived_message_status(&msg, &cases)?;
        msg.workspace.remote_sync = None;
        self.write_message_cache(&msg)?;
        if self.triage_candidate(&msg, &cases)? {
            self.write_triage_view(&msg)?;
        } else {
            self.remove_triage_view_for_message(message_id)?;
        }
        Ok(())
    }

    pub(super) fn update_messages_workspace(
        &self,
        message_ids: &[String],
        status: &str,
    ) -> Result<()> {
        let status = MessageStatus::parse(status)?;
        for message_id in message_ids {
            validate_id("message_id", message_id)?;
            let mut msg = self.read_message_by_id(message_id)?;
            msg.workspace.status = status.as_str().to_string();
            if matches!(status, MessageStatus::Spam | MessageStatus::Trashed) {
                msg.workspace.archive_uid = None;
                msg.workspace.archived_rfc3339 = None;
                msg.workspace.origin = None;
            }
            msg.workspace.remote_sync = None;
            self.write_message_cache(&msg)?;
            self.remove_triage_view_for_message(message_id)?;
        }
        Ok(())
    }

    pub(super) fn restore_local_message_disposition(
        &self,
        message_id: &str,
        expected_status: &str,
        push_kind: &str,
        event_kind: &str,
        result_code: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let mut message = self.read_message_by_id(message_id)?;
        if message.workspace.status != expected_status {
            return Err(AppError::new(
                "invalid_request",
                format!("message {message_id} is not {expected_status}"),
            ));
        }
        let removed_push =
            crate::push_queue::remove_pending_message_pushes(&self.root, message_id, push_kind)?;
        let push_ids = removed_push
            .iter()
            .map(|item| item.push_id.clone())
            .collect::<Vec<_>>();
        message.workspace.status = "triage".to_string();
        message.workspace.remote_sync = None;
        self.write_message_cache(&message)?;
        self.refresh_message_after_ref_change(message_id)?;
        self.clear_message_pending_pushes(message_id, &push_ids, false)?;
        self.refresh_disposition_views()?;
        self.append_audit_event(
            event_kind,
            vec![audit_target("message", message_id)],
            reason,
            json!({
                "message_id": message_id,
                "from_status": expected_status,
                "to_status": "triage",
                "removed_push_ids": push_ids.clone(),
            }),
        )?;
        Ok(json!({
            "code": result_code,
            "message_id": message_id,
            "from_status": expected_status,
            "status": "triage",
            "triage_path": format!("triage/{message_id}.md"),
            "removed_push_count": push_ids.len(),
            "push_ids": push_ids,
        }))
    }

    pub(super) fn remove_triage_view_for_message(&self, message_id: &str) -> Result<()> {
        let path = self.root.join("triage").join(format!("{message_id}.md"));
        if path.exists() {
            remove_file(&path)?;
        }
        Ok(())
    }

    pub(crate) fn read_message_by_id(&self, message_id: &str) -> Result<MessageFile> {
        validate_id("message_id", message_id)?;
        if message_eml_path(&self.root, message_id).is_file() {
            self.materialize_message_cache_if_needed(message_id)?;
        }
        let path = self.message_path(message_id);
        read_message(&path)
    }

    pub(crate) fn relocate_message(
        &self,
        message_id: &str,
        target_locations: &[crate::types::RemoteLocation],
    ) -> Result<()> {
        validate_id("message_id", message_id)?;
        let mut locations: Vec<crate::types::RemoteLocation> = Vec::new();
        for location in target_locations {
            if !locations.iter().any(|existing| {
                existing.mailbox_name == location.mailbox_name
                    && existing.uid_validity == location.uid_validity
                    && existing.uid == location.uid
            }) {
                locations.push(location.clone());
            }
        }
        if locations.is_empty() {
            return Ok(());
        };
        let mut message = self.read_message_by_id(message_id)?;
        message.remote = Some(crate::types::RemoteState { locations });
        self.persist_message_remote(&message)?;
        self.write_message_materialized_cache(&message)?;
        Ok(())
    }
}
