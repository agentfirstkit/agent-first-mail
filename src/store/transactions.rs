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
}

#[derive(Debug)]
pub(crate) struct LocalTransaction {
    path: PathBuf,
    finished: bool,
}

impl Workspace {
    pub(crate) fn begin_transaction(
        &self,
        kind: &str,
        paths: impl IntoIterator<Item = String>,
    ) -> Result<LocalTransaction> {
        validate_id("transaction kind", kind)?;
        let transactions_dir = self.root.join(".afmail/transactions");
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
        };
        write_json_pretty(&path, &transaction)?;
        Ok(LocalTransaction {
            path,
            finished: false,
        })
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
        let dir = self.root.join(".afmail/transactions");
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
            let transaction: TransactionFile =
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
            out.push(transaction);
        }
        out.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id));
        Ok(out)
    }
}

impl LocalTransaction {
    pub(crate) fn commit(mut self) -> Result<()> {
        if self.path.exists() {
            remove_file(&self.path)?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for LocalTransaction {
    fn drop(&mut self) {
        if !self.finished && !std::thread::panicking() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn incomplete_transaction_paths(root: &Path) -> Result<Vec<String>> {
    let dir = root.join(".afmail/transactions");
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
