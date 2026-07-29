use super::*;

const LEGACY_MESSAGE_COLLECTION_SCHEMA_VERSION: u64 = 1;

#[derive(Debug)]
struct CollectionMigration {
    path: PathBuf,
    target: MessageCollection,
    rewrite_required: bool,
}

#[derive(Debug)]
struct ParsedCollectionMigration {
    collection: MessageCollection,
    retired_projection_fields_present: bool,
}

#[derive(Debug)]
struct MessageMetadataMigration {
    path: PathBuf,
    target: MessageMetadata,
    rewrite_required: bool,
}

#[derive(Debug)]
pub(super) struct WorkspaceMigration {
    message_metadata: Vec<MessageMetadataMigration>,
    legacy_message_paths: Vec<PathBuf>,
    legacy_flat_paths: Vec<PathBuf>,
    case_files: Vec<CollectionMigration>,
    notification_files: Vec<CollectionMigration>,
    disposition_files: Vec<CollectionMigration>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WorkspaceMigrationCounts {
    pub(super) message_metadata_migrated_count: usize,
    pub(super) case_migrated_count: usize,
    pub(super) notification_migrated_count: usize,
    pub(super) disposition_migrated_count: usize,
}

impl WorkspaceMigration {
    pub(super) fn has_legacy_inputs(&self) -> bool {
        !self.legacy_message_paths.is_empty()
            || !self.legacy_flat_paths.is_empty()
            || self
                .message_metadata
                .iter()
                .any(|value| value.rewrite_required)
            || self.case_files.iter().any(|value| value.rewrite_required)
            || self
                .notification_files
                .iter()
                .any(|value| value.rewrite_required)
            || self
                .disposition_files
                .iter()
                .any(|value| value.rewrite_required)
    }

    pub(super) fn preflight(root: &Path) -> Result<Self> {
        let message_dir = root.join(".afmail/messages");
        let mut message_ids = BTreeSet::new();
        let mut legacy_message_paths = Vec::new();
        if message_dir.is_dir() {
            for entry in read_dir(&message_dir, "preflight message metadata migration")? {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if let Some(message_id) = name.strip_suffix(".remote.json") {
                    validate_id("message_id", message_id)?;
                    read_legacy_message_remote_file(root, message_id)?.ok_or_else(|| {
                        AppError::new(
                            "workspace_upgrade_invalid",
                            format!(
                                "legacy message metadata disappeared: {}",
                                rel_path(root, &path)
                            ),
                        )
                    })?;
                    if !message_dir.join(format!("{message_id}.eml")).is_file() {
                        return Err(AppError::new(
                            "workspace_upgrade_invalid",
                            format!("legacy message metadata has no EML evidence: {message_id}"),
                        ));
                    }
                    message_ids.insert(message_id.to_string());
                    legacy_message_paths.push(path);
                } else if let Some(message_id) = name.strip_suffix(".eml") {
                    validate_id("message_id", message_id)?;
                    message_ids.insert(message_id.to_string());
                }
            }
        }

        let mut message_metadata = Vec::new();
        for message_id in message_ids {
            let metadata_path = message_dir.join(format!("{message_id}.json"));
            let (metadata, rewrite_required) = if let Some((metadata, rewrite_required)) =
                read_message_metadata_for_migration(root, &message_id)?
            {
                if let Some(legacy) = read_legacy_message_remote_file(root, &message_id)?
                    && metadata.remote.as_ref() != Some(&legacy.remote_state())
                {
                    return Err(AppError::new(
                        "workspace_upgrade_conflict",
                        format!(
                            "legacy and current message metadata differ for {message_id}"
                        ),
                    )
                    .with_details(json!({
                        "legacy_path": rel_path(root, &legacy_message_remote_path(root, &message_id)),
                        "current_path": rel_path(root, &metadata_path),
                    })));
                }
                (metadata, rewrite_required)
            } else {
                let remote = read_legacy_message_remote_file(root, &message_id)?
                    .map(|legacy| legacy.remote_state());
                (
                    MessageMetadata {
                        schema_name: "message_metadata".to_string(),
                        schema_version: 2,
                        message_id,
                        remote,
                        agent_suggestion: None,
                        human_suggestion: None,
                        case_relationship_suggestion: None,
                    },
                    true,
                )
            };
            message_metadata.push(MessageMetadataMigration {
                path: metadata_path,
                target: metadata,
                rewrite_required,
            });
        }

        let case_files = preflight_collections(
            root,
            &[layout::CASES, layout::ARCHIVED_CASES],
            "case.json",
            CASE_SCHEMA_NAME,
        )?;
        let notification_files = preflight_collections(
            root,
            &[layout::NOTIFICATIONS],
            "notification.json",
            NOTIFICATION_SCHEMA_NAME,
        )?;
        let disposition_files = preflight_disposition_collections(root)?;
        let legacy_flat_paths = preflight_flat_layout(root)?;
        Ok(Self {
            message_metadata,
            legacy_message_paths,
            legacy_flat_paths,
            case_files,
            notification_files,
            disposition_files,
        })
    }

    pub(super) fn apply(&self) -> Result<WorkspaceMigrationCounts> {
        let mut counts = WorkspaceMigrationCounts::default();
        for migration in &self.message_metadata {
            if !migration.rewrite_required {
                continue;
            }
            write_message_metadata_file(
                migration
                    .path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                    .ok_or_else(|| {
                        AppError::new("workspace_upgrade_invalid", "invalid message metadata path")
                    })?,
                &migration.target,
            )?;
            counts.message_metadata_migrated_count += 1;
        }
        for migration in &self.case_files {
            if !migration.rewrite_required {
                continue;
            }
            write_json_pretty(&migration.path, &migration.target)?;
            counts.case_migrated_count += 1;
        }
        for migration in &self.notification_files {
            if !migration.rewrite_required {
                continue;
            }
            write_json_pretty(&migration.path, &migration.target)?;
            counts.notification_migrated_count += 1;
        }
        for migration in &self.disposition_files {
            if !migration.rewrite_required {
                continue;
            }
            write_json_pretty(&migration.path, &migration.target)?;
            counts.disposition_migrated_count += 1;
        }
        let legacy_pull_cursors = self.legacy_flat_paths.iter().find(|path| {
            path.file_name().and_then(|value| value.to_str()) == Some("pull-cursors.json")
        });
        if let Some(source) = legacy_pull_cursors {
            let destination = source
                .parent()
                .ok_or_else(|| {
                    AppError::new(
                        "workspace_upgrade_invalid",
                        "invalid legacy pull cursor path",
                    )
                })?
                .join(layout::AFMAIL_PULL_CURSORS);
            if !destination.exists() {
                let data = read_to_string(source, "read legacy pull cursors")?;
                write_string_atomic(&destination, &data)?;
            }
        }
        Ok(counts)
    }

    pub(super) fn remove_legacy_message_files(&self) -> Result<()> {
        for path in &self.legacy_message_paths {
            if path.exists() {
                remove_file(path)?;
            }
        }
        for path in &self.legacy_flat_paths {
            if path.exists() {
                remove_file(path)?;
            }
        }
        Ok(())
    }
}

fn read_message_metadata_for_migration(
    root: &Path,
    message_id: &str,
) -> Result<Option<(MessageMetadata, bool)>> {
    let path = message_metadata_path(root, message_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = read_to_string(&path, "preflight message metadata migration")?;
    let mut metadata: MessageMetadata = serde_json::from_str(&data).map_err(|error| {
        AppError::json("parse message metadata for migration", &error).with_details(json!({
            "path": rel_path(root, &path),
        }))
    })?;
    if metadata.schema_name != "message_metadata"
        || !matches!(metadata.schema_version, 1 | 2)
        || metadata.message_id != message_id
    {
        return Err(AppError::new(
            "workspace_upgrade_invalid",
            format!(
                "unsupported message metadata schema: {}",
                rel_path(root, &path)
            ),
        ));
    }
    super::suggestions::validate_message_suggestion_pair(
        metadata.agent_suggestion.as_ref(),
        metadata.human_suggestion.as_ref(),
    )?;
    validate_case_relationship_suggestion(metadata.case_relationship_suggestion.as_ref())?;
    let rewrite_required = metadata.schema_version != 2;
    metadata.schema_version = 2;
    Ok(Some((metadata, rewrite_required)))
}

fn preflight_flat_layout(root: &Path) -> Result<Vec<PathBuf>> {
    let metadata_root = root.join(layout::AFMAIL);
    let mut paths = Vec::new();
    for name in layout::LEGACY_AFMAIL_FILES {
        let path = metadata_root.join(name);
        if path.exists() {
            if !path.is_file() {
                return Err(AppError::new(
                    "workspace_upgrade_invalid",
                    format!(
                        "legacy workspace path is not a file: {}",
                        rel_path(root, &path)
                    ),
                ));
            }
            paths.push(path);
        }
    }
    if let Some(source) = paths
        .iter()
        .find(|path| path.file_name().and_then(|value| value.to_str()) == Some("pull-cursors.json"))
    {
        let data = read_to_string(source, "preflight legacy pull cursors")?;
        serde_json::from_str::<Value>(&data)
            .map_err(|error| AppError::json("parse legacy pull cursors for migration", &error))?;
        let destination = metadata_root.join(layout::AFMAIL_PULL_CURSORS);
        if destination.exists() {
            let destination_data = read_to_string(&destination, "preflight pull cursors")?;
            if destination_data != data {
                return Err(AppError::new(
                    "workspace_upgrade_conflict",
                    "legacy and current pull cursor files differ",
                )
                .with_details(json!({
                    "legacy_path": rel_path(root, source),
                    "current_path": rel_path(root, &destination),
                })));
            }
        }
    }
    Ok(paths)
}

fn preflight_collections(
    root: &Path,
    top_levels: &[&str],
    filename: &str,
    schema_name: &str,
) -> Result<Vec<CollectionMigration>> {
    let mut out = Vec::new();
    for top_level in top_levels {
        let base = root.join(top_level);
        if !base.is_dir() {
            continue;
        }
        collect_named_files(&base, filename, &mut |path| {
            let parsed = parse_collection_for_migration(root, path)?;
            let mut collection = parsed.collection;
            if collection.schema_name != schema_name
                || !matches!(
                    collection.schema_version,
                    LEGACY_MESSAGE_COLLECTION_SCHEMA_VERSION
                        | MESSAGE_COLLECTION_SCHEMA_VERSION
                        | SUGGESTIBLE_COLLECTION_SCHEMA_VERSION
                )
            {
                return Err(AppError::new(
                    "workspace_upgrade_invalid",
                    format!("unsupported collection schema: {}", rel_path(root, path)),
                ));
            }
            if collection.schema_version == LEGACY_MESSAGE_COLLECTION_SCHEMA_VERSION
                && (collection.agent_suggestion.is_some() || collection.human_suggestion.is_some())
            {
                return Err(AppError::new(
                    "workspace_upgrade_invalid",
                    format!(
                        "legacy collection unexpectedly contains suggestion fields: {}",
                        rel_path(root, path)
                    ),
                ));
            }
            let mut rewrite_required = parsed.retired_projection_fields_present
                || collection.schema_version != SUGGESTIBLE_COLLECTION_SCHEMA_VERSION;
            collection.schema_version = SUGGESTIBLE_COLLECTION_SCHEMA_VERSION;
            if schema_name == CASE_SCHEMA_NAME {
                if collection.status.is_empty() && collection.archived_rfc3339.is_some() {
                    collection.status = "archived".to_string();
                    rewrite_required = true;
                }
                let case_path = path.parent().and_then(Path::parent).ok_or_else(|| {
                    AppError::new(
                        "workspace_upgrade_invalid",
                        format!("invalid case metadata path: {}", rel_path(root, path)),
                    )
                })?;
                validate_case_collection(&collection, case_path)?;
                validate_case_membership(&collection, case_path)?;
                validate_case_status(&collection, case_path)?;
                super::suggestions::validate_case_suggestion_pair(
                    collection.agent_suggestion.as_ref(),
                    collection.human_suggestion.as_ref(),
                )?;
            } else {
                let notification_path = path.parent().and_then(Path::parent).ok_or_else(|| {
                    AppError::new(
                        "workspace_upgrade_invalid",
                        format!(
                            "invalid notification metadata path: {}",
                            rel_path(root, path)
                        ),
                    )
                })?;
                let notification_ref = notification_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| {
                        AppError::new(
                            "workspace_upgrade_invalid",
                            format!(
                                "invalid notification directory name: {}",
                                rel_path(root, notification_path)
                            ),
                        )
                    })?;
                let notification_uid = parse_notification_ref(notification_ref)?;
                super::notification::validate_notification_collection(
                    root,
                    path,
                    &notification_uid,
                    None,
                    &collection,
                )?;
                super::suggestions::validate_notification_suggestion_pair(
                    collection.agent_suggestion.as_ref(),
                    collection.human_suggestion.as_ref(),
                )?;
            }
            out.push(CollectionMigration {
                path: path.to_path_buf(),
                target: collection,
                rewrite_required,
            });
            Ok(())
        })?;
    }
    out.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(out)
}

fn preflight_disposition_collections(root: &Path) -> Result<Vec<CollectionMigration>> {
    let mut out = Vec::new();
    for spec in super::disposition_views::DISPOSITION_VIEW_SPECS {
        let path = root.join(spec.dir).join("data").join(spec.data_file);
        if !path.exists() {
            continue;
        }
        let parsed = parse_collection_for_migration(root, &path)?;
        let mut collection = parsed.collection;
        if !matches!(
            collection.schema_version,
            LEGACY_MESSAGE_COLLECTION_SCHEMA_VERSION | MESSAGE_COLLECTION_SCHEMA_VERSION
        ) {
            return Err(AppError::new(
                "workspace_upgrade_invalid",
                format!(
                    "unsupported disposition collection schema: {}",
                    rel_path(root, &path)
                ),
            ));
        }
        let legacy_collection_name = match spec.schema_name {
            "spam" => "Spam",
            "trash" => "Trash",
            "deleted_remote" => "Remote Deleted",
            _ => {
                return Err(AppError::new(
                    "workspace_upgrade_invalid",
                    format!("unsupported disposition schema: {}", spec.schema_name),
                ));
            }
        };
        if collection.collection_name != spec.collection_uid
            && collection.collection_name != legacy_collection_name
        {
            return Err(AppError::new(
                "workspace_upgrade_invalid",
                format!(
                    "unsupported disposition collection name: {}",
                    rel_path(root, &path)
                ),
            ));
        }
        let rewrite_required = parsed.retired_projection_fields_present
            || collection.schema_version != MESSAGE_COLLECTION_SCHEMA_VERSION
            || collection.collection_name != spec.collection_uid;
        collection.schema_version = MESSAGE_COLLECTION_SCHEMA_VERSION;
        collection.collection_name = spec.collection_uid.to_string();
        super::disposition_views::validate_disposition_collection(root, &path, spec, &collection)?;
        out.push(CollectionMigration {
            path,
            target: collection,
            rewrite_required,
        });
    }
    out.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(out)
}

fn parse_collection_for_migration(root: &Path, path: &Path) -> Result<ParsedCollectionMigration> {
    // Compatibility is intentionally isolated to `afmail init`: remove only
    // the retired derived projections, then deserialize the remaining object
    // through the current strict canonical type. Unknown fields still fail.
    let data = read_to_string(path, "preflight collection migration")?;
    let mut value: Value = serde_json::from_str(&data)
        .map_err(|error| AppError::json("parse collection metadata for migration", &error))?;
    let object = value.as_object_mut().ok_or_else(|| {
        AppError::new(
            "workspace_upgrade_invalid",
            format!(
                "collection metadata must be a JSON object: {}",
                rel_path(root, path)
            ),
        )
    })?;
    let message_count =
        take_retired_collection_field::<usize>(root, path, object, "message_count")?;
    let thread_count = take_retired_collection_field::<usize>(root, path, object, "thread_count")?;
    let attachment_count =
        take_retired_collection_field::<usize>(root, path, object, "attachment_count")?;
    let last_message_rfc3339 = take_retired_collection_field::<Option<String>>(
        root,
        path,
        object,
        "last_message_rfc3339",
    )?;
    let retired_projection_fields_present = message_count.is_some()
        || thread_count.is_some()
        || attachment_count.is_some()
        || last_message_rfc3339.is_some();
    let collection: MessageCollection = serde_json::from_value(value).map_err(|error| {
        AppError::json("parse collection metadata for migration", &error).with_details(json!({
            "path": rel_path(root, path),
        }))
    })?;
    if let Some(message_count) = message_count
        && message_count != collection.items.len()
    {
        return Err(AppError::new(
            "workspace_upgrade_invalid",
            format!(
                "legacy message_count conflicts with canonical items: {}",
                rel_path(root, path)
            ),
        )
        .with_details(json!({
            "path": rel_path(root, path),
            "message_count": message_count,
            "item_count": collection.items.len(),
        })));
    }
    Ok(ParsedCollectionMigration {
        collection,
        retired_projection_fields_present,
    })
}

fn take_retired_collection_field<T: serde::de::DeserializeOwned>(
    root: &Path,
    path: &Path,
    object: &mut serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<T>> {
    let Some(value) = object.remove(field) else {
        return Ok(None);
    };
    serde_json::from_value(value).map(Some).map_err(|error| {
        AppError::json("parse retired collection projection for migration", &error).with_details(
            json!({
                "path": rel_path(root, path),
                "field": field,
            }),
        )
    })
}

fn collect_named_files(
    dir: &Path,
    filename: &str,
    visit: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    for entry in read_dir(dir, "walk workspace migration inputs")? {
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, filename, visit)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(filename) {
            visit(&path)?;
        }
    }
    Ok(())
}
