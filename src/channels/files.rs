use super::{ChannelKind, MessageOrigin, ReplyRoute, require_files, validate_identifier};
use crate::config::MailConfig;
use crate::error::{AppError, Result};
use crate::frontmatter::DraftFrontmatter;
use crate::types::{MessageFile, OutboundDelivery, PreparedOutbound};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FilesSection {
    #[serde(default)]
    pub sources: BTreeMap<String, FileSourceConfig>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileSourceConfig {
    pub inbox_path: PathBuf,
    pub outbox_path: PathBuf,
    #[serde(default = "default_max_bytes")]
    pub max_message_bytes: u64,
    #[serde(default = "default_max_files")]
    pub max_input_files: usize,
}
fn default_max_bytes() -> u64 {
    1024 * 1024
}
fn default_max_files() -> usize {
    10000
}
impl FilesSection {
    pub(crate) fn validate(&self) -> Result<()> {
        for (id, source) in &self.sources {
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(AppError::new("config_invalid", "invalid file source id"));
            }
            if source.inbox_path.as_os_str().is_empty()
                || source.outbox_path.as_os_str().is_empty()
                || source.max_message_bytes == 0
                || source.max_message_bytes > 100 * 1024 * 1024
                || source.max_input_files == 0
            {
                return Err(AppError::new(
                    "config_invalid",
                    "file sources require inbox/outbox paths and positive bounds (maximum message size 100 MiB)",
                ));
            }
        }
        // Relative paths are checked lexically here; `checked_paths` repeats the
        // check against the real workspace root, where absolute and aliased
        // spellings resolve.
        if let Some((_, error)) = self.path_conflicts(Path::new("")).into_iter().next() {
            return Err(error);
        }
        Ok(())
    }

    /// Resolve every source's inbox and outbox against `base` and report each
    /// source whose paths overlap afmail-managed state or another channel's
    /// inbox. An empty `base` keeps relative paths relative (no filesystem).
    fn path_conflicts(&self, base: &Path) -> BTreeMap<String, AppError> {
        let physical = !base.as_os_str().is_empty();
        let place = |path: &Path| {
            let joined = normalize(&base.join(path));
            if physical {
                resolve_existing(&joined)
            } else {
                joined
            }
        };
        let root = if physical {
            place(Path::new(""))
        } else {
            PathBuf::new()
        };
        let managed: Vec<PathBuf> = MANAGED_DIRECTORIES
            .iter()
            .map(|name| root.join(name))
            .collect();
        let resolved: Vec<_> = self
            .sources
            .iter()
            .map(|(id, source)| (id, place(&source.inbox_path), place(&source.outbox_path)))
            .collect();
        let mut conflicts = BTreeMap::new();
        for (id, inbox, outbox) in &resolved {
            for (kind, path) in [("inbox", inbox), ("outbox", outbox)] {
                if *path == root || managed.iter().any(|dir| path.starts_with(dir)) {
                    conflicts.entry((*id).clone()).or_insert_with(|| {
                        AppError::new(
                            "config_invalid",
                            format!("file source {id} {kind} must not be the workspace root or lie inside an afmail-managed directory"),
                        )
                    });
                }
            }
            for (other, other_inbox, _) in &resolved {
                // Identity is per source, so a shared inbox imports every file twice.
                if id != other && inbox == other_inbox {
                    for owner in [id, other] {
                        conflicts.entry((*owner).clone()).or_insert_with(|| {
                            AppError::new(
                                "config_invalid",
                                format!("file source {id} inbox must differ from the inbox of file source {other}"),
                            )
                        });
                    }
                }
                if outbox == other_inbox {
                    for owner in [id, other] {
                        conflicts.entry((*owner).clone()).or_insert_with(|| {
                            AppError::new(
                                "config_invalid",
                                format!("file source {id} outbox must differ from the inbox of file source {other}"),
                            )
                        });
                    }
                }
            }
        }
        conflicts
    }
}

/// Workspace directories afmail writes or regenerates. A channel directory
/// inside one would let afmail read its own state or overwrite producer files.
const MANAGED_DIRECTORIES: &[&str] = &[
    crate::layout::AFMAIL,
    crate::layout::TRIAGE,
    crate::layout::CASES,
    crate::layout::ARCHIVED_CASES,
    crate::layout::NOTIFICATIONS,
    crate::layout::MESSAGES,
    crate::layout::TEMPLATES,
    crate::layout::SPAM,
    crate::layout::TRASH,
    crate::layout::DELETED,
    crate::layout::CONTACTS,
    crate::layout::IDENTITIES,
];

/// Lexically remove `.`, `..` and trailing separators without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}
/// Resolve symbolic links in the longest existing ancestor so two spellings of
/// one directory compare equal even when the directory does not exist yet.
fn resolve_existing(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(mut resolved) = fs::canonicalize(existing) {
            for name in rest.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// A native message. IDs are opaque channel identifiers, not email addresses.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeMessage {
    pub schema_name: String,
    pub schema_version: u64,
    pub external_message_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub recipient_ids: Vec<String>,
    pub subject: String,
    pub body_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_external_message_id: Option<String>,
}
impl NativeMessage {
    fn validate(&self) -> Result<()> {
        if self.schema_name != "channel_message"
            || self.schema_version != 1
            || self.recipient_ids.is_empty()
        {
            return Err(AppError::new(
                "channel_message_invalid",
                "expected channel_message v1 with at least one recipient",
            ));
        }
        for id in [
            &self.external_message_id,
            &self.conversation_id,
            &self.sender_id,
        ]
        .into_iter()
        .chain(self.recipient_ids.iter())
        .chain(self.reply_to_external_message_id.iter())
        {
            validate_identifier(id)?;
        }
        validate_identifier(&self.subject)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredMessage {
    source_id: String,
    direction: String,
    timestamp_rfc3339: String,
    /// Original UTF-8 JSON, retained byte for byte inside the evidence envelope.
    content_json: String,
}
fn decode(raw: &[u8]) -> Result<StoredMessage> {
    serde_json::from_slice(raw).map_err(|e| AppError::json("parse native message evidence", &e))
}
fn native(raw: &str) -> Result<NativeMessage> {
    let message: NativeMessage =
        serde_json::from_str(raw).map_err(|e| AppError::json("parse channel message", &e))?;
    message.validate()?;
    Ok(message)
}
pub(crate) fn materialize(message_id: &str, raw: &[u8]) -> Result<MessageFile> {
    let stored = decode(raw)?;
    let message = native(&stored.content_json)?;
    serde_json::from_value(json!({
        "schema_name":"message", "schema_version":crate::types::MESSAGE_SCHEMA_VERSION,
        "message_id":message_id,"direction":stored.direction,
        "origin":MessageOrigin { reply_route: ReplyRoute { channel:ChannelKind::File, source_id:stored.source_id, conversation_id:message.conversation_id }, external_message_id:message.external_message_id, reply_to_external_message_id:message.reply_to_external_message_id },
        "subject":message.subject,"from":message.sender_id,"to":message.recipient_ids,
        "body_text":message.body_text,
        "received_rfc3339":if stored.direction == "inbound" { Some(&stored.timestamp_rfc3339) } else { None },
        "sent_rfc3339":if stored.direction == "outbound" { Some(&stored.timestamp_rfc3339) } else { None },
        "workspace":{"status":"triage"}
    })).map_err(|e| AppError::json("materialize channel message", &e))
}
fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}
fn source<'a>(config: &'a MailConfig, route: &ReplyRoute) -> Result<&'a FileSourceConfig> {
    require_files()?;
    validate_identifier(&route.conversation_id)?;
    config.files.sources.get(&route.source_id).ok_or_else(|| {
        AppError::new(
            "config_missing",
            "reply route names an unconfigured file source",
        )
    })
}

/// Read only a stable regular file, bounding actual bytes as well as stat size.
fn read_regular(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|e| AppError::io("stat channel file", &e))?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Err(AppError::new(
            "channel_file_invalid",
            "channel file must be a bounded regular file; symbolic links are refused",
        ));
    }
    let file = File::open(path).map_err(|e| AppError::io("open channel file", &e))?;
    let opened = file
        .metadata()
        .map_err(|e| AppError::io("stat open channel file", &e))?;
    if !opened.is_file() {
        return Err(AppError::new(
            "channel_file_invalid",
            "channel file is not regular",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
            return Err(AppError::new(
                "channel_file_changed",
                "channel file changed while opening",
            ));
        }
    }
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| AppError::io("read channel file", &e))?;
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::new(
            "channel_file_invalid",
            "channel file exceeds the configured size limit",
        ));
    }
    Ok(bytes)
}

/// Publish once using a same-directory hard link. An identical existing payload
/// is success; a different payload is a conflict, never an overwrite.
fn publish_once(path: &Path, bytes: &[u8], max_bytes: u64) -> Result<()> {
    publish_staged(path, bytes, max_bytes).map_err(PublishError::into_error)
}

/// Where a publish failed. Only a failure before the hard link proves that the
/// output was never made visible to the channel's consumer.
enum PublishError {
    BeforeLink(AppError),
    Visible(AppError),
}
impl PublishError {
    fn into_error(self) -> AppError {
        match self {
            Self::BeforeLink(error) | Self::Visible(error) => error,
        }
    }
}
/// Tag an error that happened before any output became visible, the same
/// contract remote senders use so the push executor can safely retry.
fn local_error(error: AppError) -> AppError {
    error.with_details(json!({"remote_calls_started": false}))
}

fn publish_staged(
    path: &Path,
    bytes: &[u8],
    max_bytes: u64,
) -> std::result::Result<(), PublishError> {
    use PublishError::{BeforeLink, Visible};
    if let Ok(existing) = read_regular(path, max_bytes) {
        return if existing == bytes {
            Ok(())
        } else {
            Err(Visible(AppError::new(
                "channel_output_conflict",
                "channel output already contains different content",
            )))
        };
    }
    let parent = path.parent().ok_or_else(|| {
        BeforeLink(AppError::new(
            "channel_path_invalid",
            "output needs a parent directory",
        ))
    })?;
    fs::create_dir_all(parent)
        .map_err(|e| BeforeLink(AppError::io("create channel output directory", &e)))?;
    static TEMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = parent.join(format!(
        ".afmail-{}-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        TEMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let attempt = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|e| BeforeLink(AppError::io("create temporary channel output", &e)))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| BeforeLink(AppError::io("write channel output", &e)))?;
        match fs::hard_link(&temp, path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_regular(path, max_bytes).map_err(Visible)? != bytes {
                    return Err(Visible(AppError::new(
                        "channel_output_conflict",
                        "channel output already contains different content",
                    )));
                }
            }
            Err(e) => return Err(BeforeLink(AppError::io("publish channel output", &e))),
        }
        #[cfg(unix)]
        File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| Visible(AppError::io("sync channel output directory", &e)))?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    attempt
}

pub(crate) fn pull(
    root: &Path,
    config: &MailConfig,
    options: crate::sources::imap::PullOptions,
) -> Result<serde_json::Value> {
    require_files()?;
    let workspace = crate::store::Workspace::at(root);
    let mut remaining_count = options.max_new_messages;
    let mut remaining_bytes = options.max_total_bytes;
    let mut ids = Vec::new();
    let mut merged = 0;
    let mut warnings = Vec::new();
    let mut candidates = 0;
    let mut skipped = Vec::new();
    let conflicts = config.files.path_conflicts(root);
    for (source_id, source) in &config.files.sources {
        // Per-source problems never fail the pull: other sources, and sources
        // already pulled in this run, keep their results.
        let source_warning = |error: &AppError| json!({"code":error.error_code,"message":error.message,"source_id":source_id});
        if let Some(error) = conflicts.get(source_id) {
            warnings.push(source_warning(error));
            continue;
        }
        let inbox = resolve(root, &source.inbox_path);
        let listed = (|| -> Result<Vec<PathBuf>> {
            let unavailable = |e: std::io::Error| {
                AppError::new(
                    "channel_inbox_unavailable",
                    format!("cannot list file inbox: {}", e.kind()),
                )
            };
            let mut paths = Vec::new();
            for entry in fs::read_dir(&inbox).map_err(unavailable)? {
                let path = entry.map_err(unavailable)?.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    paths.push(path);
                }
                if paths.len() > source.max_input_files {
                    return Err(AppError::new(
                        "channel_inbox_limit",
                        "file inbox exceeds max_input_files; retire old producer files before pulling",
                    ));
                }
            }
            Ok(paths)
        })();
        let mut paths = match listed {
            Ok(paths) => paths,
            Err(error) => {
                warnings.push(source_warning(&error));
                continue;
            }
        };
        paths.sort();
        for path in paths {
            candidates += 1;
            let attempt = (|| -> Result<()> {
                let raw = read_regular(&path, source.max_message_bytes)?;
                let input_json = std::str::from_utf8(&raw).map_err(|e| {
                    AppError::new(
                        "channel_message_invalid",
                        format!("input JSON is not UTF-8: {e}"),
                    )
                })?;
                let message = native(input_json)?;
                let key = serde_json::to_vec(&(source_id, &message.external_message_id))
                    .map_err(|e| AppError::json("hash channel identity", &e))?;
                let id = format!("message_file_{}", blake3::hash(&key).to_hex());
                let evidence_path = root.join(".afmail/messages").join(format!("{id}.message"));
                if evidence_path.exists() {
                    let stored = decode(&read_regular(
                        &evidence_path,
                        source
                            .max_message_bytes
                            .saturating_mul(6)
                            .saturating_add(4096),
                    )?)?;
                    if stored.source_id != *source_id || stored.content_json != input_json {
                        return Err(AppError::new(
                            "channel_message_conflict",
                            "external message id already belongs to different input bytes",
                        ));
                    }
                    merged += 1;
                    return Ok(());
                }
                let size_bytes = raw.len() as u64;
                let reason = if remaining_count == 0 {
                    Some("message_count_budget_exceeded")
                } else if size_bytes > remaining_bytes {
                    Some("total_bytes_budget_exceeded")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    // The producer retires inputs from this result, so every
                    // input left unimported is named, not only counted.
                    skipped.push(json!({"source_id":source_id,"input_path":path,"reason":reason,"size_bytes":size_bytes}));
                    return Ok(());
                }
                if options.dry_run {
                    remaining_count -= 1;
                    remaining_bytes -= size_bytes;
                    return Ok(());
                }
                let stored = StoredMessage {
                    source_id: source_id.clone(),
                    direction: "inbound".into(),
                    timestamp_rfc3339: crate::store::now_rfc3339(),
                    content_json: input_json.into(),
                };
                let evidence = serde_json::to_vec(&stored)
                    .map_err(|e| AppError::json("encode channel evidence", &e))?;
                let parsed = materialize(&id, &evidence)?;
                workspace.persist_message_metadata(&parsed)?;
                publish_once(&evidence_path, &evidence, u64::MAX)?;
                // Only durable evidence consumes the budget; a failed write is a
                // warning and leaves the budget to later inputs and adapters.
                remaining_count -= 1;
                remaining_bytes -= size_bytes;
                workspace.write_message_artifacts(&parsed)?;
                ids.push(id);
                Ok(())
            })();
            if let Err(error) = attempt {
                warnings.push(json!({"code":error.error_code,"message":error.message,"source_id":source_id,"input_path":path}));
            }
        }
    }
    if !options.dry_run {
        workspace.render_refresh()?;
    }
    Ok(
        json!({"dry_run":options.dry_run,"new_message_count":ids.len(),"triage_created_count":ids.len(),"new_message_ids":ids,
        "stored_new_message_bytes":if options.dry_run {0} else {options.max_total_bytes-remaining_bytes},"candidate_count":candidates,
        "candidate_declared_bytes":if options.dry_run {options.max_total_bytes-remaining_bytes} else {0},
        "remaining_message_budget":remaining_count,"remaining_bytes_budget":remaining_bytes,"budget_limited":!skipped.is_empty(),
        "files":{"merged_count":merged,"skipped_input_count":skipped.len(),"skipped_inputs":skipped,"warnings":warnings,"capabilities":ChannelKind::File.capabilities()}}),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare(
    root: &Path,
    _case_path: &Path,
    fm: &DraftFrontmatter,
    body: &str,
    config: &MailConfig,
    route: &ReplyRoute,
    message_id: &str,
    date: Option<&str>,
) -> Result<PreparedOutbound> {
    let config_source = source(config, route)?;
    validate_draft(fm)?;
    let identity = config.resolve_identity(root, fm.identity.as_deref())?;
    let reply_to = fm
        .reply_to_message_id
        .as_deref()
        .map(|id| crate::store::Workspace::at(root).read_message_by_id(id))
        .transpose()?
        .and_then(|m| m.origin.map(|o| o.external_message_id));
    let message = NativeMessage {
        schema_name: "channel_message".into(),
        schema_version: 1,
        external_message_id: message_id.into(),
        conversation_id: route.conversation_id.clone(),
        sender_id: identity.identity,
        recipient_ids: fm.to.clone(),
        subject: fm.subject.clone().unwrap_or_default(),
        body_text: crate::draft_content::outbound_body(body)
            .map_err(|e| AppError::new("draft_invalid", e.to_string()))?,
        reply_to_external_message_id: reply_to,
    };
    message.validate()?;
    let content_json = serde_json::to_string_pretty(&message)
        .map_err(|e| AppError::json("encode channel output", &e))?;
    if content_json.len() as u64 > config_source.max_message_bytes {
        return Err(AppError::new(
            "message_too_large",
            "file output exceeds max_message_bytes",
        ));
    }
    let stored = StoredMessage {
        source_id: route.source_id.clone(),
        direction: "outbound".into(),
        timestamp_rfc3339: date
            .map(str::to_owned)
            .unwrap_or_else(crate::store::now_rfc3339),
        content_json,
    };
    let raw =
        serde_json::to_vec(&stored).map_err(|e| AppError::json("encode channel evidence", &e))?;
    Ok(PreparedOutbound {
        message_id: message_id.into(),
        raw,
        delivery: OutboundDelivery::File(route.clone()),
    })
}
pub(crate) fn validate_draft(fm: &DraftFrontmatter) -> Result<()> {
    if !fm.attachments.is_empty() || !fm.cc.is_empty() || !fm.bcc.is_empty() {
        return Err(AppError::new(
            "channel_operation_unsupported",
            "file channels support text and to recipients; attachments, cc and bcc are unavailable",
        ));
    }
    for value in &fm.to {
        validate_identifier(value)?;
    }
    Ok(())
}
pub(crate) fn publish(root: &Path, config: &MailConfig, prepared: &PreparedOutbound) -> Result<()> {
    let OutboundDelivery::File(route) = &prepared.delivery else {
        return Err(local_error(AppError::new(
            "channel_operation_unsupported",
            "file output requires a file route",
        )));
    };
    let config_source = source(config, route).map_err(local_error)?;
    if let Some(error) = config.files.path_conflicts(root).remove(&route.source_id) {
        return Err(local_error(error));
    }
    let stored = decode(&prepared.raw).map_err(local_error)?;
    let path =
        resolve(root, &config_source.outbox_path).join(format!("{}.json", prepared.message_id));
    publish_staged(
        &path,
        stored.content_json.as_bytes(),
        config_source.max_message_bytes,
    )
    .map_err(|error| match error {
        PublishError::BeforeLink(error) => local_error(error),
        PublishError::Visible(error) => error,
    })
}
pub(crate) fn output_confirmed(
    root: &Path,
    config: &MailConfig,
    prepared: &PreparedOutbound,
) -> Result<bool> {
    let OutboundDelivery::File(route) = &prepared.delivery else {
        return Ok(false);
    };
    let config_source = source(config, route)?;
    let stored = decode(&prepared.raw)?;
    let path =
        resolve(root, &config_source.outbox_path).join(format!("{}.json", prepared.message_id));
    if !path
        .try_exists()
        .map_err(|e| AppError::io("check channel output", &e))?
    {
        return Ok(false);
    }
    Ok(read_regular(&path, config_source.max_message_bytes)? == stored.content_json.as_bytes())
}

pub(crate) fn record_sent(
    root: &Path,
    case_path: &Path,
    case_uid: &str,
    prepared: &PreparedOutbound,
) -> Result<()> {
    let mut message = materialize(&prepared.message_id, &prepared.raw)?;
    message.workspace.status = "case".into();
    let workspace = crate::store::Workspace::at(root);
    workspace.persist_message_metadata(&message)?;
    publish_once(
        &root
            .join(".afmail/messages")
            .join(format!("{}.message", prepared.message_id)),
        &prepared.raw,
        u64::MAX,
    )?;
    workspace.write_message_artifacts(&message)?;
    workspace.append_sent_message_to_case(
        case_path,
        case_uid,
        &prepared.message_id,
        message.subject.as_deref(),
        message.sent_rfc3339.as_deref().unwrap_or(""),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit test fixtures")]
mod tests {
    use super::*;

    fn fixture(outbox: &str) -> (tempfile::TempDir, MailConfig, PreparedOutbound) {
        let temp = tempfile::tempdir().unwrap();
        let mut config = MailConfig::default();
        config.files.sources.insert(
            "inquiries".into(),
            FileSourceConfig {
                inbox_path: "input".into(),
                outbox_path: outbox.into(),
                max_message_bytes: default_max_bytes(),
                max_input_files: default_max_files(),
            },
        );
        let stored = StoredMessage {
            source_id: "inquiries".into(),
            direction: "outbound".into(),
            timestamp_rfc3339: "2026-10-24T00:00:00Z".into(),
            content_json: "{\"reply\":true}".into(),
        };
        let prepared = PreparedOutbound {
            message_id: "message_out".into(),
            raw: serde_json::to_vec(&stored).unwrap(),
            delivery: OutboundDelivery::File(ReplyRoute {
                channel: ChannelKind::File,
                source_id: "inquiries".into(),
                conversation_id: "inquiry/1".into(),
            }),
        };
        (temp, config, prepared)
    }
    fn remote_calls_started(error: &AppError) -> Option<bool> {
        error.details.as_ref()?["remote_calls_started"].as_bool()
    }

    #[test]
    fn failures_before_the_link_are_tagged_local() {
        let (temp, config, prepared) = fixture("output");
        // A regular file where the outbox directory belongs fails directory creation.
        fs::write(temp.path().join("output"), "").unwrap();
        let error = publish(temp.path(), &config, &prepared).unwrap_err();
        assert_eq!(remote_calls_started(&error), Some(false), "{error:?}");
        let mut unrouted = prepared.clone();
        unrouted.raw = b"not json".to_vec();
        let error = publish(temp.path(), &config, &unrouted).unwrap_err();
        assert_eq!(remote_calls_started(&error), Some(false), "{error:?}");
    }

    #[test]
    #[cfg(feature = "files")]
    fn different_existing_output_is_an_untagged_conflict() {
        let (temp, config, prepared) = fixture("output");
        fs::create_dir_all(temp.path().join("output")).unwrap();
        fs::write(temp.path().join("output/message_out.json"), "{}").unwrap();
        let error = publish(temp.path(), &config, &prepared).unwrap_err();
        assert_eq!(error.error_code, "channel_output_conflict");
        assert_eq!(remote_calls_started(&error), None);
        fs::write(
            temp.path().join("output/message_out.json"),
            "{\"reply\":true}",
        )
        .unwrap();
        publish(temp.path(), &config, &prepared).unwrap();
    }

    #[test]
    #[cfg(feature = "files")]
    fn overlapping_outbox_is_rejected_before_any_write() {
        let (temp, config, prepared) = fixture(".afmail/../messages/out");
        let error = publish(temp.path(), &config, &prepared).unwrap_err();
        assert_eq!(error.error_code, "config_invalid");
        assert_eq!(remote_calls_started(&error), Some(false));
        assert!(!temp.path().join("messages").exists());
        assert!(config.files.validate().is_err());
    }
}
