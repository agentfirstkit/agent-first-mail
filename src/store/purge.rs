use super::*;

const PURGE_PLAN_SCHEMA_NAME: &str = "purge_plan";
const PURGE_PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PurgeDisposition {
    Spam,
    Trash,
    Deleted,
}

impl PurgeDisposition {
    fn status(self) -> MessageStatus {
        match self {
            Self::Spam => MessageStatus::Spam,
            Self::Trash => MessageStatus::Trashed,
            Self::Deleted => MessageStatus::DeletedRemote,
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::Trash => "trash",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Serialize)]
struct PurgeArtifact {
    path: String,
    size_bytes: u64,
    content_hash: String,
}

#[derive(Debug, Serialize)]
struct PurgeCandidate {
    message_id: String,
    disposition: PurgeDisposition,
    age_basis_rfc3339: String,
    size_bytes: u64,
    artifacts: Vec<PurgeArtifact>,
}

#[derive(Debug, Serialize)]
struct PurgeSkipped {
    message_id: String,
    disposition: PurgeDisposition,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
struct PurgeDigest<'a> {
    schema_name: &'static str,
    schema_version: u32,
    workspace_uid: &'a str,
    targets: Vec<&'static str>,
    older_than_days: u64,
    candidates: &'a [PurgeCandidate],
    skipped: &'a [PurgeSkipped],
}

struct PurgePlan {
    workspace_uid: String,
    workspace_path: String,
    plan_id: String,
    target: &'static str,
    targets: Vec<PurgeDisposition>,
    older_than_days: u64,
    candidates: Vec<PurgeCandidate>,
    skipped: Vec<PurgeSkipped>,
}

impl Workspace {
    pub fn purge_spam(&self, days: u64, confirm: Option<&str>) -> Result<Value> {
        self.purge_dispositions(&[PurgeDisposition::Spam], days, confirm)
    }

    pub fn purge_trash(&self, days: u64, confirm: Option<&str>) -> Result<Value> {
        self.purge_dispositions(&[PurgeDisposition::Trash], days, confirm)
    }

    pub fn purge_deleted(&self, days: u64, confirm: Option<&str>) -> Result<Value> {
        self.purge_dispositions(&[PurgeDisposition::Deleted], days, confirm)
    }

    pub fn purge_discards(&self, days: u64, confirm: Option<&str>) -> Result<Value> {
        self.purge_dispositions(
            &[
                PurgeDisposition::Spam,
                PurgeDisposition::Trash,
                PurgeDisposition::Deleted,
            ],
            days,
            confirm,
        )
    }

    fn purge_dispositions(
        &self,
        targets: &[PurgeDisposition],
        older_than_days: u64,
        confirm: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let plan = self.build_purge_plan(targets, older_than_days)?;
        let Some(confirmed_plan_id) = confirm else {
            return Ok(purge_plan_value(&plan, false));
        };
        if confirmed_plan_id != plan.plan_id {
            return Err(AppError::new(
                "purge_plan_changed",
                "purge state no longer matches the reviewed plan",
            )
            .with_details(json!({
                "confirmed_plan_id": confirmed_plan_id,
                "current_plan_id": plan.plan_id,
                "local_deletions_started": false,
            })));
        }

        // Validate the complete reviewed snapshot before the first destructive
        // operation so a stale plan cannot partially apply.
        for candidate in &plan.candidates {
            verify_purge_artifacts(&self.root, &candidate.artifacts)?;
        }
        let mut purged = Vec::new();
        let mut spam_count = 0;
        let mut trash_count = 0;
        let mut deleted_count = 0;
        for candidate in &plan.candidates {
            purge_message_artifacts(&self.root, &candidate.message_id)?;
            self.clear_message_disposition(candidate.disposition.status(), &candidate.message_id)?;
            match candidate.disposition {
                PurgeDisposition::Spam => spam_count += 1,
                PurgeDisposition::Trash => trash_count += 1,
                PurgeDisposition::Deleted => deleted_count += 1,
            }
            purged.push(candidate.message_id.clone());
        }
        let dispositions = self.refresh_disposition_views()?;
        let mut result = json!({
            "confirmed": true,
            "plan_id": plan.plan_id,
            "workspace_uid": plan.workspace_uid,
            "workspace_path": plan.workspace_path,
            "target": plan.target,
            "targets": plan.targets.iter().map(|target| target.target()).collect::<Vec<_>>(),
            "older_than_days": plan.older_than_days,
            "planned_count": plan.candidates.len(),
            "purged_count": purged.len(),
            "purged_message_ids": purged,
            "purged_spam_count": spam_count,
            "purged_trash_count": trash_count,
            "purged_deleted_count": deleted_count,
            "skipped_count": plan.skipped.len(),
            "skipped": plan.skipped,
        });
        merge_disposition_refresh_into_purge(&mut result, &dispositions);
        Ok(result)
    }

    fn build_purge_plan(&self, targets: &[PurgeDisposition], days: u64) -> Result<PurgePlan> {
        let identity = crate::workspace_identity::load_workspace_identity(&self.root)?;
        let workspace_path = fs::canonicalize(&self.root)
            .map_err(|error| AppError::io("canonicalize purge workspace", &error))?;
        let cutoff = Utc::now() - Duration::days(days as i64);
        let by_status = targets
            .iter()
            .copied()
            .map(|target| (target.status(), target))
            .collect::<BTreeMap<_, _>>();
        let mut candidates = Vec::new();
        let mut skipped = Vec::new();
        for path in message_json_paths(&self.root)? {
            let Some(message_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let message = self.read_message_by_id(message_id)?;
            let status = MessageStatus::parse(&message.workspace.status)?;
            let Some(disposition) = by_status.get(&status).copied() else {
                continue;
            };
            let age_basis = self.message_purge_age_time(&message)?;
            if age_basis > cutoff {
                skipped.push(PurgeSkipped {
                    message_id: message.message_id,
                    disposition,
                    reason: "recent",
                });
                continue;
            }
            if self.message_id_is_referenced(&message.message_id)? {
                skipped.push(PurgeSkipped {
                    message_id: message.message_id,
                    disposition,
                    reason: "referenced",
                });
                continue;
            }
            let artifacts = purge_artifacts(&self.root, &message.message_id)?;
            let size_bytes = artifacts.iter().map(|artifact| artifact.size_bytes).sum();
            candidates.push(PurgeCandidate {
                message_id: message.message_id,
                disposition,
                age_basis_rfc3339: age_basis.to_rfc3339_opts(SecondsFormat::Secs, true),
                size_bytes,
                artifacts,
            });
        }
        candidates.sort_by(|left, right| left.message_id.cmp(&right.message_id));
        skipped.sort_by(|left, right| left.message_id.cmp(&right.message_id));
        let target_names = targets
            .iter()
            .map(|target| target.target())
            .collect::<Vec<_>>();
        let digest = PurgeDigest {
            schema_name: PURGE_PLAN_SCHEMA_NAME,
            schema_version: PURGE_PLAN_SCHEMA_VERSION,
            workspace_uid: &identity.workspace_uid,
            targets: target_names,
            older_than_days: days,
            candidates: &candidates,
            skipped: &skipped,
        };
        let hash = blake3_json_fingerprint(&digest)?;
        Ok(PurgePlan {
            workspace_uid: identity.workspace_uid,
            workspace_path: path_to_string(&workspace_path),
            plan_id: format!("purge_plan_{hash}"),
            target: purge_target_name(targets),
            targets: targets.to_vec(),
            older_than_days: days,
            candidates,
            skipped,
        })
    }

    fn message_purge_age_time(&self, message: &MessageFile) -> Result<DateTime<Utc>> {
        let status = MessageStatus::parse(&message.workspace.status)?;
        let value = self
            .disposition_state_for_message(&message.message_id)?
            .and_then(|(found, added)| (found == status).then_some(added))
            .or_else(|| message.received_rfc3339.clone())
            .or_else(|| message.sent_rfc3339.clone())
            .ok_or_else(|| {
                AppError::new(
                    "purge_age_unknown",
                    format!("message {} has no purge age", message.message_id),
                )
            })?;
        DateTime::parse_from_rfc3339(&value)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|error| {
                AppError::new(
                    "purge_age_invalid",
                    format!(
                        "message {} has invalid purge age {value:?}: {error}",
                        message.message_id
                    ),
                )
            })
    }
}

fn purge_plan_value(plan: &PurgePlan, confirmed: bool) -> Value {
    json!({
        "schema_name": PURGE_PLAN_SCHEMA_NAME,
        "schema_version": PURGE_PLAN_SCHEMA_VERSION,
        "confirmed": confirmed,
        "workspace_uid": plan.workspace_uid,
        "workspace_path": plan.workspace_path,
        "plan_id": plan.plan_id,
        "target": plan.target,
        "targets": plan.targets.iter().map(|target| target.target()).collect::<Vec<_>>(),
        "older_than_days": plan.older_than_days,
        "candidate_count": plan.candidates.len(),
        "candidate_size_bytes": plan.candidates.iter().map(|item| item.size_bytes).sum::<u64>(),
        "candidates": plan.candidates,
        "skipped_count": plan.skipped.len(),
        "skipped": plan.skipped,
        "hint": format!("Review this plan, then run purge with --confirm {}", plan.plan_id),
    })
}

fn purge_artifacts(root: &Path, message_id: &str) -> Result<Vec<PurgeArtifact>> {
    let base = root.join(".afmail/messages");
    let mut paths = vec![
        root.join(layout::MESSAGES)
            .join(format!("{message_id}.json")),
        base.join(format!("{message_id}.json")),
        base.join(format!("{message_id}.eml")),
        base.join(format!("{message_id}.state.json")),
        base.join(format!("{message_id}.txt")),
    ];
    collect_files(&base.join(format!("{message_id}.files")), &mut paths)?;
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| path.is_file())
        .map(|path| {
            let bytes =
                fs::read(&path).map_err(|error| AppError::io("read purge artifact", &error))?;
            Ok(PurgeArtifact {
                path: rel_path(root, &path),
                size_bytes: bytes.len() as u64,
                content_hash: blake3_fingerprint(&bytes),
            })
        })
        .collect()
}

fn collect_files(path: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| AppError::io("inspect purge artifact", &error))?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::new(
            "purge_artifact_unsafe",
            format!("refusing symlink {}", path_to_string(path)),
        ));
    }
    if metadata.is_file() {
        paths.push(path.to_path_buf());
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in read_dir(path, "read purge artifact directory")? {
            collect_files(&entry.path(), paths)?;
        }
        return Ok(());
    }
    Err(AppError::new(
        "purge_artifact_unsafe",
        format!("refusing special file {}", path_to_string(path)),
    ))
}

fn verify_purge_artifacts(root: &Path, artifacts: &[PurgeArtifact]) -> Result<()> {
    for artifact in artifacts {
        let bytes = fs::read(root.join(&artifact.path))
            .map_err(|error| AppError::io("verify purge artifact", &error))?;
        if bytes.len() as u64 != artifact.size_bytes
            || blake3_fingerprint(&bytes) != artifact.content_hash
        {
            return Err(AppError::new(
                "purge_plan_changed",
                format!("purge artifact changed: {}", artifact.path),
            )
            .with_details(json!({ "local_deletions_started": false, "path": artifact.path })));
        }
    }
    Ok(())
}

fn purge_target_name(targets: &[PurgeDisposition]) -> &'static str {
    match targets {
        [PurgeDisposition::Spam] => "spam",
        [PurgeDisposition::Trash] => "trash",
        [PurgeDisposition::Deleted] => "deleted",
        _ => "discards",
    }
}

fn merge_disposition_refresh_into_purge(purge: &mut Value, dispositions: &Value) {
    let (Some(purge), Some(dispositions)) = (purge.as_object_mut(), dispositions.as_object())
    else {
        return;
    };
    for key in [
        "spam_count",
        "spam_written_count",
        "stale_spam_removed_count",
        "trash_count",
        "trash_written_count",
        "stale_trash_removed_count",
        "deleted_count",
        "deleted_written_count",
        "stale_deleted_removed_count",
    ] {
        if let Some(value) = dispositions.get(key) {
            purge.insert(key.to_string(), value.clone());
        }
    }
}
