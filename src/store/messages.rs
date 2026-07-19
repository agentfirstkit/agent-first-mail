use super::*;

pub(super) fn message_json_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let dir = root.join(layout::MESSAGES);
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
struct MessageRemoteFile {
    schema_name: String,
    schema_version: u64,
    message_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    locations: Vec<RemoteLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageCacheReadMode {
    Strict,
    Repair,
}

#[derive(Debug, Serialize)]
struct MessageDispositionResult {
    code: &'static str,
    message_id: String,
    special_use: String,
    message_ids: Vec<String>,
    removed_push_count: usize,
    removed_push_ids: Vec<String>,
    location_count: usize,
    queued_location_count: usize,
    queued: bool,
    push_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct MessageNotificationResult {
    code: &'static str,
    message_id: String,
    notification_uid: String,
    path: String,
    special_use: String,
    eligible_message_ids: Vec<String>,
    location_count: usize,
    queued_location_count: usize,
    queued: bool,
    push_ids: Vec<String>,
    push_id: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct UnmarkDiscardedMessageContext<'a> {
    expected_status: MessageStatus,
    undo_push_kind: &'a str,
    queue_push_kind: &'a str,
    source_kind: SpecialUseKind,
    steps: &'a [crate::config::ActionStep],
    result_code: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(super) struct MessageCacheRebuildTotals {
    pub(super) rebuilt_count: usize,
    pub(super) removed_text_cache_count: usize,
    pub(super) validated_eml_count: usize,
    pub(super) materialized_cache_written_count: usize,
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

fn read_message_remote_file(root: &Path, message_id: &str) -> Result<Option<MessageRemoteFile>> {
    let path = message_remote_path(root, message_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path).map_err(|e| {
        AppError::new(
            "store_error",
            format!("read message remote {}: {e}", rel_path(root, &path)),
        )
    })?;
    let remote: MessageRemoteFile = serde_json::from_str(&data).map_err(|e| {
        AppError::new(
            "store_error",
            format!("parse message remote {}: {e}", rel_path(root, &path)),
        )
    })?;
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
        notification_uid: None,
        notification_rfc3339: None,
        origin: None,
        remote_sync: None,
        push: None,
    }
}

pub(super) fn purge_message_artifacts(root: &Path, message_id: &str) -> Result<()> {
    validate_id("message_id", message_id)?;
    let message_dir = root.join(".afmail/messages");
    // Keep the canonical materialized message until every auxiliary artifact is
    // gone. A failed purge can then rediscover the disposition and resume.
    for path in [
        message_dir.join(format!("{message_id}.eml")),
        message_dir.join(format!("{message_id}.state.json")),
        message_dir.join(format!("{message_id}.remote.json")),
        message_dir.join(format!("{message_id}.txt")),
        message_dir.join(format!("{message_id}.json")),
    ] {
        if path.exists() {
            remove_file(&path)?;
        }
    }
    let files_dir = message_dir.join(format!("{message_id}.files"));
    if files_dir.exists() {
        remove_dir_all(&files_dir)?;
    }
    let materialized = root
        .join(layout::MESSAGES)
        .join(format!("{message_id}.json"));
    if materialized.exists() {
        remove_file(&materialized)?;
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
    let data = fs::read_to_string(path).map_err(|e| {
        AppError::new(
            "store_error",
            format!("read message json {}: {e}", path_to_string(path)),
        )
    })?;
    let message: MessageFile = serde_json::from_str(&data).map_err(|e| {
        AppError::new(
            "store_error",
            format!("parse message json {}: {e}", path_to_string(path)),
        )
    })?;
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

#[derive(Clone, Debug, Default)]
pub(super) struct MessageRelationIndex {
    rfc822_ids: BTreeMap<String, String>,
    quoted_by_message: BTreeMap<String, String>,
    related_by_message: BTreeMap<String, Vec<String>>,
}

pub(super) struct MaterializedOverlayIndex {
    cases: CaseIndex,
    dispositions: BTreeMap<String, MessageStatus>,
    notifications: BTreeMap<String, (String, String)>,
    pending_push: BTreeMap<String, Vec<WorkspacePendingPush>>,
}

pub(super) struct RenderStaticInputs {
    config: MailConfig,
    resolved_identities: Vec<ResolvedIdentity>,
    contact_map: BTreeMap<String, MessageContact>,
}

pub(super) struct MaterializationContext {
    static_inputs: Arc<RenderStaticInputs>,
    overlays: MaterializedOverlayIndex,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectedMessageSnapshot {
    pub(super) hash: String,
    pub(super) has_visible_quote: bool,
}

pub(super) struct RenderContext {
    materialization: MaterializationContext,
    message_relations: MessageRelationIndex,
    message_snapshots: BTreeMap<String, ProjectedMessageSnapshot>,
}

pub(super) struct RenderPreparation {
    pub(super) context: RenderContext,
    pub(super) materialization: MessageCacheRebuildTotals,
}

impl MessageRelationIndex {
    pub(super) fn build(workspace: &Workspace) -> Result<Self> {
        let messages = message_json_paths(&workspace.root)?
            .into_iter()
            .map(|path| read_message(&path))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::from_messages(&messages))
    }

    pub(super) fn from_messages(messages: &[MessageFile]) -> Self {
        let mut index = Self::default();
        let mut replies_by_parent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut reply_headers = BTreeMap::new();
        for message in messages {
            if let Some(normalized) = message
                .rfc822_message_id
                .as_deref()
                .and_then(normalize_rfc822_message_id)
            {
                index
                    .rfc822_ids
                    .entry(normalized)
                    .or_insert_with(|| message.message_id.clone());
            }
            let headers = message_reply_header_ids(message);
            for header_id in &headers {
                replies_by_parent
                    .entry(header_id.clone())
                    .or_default()
                    .insert(message.message_id.clone());
            }
            reply_headers.insert(message.message_id.clone(), headers);
        }
        for message in messages {
            let mut related = BTreeSet::new();
            let headers = reply_headers
                .get(&message.message_id)
                .cloned()
                .unwrap_or_default();
            for header_id in headers.iter().rev() {
                if let Some(local_message_id) = index.rfc822_ids.get(header_id) {
                    if local_message_id != &message.message_id {
                        related.insert(local_message_id.clone());
                    }
                    index
                        .quoted_by_message
                        .entry(message.message_id.clone())
                        .or_insert_with(|| local_message_id.clone());
                }
            }
            if let Some(current_rfc822_id) = message
                .rfc822_message_id
                .as_deref()
                .and_then(normalize_rfc822_message_id)
                && let Some(reply_ids) = replies_by_parent.get(&current_rfc822_id)
            {
                for reply_id in reply_ids {
                    if reply_id != &message.message_id {
                        related.insert(reply_id.clone());
                    }
                }
            }
            index
                .related_by_message
                .insert(message.message_id.clone(), related.into_iter().collect());
        }
        index
    }

    pub(super) fn quoted_message_id(&self, message_id: &str) -> Option<&str> {
        self.quoted_by_message.get(message_id).map(String::as_str)
    }

    pub(super) fn related_message_ids(&self, message_id: &str) -> &[String] {
        self.related_by_message
            .get(message_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

impl RenderStaticInputs {
    pub(super) fn load(workspace: &Workspace, config: MailConfig) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            resolved_identities: config.identity_profiles(&workspace.root)?,
            contact_map: workspace.contact_email_map()?,
            config,
        }))
    }
}

impl RenderContext {
    pub(super) fn config(&self) -> &MailConfig {
        &self.materialization.static_inputs.config
    }

    pub(super) fn relation_index(&self) -> &MessageRelationIndex {
        &self.message_relations
    }

    pub(super) fn quoted_message_id(&self, message_id: &str) -> Option<&str> {
        self.message_snapshots
            .get(message_id)
            .filter(|snapshot| snapshot.has_visible_quote)
            .and_then(|_| self.message_relations.quoted_message_id(message_id))
    }

    pub(super) fn related_message_ids(&self, message_id: &str) -> &[String] {
        self.message_relations.related_message_ids(message_id)
    }

    pub(super) fn snapshot(&self, message_id: &str) -> Option<&ProjectedMessageSnapshot> {
        self.message_snapshots.get(message_id)
    }

    pub(super) fn related_message_dependencies(
        &self,
        message_id: &str,
    ) -> Result<Vec<(&str, &str)>> {
        self.related_message_ids(message_id)
            .iter()
            .map(|related_id| {
                let snapshot = self.message_snapshots.get(related_id).ok_or_else(|| {
                    AppError::new(
                        "internal",
                        format!("missing render snapshot for related message {related_id}"),
                    )
                })?;
                Ok((related_id.as_str(), snapshot.hash.as_str()))
            })
            .collect()
    }

    pub(super) fn message_section_dependency(&self, message_id: &str) -> Result<Value> {
        let snapshot = self.message_snapshots.get(message_id).ok_or_else(|| {
            AppError::new(
                "internal",
                format!("missing render snapshot for message {message_id}"),
            )
        })?;
        Ok(json!({
            "message_id": message_id,
            "projected_message_hash": snapshot.hash,
            "quoted_message_id": self.quoted_message_id(message_id),
        }))
    }

    pub(super) fn read_message(
        &self,
        workspace: &Workspace,
        message_id: &str,
    ) -> Result<MessageFile> {
        validate_id("message_id", message_id)?;
        let mut message = read_message(&workspace.message_path(message_id))?;
        workspace.apply_materialized_workspace_overlays_with_index(
            &mut message,
            &self.materialization.overlays,
        )?;
        workspace.apply_identity_match_with_profiles(
            &mut message,
            &self.materialization.static_inputs.resolved_identities,
        )?;
        workspace.apply_contact_link(
            &mut message,
            &self.materialization.static_inputs.contact_map,
        );
        Ok(message)
    }
}

impl Workspace {
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
        self.write_message_materialized_cache_with_outcome(message)?;
        Ok(())
    }

    fn write_message_materialized_cache_with_outcome(
        &self,
        message: &MessageFile,
    ) -> Result<FileWriteOutcome> {
        let mut message = message.clone();
        message.schema_name = "message".to_string();
        message.schema_version = 1;
        if message.eml_path.is_none() {
            message.eml_path = Some(format!(".afmail/messages/{}.eml", message.message_id));
        }
        let deprecated_state = message_state_path(&self.root, &message.message_id);
        if deprecated_state.exists() {
            remove_file(&deprecated_state)?;
        }
        let path = self.message_path(&message.message_id);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        write_json_pretty_if_changed(&path, &message)
    }

    pub(super) fn write_message_cache(&self, message: &MessageFile) -> Result<()> {
        self.write_message_materialized_cache(message)
    }

    pub(crate) fn write_message_artifacts(&self, message: &MessageFile) -> Result<()> {
        self.persist_message_remote(message)?;
        self.write_message_materialized_cache(message)
    }

    fn materialize_message_cache_if_needed_with_context(
        &self,
        message_id: &str,
        context: &MaterializationContext,
    ) -> Result<bool> {
        if !self.message_cache_needs_materialize(message_id)? {
            return Ok(false);
        }
        self.materialize_message_cache_with_context(message_id, context)?;
        Ok(true)
    }

    fn materialize_message_cache_with_context(
        &self,
        message_id: &str,
        context: &MaterializationContext,
    ) -> Result<MessageFile> {
        let message = self.read_message_from_eml_with_context(
            message_id,
            MessageCacheReadMode::Repair,
            context,
        )?;
        self.write_message_materialized_cache(&message)?;
        Ok(message)
    }

    fn materialization_context(&self) -> Result<MaterializationContext> {
        let static_inputs = RenderStaticInputs::load(self, MailConfig::load(&self.root)?)?;
        self.materialization_context_with_static_inputs(static_inputs)
    }

    fn materialization_context_with_static_inputs(
        &self,
        static_inputs: Arc<RenderStaticInputs>,
    ) -> Result<MaterializationContext> {
        Ok(MaterializationContext {
            static_inputs,
            overlays: self.materialized_overlay_index()?,
        })
    }

    fn read_message_from_eml(
        &self,
        message_id: &str,
        cache_read_mode: MessageCacheReadMode,
    ) -> Result<MessageFile> {
        let context = self.materialization_context()?;
        self.read_message_from_eml_with_context(message_id, cache_read_mode, &context)
    }

    fn read_message_from_eml_with_context(
        &self,
        message_id: &str,
        cache_read_mode: MessageCacheReadMode,
        context: &MaterializationContext,
    ) -> Result<MessageFile> {
        validate_id("message_id", message_id)?;
        let eml_path = message_eml_path(&self.root, message_id);
        let raw = fs::read(&eml_path).map_err(|e| {
            AppError::new(
                "store_error",
                format!("read message eml {}: {e}", rel_path(&self.root, &eml_path)),
            )
        })?;
        let cache_path = self.message_path(message_id);
        let prior = if cache_path.is_file() {
            match read_message(&cache_path) {
                Ok(message) => Some(message),
                Err(_) if cache_read_mode == MessageCacheReadMode::Repair => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let remote =
            read_message_remote_file(&self.root, message_id)?.map(|file| file.remote_state());
        let direction = self.infer_materialized_direction(
            &raw,
            prior.as_ref(),
            remote.as_ref(),
            &context.static_inputs.config,
        );
        let mut parsed = crate::mail::parse_message_with_options(
            message_id.to_string(),
            &raw,
            crate::mail::MessageParseOptions {
                direction,
                workspace: prior
                    .as_ref()
                    .map(|message| message.workspace.clone())
                    .unwrap_or_else(default_message_workspace),
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
        )
        .map_err(|mut error| {
            error.message = format!(
                "parse message eml {}: {}",
                rel_path(&self.root, &eml_path),
                error.message
            );
            error
        })?;
        self.apply_fetched_attachment_files(message_id, &mut parsed.message.attachments);
        self.apply_materialized_workspace_overlays_with_index(
            &mut parsed.message,
            &context.overlays,
        )?;
        self.apply_identity_match_with_profiles(
            &mut parsed.message,
            &context.static_inputs.resolved_identities,
        )?;
        self.apply_contact_link(&mut parsed.message, &context.static_inputs.contact_map);
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
            message_remote_path(&self.root, message_id),
        ] {
            if !input.exists() {
                continue;
            }
            if let Ok(input_modified) = fs::metadata(&input).and_then(|meta| meta.modified())
                && input_modified > cache_modified
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn infer_materialized_direction(
        &self,
        raw: &[u8],
        prior: Option<&MessageFile>,
        remote: Option<&RemoteState>,
        config: &MailConfig,
    ) -> Option<String> {
        if let Some(direction) = prior.and_then(|message| message.direction.clone()) {
            return Some(direction);
        }
        if let Some(remote) = remote {
            for location in &remote.locations {
                if let Some(mailbox_id) = location.mailbox_id.as_deref()
                    && let Ok(action) = config.pull_action(mailbox_id)
                {
                    return Some(action.direction.as_str().to_string());
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

    pub(super) fn apply_materialized_workspace_overlays(
        &self,
        message: &mut MessageFile,
    ) -> Result<()> {
        let index = self.materialized_overlay_index()?;
        self.apply_materialized_workspace_overlays_with_index(message, &index)
    }

    fn pending_push_kind_for_overlay(&self, item: &PushItem, message_id: &str) -> String {
        if item.message().is_none() {
            return item.display_kind();
        }
        let Ok(message) = read_message(&self.message_path(message_id)) else {
            return item.display_kind();
        };
        match message.workspace.status.as_str() {
            "spam" => "message.spam".to_string(),
            "trashed" => "message.trash".to_string(),
            "case" => "case.add".to_string(),
            "notification" | "archived" => "message.notification".to_string(),
            "triage" => message
                .remote
                .as_ref()
                .and_then(|remote| {
                    remote.locations.iter().find_map(|location| {
                        let mailbox_id = location.mailbox_id.as_deref().unwrap_or_default();
                        let mailbox_name = location.mailbox_name.as_str();
                        if mailbox_id == "junk" || mailbox_name.eq_ignore_ascii_case("junk") {
                            Some("message.unspam".to_string())
                        } else if mailbox_id == "trash"
                            || mailbox_name.eq_ignore_ascii_case("trash")
                        {
                            Some("message.untrash".to_string())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_else(|| item.display_kind()),
            _ => item.display_kind(),
        }
    }

    pub(super) fn materialized_overlay_index(&self) -> Result<MaterializedOverlayIndex> {
        let mut pending_push: BTreeMap<String, Vec<WorkspacePendingPush>> = BTreeMap::new();
        for item in crate::push_queue::pending_items(&self.root)? {
            if item
                .local_stage
                .is_some_and(|stage| stage >= PushLocalStage::PendingCleared)
            {
                continue;
            }
            for message_id in item.message_ids() {
                let kind = self.pending_push_kind_for_overlay(&item, message_id);
                pending_push
                    .entry(message_id.clone())
                    .or_default()
                    .push(WorkspacePendingPush {
                        push_id: item.push_id.clone(),
                        kind,
                        queued_rfc3339: item.created_rfc3339.clone(),
                        last_error: item.last_error.clone(),
                    });
            }
        }
        for pending in pending_push.values_mut() {
            pending.sort_by(|a, b| a.push_id.cmp(&b.push_id));
        }
        Ok(MaterializedOverlayIndex {
            cases: CaseIndex::build(self)?,
            dispositions: self.disposition_state_by_message()?,
            notifications: self.notification_state_by_message()?,
            pending_push,
        })
    }

    pub(super) fn apply_materialized_workspace_overlays_with_index(
        &self,
        message: &mut MessageFile,
        index: &MaterializedOverlayIndex,
    ) -> Result<()> {
        message.workspace.notification_uid = None;
        message.workspace.notification_rfc3339 = None;
        if let Some(status) = index.dispositions.get(&message.message_id) {
            message.workspace.status = status.as_str().to_string();
            message.workspace.origin = None;
        } else if let Some((notification_uid, notification_rfc3339)) =
            index.notifications.get(&message.message_id)
        {
            message.workspace.status = MessageStatus::Notification.as_str().to_string();
            message.workspace.notification_uid = Some(notification_uid.clone());
            message.workspace.notification_rfc3339 = Some(notification_rfc3339.clone());
            message.workspace.origin = None;
        } else if index.cases.has_active_reference(&message.message_id) {
            message.workspace.status = MessageStatus::Case.as_str().to_string();
        } else if index.cases.has_archived_reference(&message.message_id)
            || message.workspace.origin.is_some()
        {
            message.workspace.status = MessageStatus::Archived.as_str().to_string();
        } else {
            message.workspace.status = MessageStatus::Triage.as_str().to_string();
        }
        let last_completed_rfc3339 = message
            .workspace
            .push
            .as_ref()
            .and_then(|push| push.last_completed_rfc3339.clone());
        message.workspace.push = index
            .pending_push
            .get(&message.message_id)
            .filter(|pending| !pending.is_empty())
            .map(|pending| WorkspacePushState {
                pending: pending.clone(),
                last_completed_rfc3339: None,
            });
        if let Some(last_completed_rfc3339) = last_completed_rfc3339 {
            let push = message
                .workspace
                .push
                .get_or_insert_with(WorkspacePushState::default);
            if push.last_completed_rfc3339.is_none() {
                push.last_completed_rfc3339 = Some(last_completed_rfc3339);
            }
        }
        Ok(())
    }

    /// Set `message.contact` from the sender address using a prebuilt
    /// email→contact map. Shared reads apply this in memory; explicit
    /// materialize/render paths may persist the projection in generated cache.
    pub(super) fn apply_contact_link(
        &self,
        message: &mut MessageFile,
        contact_map: &BTreeMap<String, MessageContact>,
    ) {
        message.contact = message
            .from
            .as_deref()
            .and_then(|from| super::contacts::contact_for_from(contact_map, from));
    }

    pub(crate) fn apply_identity_match(
        &self,
        message: &mut MessageFile,
        config: &MailConfig,
    ) -> Result<()> {
        let profiles = config.identity_profiles(&self.root)?;
        self.apply_identity_match_with_profiles(message, &profiles)
    }

    pub(super) fn apply_identity_match_with_profiles(
        &self,
        message: &mut MessageFile,
        profiles: &[ResolvedIdentity],
    ) -> Result<()> {
        if message
            .direction
            .as_deref()
            .and_then(|direction| MailDirection::parse(direction).ok())
            == Some(MailDirection::Outbound)
        {
            message.identity = None;
            message.identity_email = None;
            message.identity_match = None;
            message.identity_candidates.clear();
            message.observed_recipient_emails.clear();
            return Ok(());
        }
        let matched = match_message_identity_with_profiles(profiles, message);
        message.identity = matched.identity;
        message.identity_email = matched.identity_email;
        message.identity_match = Some(matched.identity_match);
        message.identity_candidates = matched.identity_candidates;
        message.observed_recipient_emails = matched.observed_recipient_emails;
        Ok(())
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
        let context = if eml_paths.is_empty() {
            None
        } else {
            Some(self.materialization_context()?)
        };

        for eml_path in eml_paths {
            let Some(message_id) = eml_path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            validate_id("message_id", message_id)?;
            let Some(context) = context.as_ref() else {
                continue;
            };
            if self.materialize_message_cache_if_needed_with_context(message_id, context)? {
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

    pub(super) fn prepare_render_context(
        &self,
        invocation: RenderInvocation,
        static_inputs: Arc<RenderStaticInputs>,
    ) -> Result<RenderPreparation> {
        if invocation == RenderInvocation::Pull {
            self.synchronize_notification_membership()?;
        }
        let materialization_context =
            self.materialization_context_with_static_inputs(static_inputs)?;
        let materialization =
            self.validate_and_materialize_messages_with_context(&materialization_context)?;
        let mut messages = Vec::new();
        let mut message_snapshots = BTreeMap::new();
        for path in message_json_paths(&self.root)? {
            let mut message = read_message(&path)?;
            self.apply_materialized_workspace_overlays_with_index(
                &mut message,
                &materialization_context.overlays,
            )?;
            self.apply_identity_match_with_profiles(
                &mut message,
                &materialization_context.static_inputs.resolved_identities,
            )?;
            self.apply_contact_link(
                &mut message,
                &materialization_context.static_inputs.contact_map,
            );
            message_snapshots.insert(
                message.message_id.clone(),
                ProjectedMessageSnapshot {
                    hash: blake3_json_fingerprint(&message)?,
                    has_visible_quote: message_body_has_visible_quote(&message.body_text),
                },
            );
            messages.push(message);
        }
        let message_relations = MessageRelationIndex::from_messages(&messages);
        Ok(RenderPreparation {
            context: RenderContext {
                materialization: materialization_context,
                message_relations,
                message_snapshots,
            },
            materialization,
        })
    }

    fn validate_and_materialize_messages_with_context(
        &self,
        context: &MaterializationContext,
    ) -> Result<MessageCacheRebuildTotals> {
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
            let needs_materialize = self.message_cache_needs_materialize(message_id)?;
            let message = self.read_message_from_eml_with_context(
                message_id,
                MessageCacheReadMode::Repair,
                context,
            )?;
            totals.validated_eml_count += 1;
            if self.write_message_materialized_cache_with_outcome(&message)?
                == FileWriteOutcome::Written
            {
                totals.materialized_cache_written_count += 1;
            }
            if needs_materialize {
                totals.rebuilt_count += 1;
            }
        }
        for path in message_json_paths(&self.root)? {
            let Some(message_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if message_eml_path(&self.root, message_id).is_file() {
                continue;
            }
            let mut message = read_message(&path)?;
            self.apply_materialized_workspace_overlays_with_index(&mut message, &context.overlays)?;
            self.apply_identity_match_with_profiles(
                &mut message,
                &context.static_inputs.resolved_identities,
            )?;
            self.apply_contact_link(&mut message, &context.static_inputs.contact_map);
            if self.write_message_materialized_cache_with_outcome(&message)?
                == FileWriteOutcome::Written
            {
                totals.materialized_cache_written_count += 1;
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

    fn notification_state_by_message(&self) -> Result<BTreeMap<String, (String, String)>> {
        let mut out = BTreeMap::new();
        for notification_uid in self.notification_ids()? {
            let data = self.read_notifications(&notification_uid)?;
            for item in data.items {
                out.insert(
                    item.message_id,
                    (notification_uid.clone(), item.added_rfc3339),
                );
            }
        }
        Ok(out)
    }

    pub fn message_list(
        &self,
        sender: Option<&str>,
        subject_contains: Option<&str>,
        since: Option<&str>,
        scope: &str,
    ) -> Result<Value> {
        self.require_workspace()?;
        let since_cutoff = match since {
            Some(value) => Some(parse_message_list_since(value)?),
            None => None,
        };
        let cases = Some(CaseIndex::build(self)?);
        let mut items = Vec::new();
        for path in message_json_paths(&self.root)? {
            let message = read_message(&path)?;
            if !message_list_scope_matches(&message.workspace.status, scope) {
                continue;
            }
            if let Some(sender_filter) = sender {
                let sender_text = message_list_sender_text(&message);
                if !sender_text
                    .to_ascii_lowercase()
                    .contains(&sender_filter.to_ascii_lowercase())
                {
                    continue;
                }
            }
            if let Some(subject_filter) = subject_contains {
                let Some(subject) = message.subject.as_deref() else {
                    continue;
                };
                if !subject
                    .to_ascii_lowercase()
                    .contains(&subject_filter.to_ascii_lowercase())
                {
                    continue;
                }
            }
            if let Some(cutoff) = since_cutoff {
                let Some(message_time) = message_list_message_time(&message) else {
                    continue;
                };
                if message_time < cutoff {
                    continue;
                }
            }
            let case_refs = cases
                .as_ref()
                .map(|index| index.case_refs_for_message(&message.message_id))
                .unwrap_or_default();
            let sort_key = message_list_message_time(&message);
            items.push((
                sort_key,
                json!({
                    "message_id": message.message_id,
                    "from": message.from.as_deref().unwrap_or(""),
                    "sender": message.sender.as_deref().unwrap_or(""),
                    "subject": message.subject.as_deref().unwrap_or(""),
                    "direction": message.direction.as_deref().unwrap_or(""),
                    "received_rfc3339": message.received_rfc3339.as_deref().unwrap_or(""),
                    "sent_rfc3339": message.sent_rfc3339.as_deref().unwrap_or(""),
                    "workspace_status": message.workspace.status.as_str(),
                    "notification_uid": message.workspace.notification_uid.as_deref(),
                    "case_refs": case_refs,
                    "json_path": format!("messages/{}.json", message.message_id),
                    "view_path": self.message_existing_view_path(&message)?,
                }),
            ));
        }
        items.sort_by(|(a_key, a_item), (b_key, b_item)| {
            b_key.cmp(a_key).then(
                a_item["message_id"]
                    .as_str()
                    .cmp(&b_item["message_id"].as_str()),
            )
        });
        let items = items.into_iter().map(|(_, item)| item).collect::<Vec<_>>();
        Ok(json!({
            "count": items.len(),
            "filters": {
                "sender": sender,
                "subject_contains": subject_contains,
                "since": since,
                "scope": scope,
            },
            "items": items,
        }))
    }

    pub fn message_show(&self, message_id: &str, meta: bool) -> Result<Value> {
        self.require_workspace()?;
        validate_id("message_id", message_id)?;
        let config = MailConfig::load(&self.root)?;
        let message = self.read_message_by_id(message_id)?;
        let flags = message_remote_flags(&message);
        let unread = !flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
        let flagged = flags
            .iter()
            .any(|flag| flag.eq_ignore_ascii_case("\\Flagged"));
        let push = message.workspace.push.clone();
        let contact = message.contact.clone();
        let related_message_ids = self.related_message_ids(message_id)?;
        let cases = CaseIndex::build(self)?;
        let related_messages =
            self.related_message_meta_rows(&related_message_ids, Some(&cases))?;
        let mut value = json!({
            "content_trust": "untrusted_remote",
            "content_provenance": { "source": "remote_mail", "message_id": message.message_id.as_str() },
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
            "attachment_count": message.attachments.len(),
            "attachments": attachment_metadata_values(&message.attachments),
            "mailbox_ids": message_mailbox_ids(&message, &config),
            "flags": flags,
            "unread": unread,
            "flagged": flagged,
            "workspace_status": message.workspace.status.as_str(),
            "notification_uid": message.workspace.notification_uid.as_deref(),
            "authentication_warning": message.authentication.is_warning(),
            "authentication": &message.authentication,
            "related_message_ids": related_message_ids,
            "related_messages": related_messages,
            "case_refs": cases.case_refs_for_message(message_id),
            "remote_missing": message_remote_missing(&message),
            "remote_missing_since_rfc3339": message_remote_missing_since_rfc3339(&message),
            "remote_effect_pending": message_remote_effect_pending(&message),
            "push": push,
            "identity": message.identity.as_deref().unwrap_or(""),
            "identity_email": message.identity_email.as_deref().unwrap_or(""),
            "identity_match": message.identity_match.as_deref().unwrap_or(""),
            "identity_candidates": &message.identity_candidates,
            "observed_recipient_emails": &message.observed_recipient_emails,
            "view_path": self.message_existing_view_path(&message)?,
            "json_path": format!("messages/{message_id}.json"),
            "contact": contact,
        });
        if !meta {
            let body_text = self.message_body_text(&message)?;
            if let Value::Object(map) = &mut value {
                map.insert("body_text".to_string(), json!(body_text));
                map.insert("message".to_string(), message_template_value(&message)?);
            }
        }
        Ok(value)
    }

    pub fn spam_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let config = crate::config::MailConfig::load(&self.root)?;
        validate_id("message_id", message_id)?;
        self.ensure_no_related_conversation(message_id)?;
        let message_ids = vec![message_id.to_string()];
        self.ensure_message_ids_unreferenced_except_message_pushes(
            &message_ids,
            None,
            &[
                crate::types::MessagePushAction::Trash,
                crate::types::MessagePushAction::Unspam,
                crate::types::MessagePushAction::Untrash,
            ],
        )?;
        let locations = self
            .message_remote_locations_except_mailbox_kinds(&message_ids, &[SpecialUseKind::Junk])?;
        let transaction = self.begin_transaction(
            "message_spam",
            vec![
                format!("messages/{message_id}.json"),
                ".afmail/push".to_string(),
            ],
        )?;
        let removed_push_ids = self.remove_pending_message_push_kinds(
            &message_ids,
            &["message.trash", "message.unspam", "message.untrash"],
        )?;
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            "message.spam",
            &message_ids,
            &locations,
            &config.actions.message.spam.steps,
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
            json!({
                "message_id": message_id,
                "special_use": SpecialUseKind::Junk.as_str(),
                "removed_push_ids": removed_push_ids.clone(),
            }),
        )?;
        serde_json::to_value(MessageDispositionResult {
            code: "message_spam_marked",
            message_id: message_id.to_string(),
            special_use: SpecialUseKind::Junk.as_str().to_string(),
            message_ids,
            removed_push_count: removed_push_ids.len(),
            removed_push_ids,
            location_count: locations.len(),
            queued_location_count: if item.is_some() { locations.len() } else { 0 },
            queued: item.is_some(),
            push_id: item.as_ref().map(|item| item.push_id.clone()),
        })
        .map_err(|e| AppError::json("serialize message disposition result", &e))
    }

    pub fn unspam_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        self.unmark_discarded_message(
            message_id,
            UnmarkDiscardedMessageContext {
                expected_status: MessageStatus::Spam,
                undo_push_kind: "message.spam",
                queue_push_kind: "message.unspam",
                source_kind: SpecialUseKind::Junk,
                steps: &config.actions.message.unspam.steps,
                result_code: "message_unspammed",
            },
            reason,
        )
    }

    pub fn untrash_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let config = crate::config::MailConfig::load(&self.root)?;
        self.unmark_discarded_message(
            message_id,
            UnmarkDiscardedMessageContext {
                expected_status: MessageStatus::Trashed,
                undo_push_kind: "message.trash",
                queue_push_kind: "message.untrash",
                source_kind: SpecialUseKind::Trash,
                steps: &config.actions.message.untrash.steps,
                result_code: "message_untrashed",
            },
            reason,
        )
    }

    pub fn notification_add_message(
        &self,
        message_id: &str,
        notification_ref: &str,
        summary: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let (notification_uid, notification_dir) = self.resolve_notification(notification_ref)?;
        self.ensure_no_related_conversation(message_id)?;
        let transaction = self.begin_transaction(
            "notification_message_add",
            vec![
                format!("messages/{message_id}.json"),
                format!("{}/{notification_uid}", layout::NOTIFICATIONS),
                ".afmail/push".to_string(),
            ],
        )?;
        let notification_rfc3339 =
            self.set_direct_message_notification(message_id, &notification_uid)?;
        self.upsert_notification_item(
            &notification_uid,
            message_id,
            summary,
            &notification_rfc3339,
        )?;
        self.refresh_notification(&notification_uid)?;
        let queue =
            self.queue_archive_for_archived_messages(&[message_id.to_string()], None, None)?;
        transaction.commit()?;
        self.append_audit_event(
            "notification_message_added",
            vec![
                audit_target("message", message_id),
                audit_target("notification", &notification_uid),
            ],
            reason,
            json!({
                "message_id": message_id,
                "notification_uid": notification_uid,
                "summary": summary,
                "to_path": format!("{}/views/messages/{message_id}.md", rel_path(&self.root, &notification_dir)),
            }),
        )?;
        serde_json::to_value(MessageNotificationResult {
            code: "notification_message_added",
            message_id: message_id.to_string(),
            notification_uid,
            path: rel_path(&self.root, &notification_dir),
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
        .map_err(|e| AppError::json("serialize message notification result", &e))
    }

    pub fn trash_message(&self, message_id: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let config = crate::config::MailConfig::load(&self.root)?;
        validate_id("message_id", message_id)?;
        self.ensure_no_related_conversation(message_id)?;
        let message_ids = vec![message_id.to_string()];
        self.ensure_message_ids_unreferenced_except_message_pushes(
            &message_ids,
            None,
            &[
                crate::types::MessagePushAction::Spam,
                crate::types::MessagePushAction::Unspam,
                crate::types::MessagePushAction::Untrash,
            ],
        )?;
        let locations = self.message_remote_locations_except_mailbox_kinds(
            &message_ids,
            &[SpecialUseKind::Trash],
        )?;
        let transaction = self.begin_transaction(
            "message_trash",
            vec![
                format!("messages/{message_id}.json"),
                ".afmail/push".to_string(),
            ],
        )?;
        let removed_push_ids = self.remove_pending_message_push_kinds(
            &message_ids,
            &["message.spam", "message.unspam", "message.untrash"],
        )?;
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            "message.trash",
            &message_ids,
            &locations,
            &config.actions.message.trash.steps,
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
            json!({
                "message_id": message_id,
                "special_use": SpecialUseKind::Trash.as_str(),
                "removed_push_ids": removed_push_ids.clone(),
            }),
        )?;
        serde_json::to_value(MessageDispositionResult {
            code: "message_trashed",
            message_id: message_id.to_string(),
            special_use: SpecialUseKind::Trash.as_str().to_string(),
            message_ids,
            removed_push_count: removed_push_ids.len(),
            removed_push_ids,
            location_count: locations.len(),
            queued_location_count: if item.is_some() { locations.len() } else { 0 },
            queued: item.is_some(),
            push_id: item.as_ref().map(|item| item.push_id.clone()),
        })
        .map_err(|e| AppError::json("serialize message disposition result", &e))
    }

    pub(super) fn message_existing_view_path(
        &self,
        message: &MessageFile,
    ) -> Result<Option<String>> {
        let message_id = message.message_id.as_str();
        let triage_path = self
            .root
            .join(layout::TRIAGE)
            .join(format!("{message_id}.md"));
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
            let case_messages = read_case_messages(&case_path, &case_uid);
            if case_messages
                .as_ref()
                .map(|messages| messages.contains_message(message_id))
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
        if let Some(notification_uid) = message.workspace.notification_uid.as_deref() {
            let notification_view = self.notification_view_path(notification_uid, message_id);
            if notification_view.is_file() {
                return Ok(Some(rel_path(&self.root, &notification_view)));
            }
        }
        Ok(None)
    }

    pub(super) fn message_body_text(&self, message: &MessageFile) -> Result<String> {
        Ok(message.body_text.clone())
    }

    pub(super) fn message_conversation_with_renderer_context(
        &self,
        message: &MessageFile,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        output_dir: Option<&Path>,
        render_context: Option<&RenderContext>,
    ) -> Result<String> {
        let body_text = self.message_body_text(message)?;
        let display_offset = config.resolved_timezone_offset();
        renderer.render(
            TemplateKey::MessageSection,
            &message_section_context_with_options(
                Some(&self.root),
                message,
                &body_text,
                config.template_language(),
                config
                    .default_identity()
                    .ok()
                    .map(|identity| identity.email.as_str()),
                MessageSectionRenderOptions {
                    display_offset: Some(&display_offset),
                    output_dir,
                    message_link: None,
                    quoted_message_id: render_context
                        .and_then(|context| context.quoted_message_id(&message.message_id)),
                },
            )?,
        )
    }

    pub(super) fn message_conversation_with_link_with_renderer_context(
        &self,
        message: &MessageFile,
        config: &MailConfig,
        renderer: &mut MarkdownTemplateRenderer<'_>,
        output_dir: Option<&Path>,
        message_link: &str,
        render_context: Option<&RenderContext>,
    ) -> Result<String> {
        let body_text = self.message_body_text(message)?;
        let display_offset = config.resolved_timezone_offset();
        renderer.render(
            TemplateKey::MessageSection,
            &message_section_context_with_options(
                Some(&self.root),
                message,
                &body_text,
                config.template_language(),
                config
                    .default_identity()
                    .ok()
                    .map(|identity| identity.email.as_str()),
                MessageSectionRenderOptions {
                    display_offset: Some(&display_offset),
                    output_dir,
                    message_link: Some(message_link),
                    quoted_message_id: render_context
                        .and_then(|context| context.quoted_message_id(&message.message_id)),
                },
            )?,
        )
    }

    pub(super) fn related_message_ids(&self, message_id: &str) -> Result<Vec<String>> {
        validate_id("message_id", message_id)?;
        let current = self.read_message_by_id(message_id)?;
        let index = MessageRelationIndex::build(self)?;
        Ok(index.related_message_ids(&current.message_id).to_vec())
    }

    pub(super) fn ensure_no_related_conversation(&self, message_id: &str) -> Result<()> {
        let related_message_ids = self.related_message_ids(message_id)?;
        if related_message_ids.is_empty() {
            return Ok(());
        }
        let mut suggested_commands = vec![format!(
            "afmail case create --name NAME --group CATEGORY --message {message_id} --reason TEXT"
        )];
        for related_id in &related_message_ids {
            suggested_commands.push(format!(
                "afmail case add CASE_REF {related_id} --reason TEXT"
            ));
        }
        suggested_commands.push("afmail case archive CASE_REF --reason TEXT".to_string());
        suggested_commands.push("afmail case trash CASE_REF --reason TEXT".to_string());
        Err(AppError::new(
            "message_has_related_conversation_use_case",
            "message has RFC-header-confirmed related conversation",
        )
        .with_hint(
            "Create a case for the conversation, add the related messages, then archive or trash the case.",
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
        self.refresh_notification_indexes()
    }

    pub(super) fn refresh_message_after_ref_change(&self, message_id: &str) -> Result<()> {
        validate_id("message_id", message_id)?;
        let mut msg = self.read_message_by_id(message_id)?;
        let cases = CaseIndex::build(self)?;
        self.apply_materialized_workspace_overlays(&mut msg)?;
        msg.workspace.remote_sync = None;
        self.write_message_materialized_cache(&msg)?;
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
            if disposition_spec_for_status(status).is_some() {
                self.set_message_disposition(status, message_id, None, &now_rfc3339())?;
            } else {
                self.clear_message_from_all_dispositions(message_id)?;
            }
            let mut msg = self.read_message_by_id(message_id)?;
            msg.workspace.status = status.as_str().to_string();
            if matches!(status, MessageStatus::Spam | MessageStatus::Trashed) {
                msg.workspace.notification_uid = None;
                msg.workspace.notification_rfc3339 = None;
                msg.workspace.origin = None;
            }
            msg.workspace.remote_sync = None;
            self.write_message_materialized_cache(&msg)?;
            self.remove_triage_view_for_message(message_id)?;
        }
        Ok(())
    }

    fn remove_pending_message_push_kinds(
        &self,
        message_ids: &[String],
        push_kinds: &[&str],
    ) -> Result<Vec<String>> {
        let mut push_ids = BTreeSet::new();
        for push_kind in push_kinds {
            let removed =
                self.remove_pending_message_pushes_for_messages(message_ids, push_kind)?;
            push_ids.extend(removed);
        }
        Ok(push_ids.into_iter().collect())
    }

    fn unmark_discarded_message(
        &self,
        message_id: &str,
        context: UnmarkDiscardedMessageContext<'_>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_id("message_id", message_id)?;
        let expected_status_name = context.expected_status.as_str();
        let message_ids = vec![message_id.to_string()];
        let source_locations =
            self.message_remote_locations_for_mailbox_kinds(&message_ids, &[context.source_kind])?;
        let mut message = self.read_message_by_id(message_id)?;
        let from_status = message.workspace.status.clone();
        let is_local_disposition = from_status == expected_status_name;
        let has_source_location = !source_locations.is_empty();
        let can_restore_from_remote = from_status == "triage" && has_source_location;
        if !(is_local_disposition || can_restore_from_remote) {
            return Err(AppError::new(
                "invalid_request",
                format!(
                    "message {message_id} is not {expected_status_name} and has no active {} remote location",
                    context.source_kind.as_str()
                ),
            ));
        }
        let transaction = self.begin_transaction(
            context.result_code,
            vec![
                format!("messages/{message_id}.json"),
                ".afmail/push".to_string(),
            ],
        )?;
        let removed_push = crate::push_queue::remove_pending_message_pushes(
            &self.root,
            message_id,
            context.undo_push_kind,
        )?;
        let push_ids = removed_push
            .iter()
            .map(|item| item.push_id.clone())
            .collect::<Vec<_>>();
        let item = crate::push_queue::queue_action_steps(
            &self.root,
            context.queue_push_kind,
            &message_ids,
            &source_locations,
            context.steps,
            None,
        )?;
        if is_local_disposition {
            self.clear_message_disposition(context.expected_status, message_id)?;
        }
        message.workspace.status = "triage".to_string();
        message.workspace.remote_sync = None;
        self.write_message_materialized_cache(&message)?;
        self.refresh_message_after_ref_change(message_id)?;
        self.clear_message_pending_pushes(message_id, &push_ids, false)?;
        if let Some(item) = &item {
            self.record_pending_push_item(item)?;
        }
        self.refresh_disposition_views()?;
        transaction.commit()?;
        self.append_audit_event(
            context.result_code,
            vec![audit_target("message", message_id)],
            reason,
            json!({
                "message_id": message_id,
                "from_status": from_status,
                "to_status": "triage",
                "removed_push_ids": push_ids.clone(),
                "queued_push_id": item.as_ref().map(|item| item.push_id.clone()),
                "source_special_use": context.source_kind.as_str(),
            }),
        )?;
        Ok(json!({
            "message_id": message_id,
            "from_status": from_status,
            "status": "triage",
            "triage_path": format!("{}/{message_id}.md", layout::TRIAGE),
            "removed_push_count": push_ids.len(),
            "removed_push_ids": push_ids,
            "location_count": source_locations.len(),
            "queued_location_count": if item.is_some() { source_locations.len() } else { 0 },
            "queued": item.is_some(),
            "push_id": item.as_ref().map(|item| item.push_id.clone()),
        }))
    }

    pub(super) fn remove_triage_view_for_message(&self, message_id: &str) -> Result<()> {
        let path = self
            .root
            .join(layout::TRIAGE)
            .join(format!("{message_id}.md"));
        if path.exists() {
            remove_file(&path)?;
        }
        Ok(())
    }

    pub(crate) fn read_message_by_id(&self, message_id: &str) -> Result<MessageFile> {
        validate_id("message_id", message_id)?;
        let eml_path = message_eml_path(&self.root, message_id);
        if eml_path.is_file() {
            return self.read_message_from_eml(message_id, MessageCacheReadMode::Strict);
        }
        let mut message = read_message(&self.message_path(message_id))?;
        self.apply_materialized_workspace_overlays(&mut message)?;
        let config = MailConfig::load(&self.root)?;
        self.apply_identity_match(&mut message, &config)?;
        let contact_map = self.contact_email_map()?;
        self.apply_contact_link(&mut message, &contact_map);
        Ok(message)
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

fn parse_message_list_since(value: &str) -> Result<DateTime<FixedOffset>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed);
    }
    let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|e| {
        AppError::new(
            "invalid_request",
            format!("invalid since filter {value}: {e}"),
        )
    })?;
    DateTime::parse_from_rfc3339(&format!("{date}T00:00:00Z")).map_err(|e| {
        AppError::new(
            "invalid_request",
            format!("invalid since filter {value}: {e}"),
        )
    })
}

fn message_list_message_time(message: &MessageFile) -> Option<DateTime<FixedOffset>> {
    message
        .received_rfc3339
        .as_deref()
        .or(message.sent_rfc3339.as_deref())
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
}

fn message_list_scope_matches(status: &str, scope: &str) -> bool {
    match scope {
        "triage" => status == "triage",
        "case" => matches!(status, "case" | "archived"),
        "notification" => status == "notification",
        "all" => true,
        _ => false,
    }
}

fn message_list_sender_text(message: &MessageFile) -> String {
    let mut parts = Vec::new();
    if let Some(from) = message.from.as_deref() {
        parts.push(from);
    }
    if let Some(sender) = message.sender.as_deref() {
        parts.push(sender);
    }
    if let Some(contact) = message.contact.as_ref() {
        parts.push(contact.display_name.as_str());
    }
    parts.join(" ")
}
