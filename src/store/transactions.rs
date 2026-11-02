use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TransactionFile {
    pub(super) schema_name: String,
    pub(super) schema_version: u64,
    pub(super) transaction_id: String,
    pub(super) kind: String,
    pub(super) created_rfc3339: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) paths: Vec<String>,
    #[serde(skip)]
    pub(super) path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct LocalTransaction {
    path: PathBuf,
}

enum TransactionRepair {
    CaseArchive(String),
    AbortedDiscardUndo,
}

impl Workspace {
    pub(crate) fn begin_transaction(
        &self,
        kind: &str,
        paths: impl IntoIterator<Item = String>,
    ) -> Result<LocalTransaction> {
        validate_id("transaction kind", kind)?;
        let transactions_dir = self.root.join(crate::layout::afmail_path(
            crate::layout::AFMAIL_TRANSACTIONS,
        ));
        create_dir_all(&transactions_dir)?;
        let transaction_id = unique_transaction_id(kind);
        let path = transactions_dir.join(format!("{transaction_id}.json"));
        let transaction = TransactionFile {
            schema_name: "local_transaction".to_string(),
            schema_version: 1,
            transaction_id,
            kind: kind.to_string(),
            created_rfc3339: now_rfc3339(),
            paths: paths.into_iter().collect(),
            path: path.clone(),
        };
        write_json_pretty(&path, &transaction)?;
        Ok(LocalTransaction { path })
    }

    pub fn ensure_no_incomplete_transactions(&self) -> Result<()> {
        let transactions = incomplete_transaction_paths(&self.root)?;
        if transactions.is_empty() {
            return Ok(());
        }
        Err(AppError::new(
            "transaction_incomplete",
            format!(
                "incomplete afmail transaction(s) detected: {}; run `afmail doctor` for details",
                transactions.join(", ")
            ),
        )
        .with_hint("Run `afmail doctor` to inspect incomplete transactions; use `afmail doctor repair --confirm` only after reviewing the issue.")
        .with_details(json!({
            "transaction_paths": transactions,
            "suggested_commands": [
                "afmail doctor",
                "afmail doctor repair --confirm"
            ]
        })))
    }

    pub(super) fn incomplete_transactions(&self) -> Result<Vec<TransactionFile>> {
        let dir = self.root.join(crate::layout::afmail_path(
            crate::layout::AFMAIL_TRANSACTIONS,
        ));
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in read_dir(&dir, "read transactions")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let data = read_to_string(&path, "read transaction")?;
            let mut transaction: TransactionFile =
                serde_json::from_str(&data).map_err(|e| AppError::json("parse transaction", &e))?;
            if transaction.schema_name != "local_transaction" || transaction.schema_version != 1 {
                return Err(AppError::new(
                    "transaction_invalid",
                    format!(
                        "invalid local transaction schema: {}",
                        rel_path(&self.root, &path)
                    ),
                ));
            }
            transaction.path = path;
            out.push(transaction);
        }
        out.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id));
        Ok(out)
    }

    pub(super) fn repair_incomplete_transactions(&self) -> Result<()> {
        let transactions = self.incomplete_transactions()?;
        let mut recoveries = Vec::new();
        for transaction in &transactions {
            let Some(repair) = transaction.repair_action(self)? else {
                return Err(AppError::new(
                    "transaction_incomplete",
                    format!(
                        "transaction {} ({}) cannot be repaired automatically",
                        transaction.transaction_id, transaction.kind
                    ),
                )
                .with_hint(
                    "Inspect the transaction paths and restore a consistent workspace state before retrying.",
                ));
            };
            recoveries.push((transaction, repair));
        }
        for (transaction, repair) in recoveries {
            match repair {
                TransactionRepair::CaseArchive(case_uid) => {
                    self.recover_case_archive_transaction(&case_uid)?;
                }
                TransactionRepair::AbortedDiscardUndo => {}
            }
            if transaction.path.exists() {
                remove_file(&transaction.path)?;
            }
        }
        Ok(())
    }
}

impl TransactionFile {
    pub(super) fn is_repairable(&self, workspace: &Workspace) -> Result<bool> {
        Ok(self.repair_action(workspace)?.is_some())
    }

    fn repair_action(&self, workspace: &Workspace) -> Result<Option<TransactionRepair>> {
        if let Some(case_uid) = self.repairable_case_archive_uid() {
            return Ok(Some(TransactionRepair::CaseArchive(case_uid)));
        }
        let (status, action) = match self.kind.as_str() {
            "message_unspammed" => (MessageStatus::Spam, MessagePushAction::Spam),
            "message_untrashed" => (MessageStatus::Trashed, MessagePushAction::Trash),
            _ => return Ok(None),
        };
        if self.paths.len() != 2 || !self.paths.iter().any(|path| path == ".afmail/push") {
            return Ok(None);
        }
        let Some(message_id) = self
            .paths
            .iter()
            .find_map(|path| path.strip_circumfix("messages/", ".json"))
        else {
            return Ok(None);
        };
        if validate_id("message_id", message_id).is_err() {
            return Ok(None);
        }
        let message = workspace.read_message_by_id(message_id)?;
        if message.workspace.status != status.as_str() {
            return Ok(None);
        }
        let pushes = crate::push_queue::message_push_items(&workspace.root, message_id)?;
        // Undo removes the original queue item before making any local changes.
        // A single started original item proves that the legacy transaction
        // stopped at that guard and never changed the disposition or queue.
        let blocked_before_writes = pushes.len() == 1
            && pushes[0].has_started_effects()
            && pushes[0]
                .message()
                .is_some_and(|payload| payload.action == action);
        Ok(blocked_before_writes.then_some(TransactionRepair::AbortedDiscardUndo))
    }

    fn repairable_case_archive_uid(&self) -> Option<String> {
        if self.kind != "case_archive" {
            return None;
        }
        let case_uid_under = |root: &str| {
            self.paths.iter().find_map(|path| {
                let relative = Path::new(path).strip_prefix(root).ok()?;
                if relative.components().count() != 2 {
                    return None;
                }
                relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| parse_case_ref(name).ok())
            })
        };
        let active_case_uid = case_uid_under(layout::CASES)?;
        let archived_case_uid = case_uid_under(layout::ARCHIVED_CASES)?;
        let push_path = layout::afmail_path(layout::AFMAIL_PUSH);
        let includes_push = self.paths.iter().any(|path| Path::new(path) == push_path);
        (active_case_uid == archived_case_uid && includes_push).then_some(active_case_uid)
    }
}

impl LocalTransaction {
    pub(crate) fn commit(self) -> Result<()> {
        if self.path.exists() {
            remove_file(&self.path)?;
        }
        Ok(())
    }
}

fn incomplete_transaction_paths(root: &Path) -> Result<Vec<String>> {
    let dir = root.join(crate::layout::afmail_path(
        crate::layout::AFMAIL_TRANSACTIONS,
    ));
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in read_dir(&dir, "read transactions")? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            out.push(rel_path(root, &path));
        }
    }
    out.sort();
    Ok(out)
}

fn unique_transaction_id(kind: &str) -> String {
    let safe_kind = kind.replace('.', "_");
    format!(
        "transaction_{}_{}",
        now_rfc3339().replace([':', '-'], ""),
        safe_kind
    )
}
