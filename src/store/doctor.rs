use super::*;

#[derive(Clone, Debug, Serialize)]
struct DoctorIssue {
    code: String,
    severity: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    refs: Vec<String>,
    repairable: bool,
}

impl DoctorIssue {
    fn error(code: &str, message: impl Into<String>, path: Option<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "error",
            message: message.into(),
            path,
            refs: Vec::new(),
            repairable: false,
        }
    }

    fn warning(
        code: &str,
        message: impl Into<String>,
        path: Option<String>,
        repairable: bool,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: "warning",
            message: message.into(),
            path,
            refs: Vec::new(),
            repairable,
        }
    }
}

impl Workspace {
    pub fn doctor(&self) -> Result<Value> {
        self.require_workspace()?;
        let issues = self.doctor_issues()?;
        let repairable_count = issues.iter().filter(|issue| issue.repairable).count();
        let error_count = issues
            .iter()
            .filter(|issue| issue.severity == "error")
            .count();
        Ok(json!({
            "code": "doctor",
            "ok": issues.is_empty(),
            "issue_count": issues.len(),
            "error_count": error_count,
            "repairable_count": repairable_count,
            "checks": {
                "git_checked": false,
                "messages": true,
                "cases": true,
                "archives": true,
                "push_queue": true,
                "templates": true,
                "transactions": true,
            },
            "issues": issues,
        }))
    }

    pub fn doctor_repair(&self, confirm: bool) -> Result<Value> {
        self.require_workspace()?;
        if !confirm {
            return Err(AppError::new(
                "confirm_required",
                "doctor repair requires --confirm",
            )
            .with_hint("Inspect with `afmail doctor`; apply repairs with `afmail doctor repair --confirm`.")
            .with_details(json!({
                "suggested_commands": [
                    "afmail doctor",
                    "afmail doctor repair --confirm"
                ]
            })));
        }
        self.ensure_no_incomplete_transactions()?;
        let before = self.doctor_issues()?;
        let cache = self.rebuild_message_cache_from_eml()?;
        for path in message_json_paths(&self.root)? {
            if let Ok(message) = read_message(&path) {
                self.persist_message_state(&message)?;
                self.persist_message_remote(&message)?;
            }
        }
        let rendered = self.render_refresh()?;
        let after = self.doctor_issues()?;
        Ok(json!({
            "code": "doctor_repair",
            "confirmed": true,
            "before_issue_count": before.len(),
            "after_issue_count": after.len(),
            "message_cache_rebuilt_count": cache.rebuilt_count,
            "text_cache_removed_count": cache.removed_text_cache_count,
            "render": rendered,
            "remaining_issues": after,
        }))
    }

    fn doctor_issues(&self) -> Result<Vec<DoctorIssue>> {
        let mut issues = Vec::new();
        self.check_transactions(&mut issues)?;
        self.check_messages(&mut issues)?;
        self.check_case_refs(&mut issues)?;
        self.check_archive_refs(&mut issues)?;
        self.check_push_overlay(&mut issues)?;
        self.check_templates(&mut issues)?;
        Ok(issues)
    }

    fn check_transactions(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for transaction in self.incomplete_transactions()? {
            issues.push(DoctorIssue::error(
                "transaction_incomplete",
                format!(
                    "incomplete local transaction {} ({})",
                    transaction.transaction_id, transaction.kind
                ),
                Some(format!(
                    ".afmail/transactions/{}.json",
                    transaction.transaction_id
                )),
            ));
        }
        Ok(())
    }

    fn check_messages(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let mut ids = BTreeSet::new();
        for path in message_json_paths(&self.root)? {
            let rel = rel_path(&self.root, &path);
            let message = match read_message(&path) {
                Ok(message) => message,
                Err(err) => {
                    issues.push(DoctorIssue::error(
                        "message_cache_invalid",
                        err.message,
                        Some(rel),
                    ));
                    continue;
                }
            };
            ids.insert(message.message_id.clone());
            let eml = self
                .root
                .join(format!(".afmail/messages/{}.eml", message.message_id));
            if !eml.is_file() {
                issues.push(DoctorIssue::error(
                    "message_eml_missing",
                    format!("missing raw .eml for {}", message.message_id),
                    Some(rel_path(&self.root, &eml)),
                ));
            }
            let state = self.root.join(format!(
                ".afmail/messages/{}.state.json",
                message.message_id
            ));
            if !state.is_file() {
                issues.push(DoctorIssue::warning(
                    "message_state_missing",
                    format!("missing state sidecar for {}", message.message_id),
                    Some(rel_path(&self.root, &state)),
                    true,
                ));
            }
            if message
                .remote
                .as_ref()
                .is_some_and(|remote| !remote.locations.is_empty())
            {
                let remote = self.root.join(format!(
                    ".afmail/messages/{}.remote.json",
                    message.message_id
                ));
                if !remote.is_file() {
                    issues.push(DoctorIssue::warning(
                        "message_remote_missing",
                        format!("missing remote sidecar for {}", message.message_id),
                        Some(rel_path(&self.root, &remote)),
                        true,
                    ));
                }
            }
        }
        for entry in read_optional_dir(&self.root.join(".afmail/messages"), "read message state")? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let message_id = name
                .strip_suffix(".state.json")
                .or_else(|| name.strip_suffix(".remote.json"));
            if let Some(message_id) = message_id {
                if !ids.contains(message_id) && !self.message_path(message_id).is_file() {
                    issues.push(DoctorIssue::warning(
                        "message_sidecar_orphaned",
                        format!("sidecar has no materialized message cache: {message_id}"),
                        Some(rel_path(&self.root, &path)),
                        true,
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_case_refs(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for (case_uid, case_path) in self.all_case_entries()? {
            let messages = read_case_messages(&case_messages_json_path(&case_path), &case_uid)?;
            for message_id in messages.message_ids {
                if !self.message_path(&message_id).is_file() {
                    let mut issue = DoctorIssue::error(
                        "case_message_ref_broken",
                        format!("case {case_uid} references missing message {message_id}"),
                        Some(rel_path(&self.root, &case_messages_json_path(&case_path))),
                    );
                    issue.refs = vec![case_uid.clone(), message_id];
                    issues.push(issue);
                }
            }
        }
        Ok(())
    }

    fn check_archive_refs(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for archive_uid in self.archive_message_category_ids()? {
            let archive = self.read_archive_messages(&archive_uid)?;
            for item in archive.items {
                if !self.message_path(&item.message_id).is_file() {
                    let mut issue = DoctorIssue::error(
                        "archive_message_ref_broken",
                        format!(
                            "archive {archive_uid} references missing message {}",
                            item.message_id
                        ),
                        Some(rel_path(
                            &self.root,
                            &self.archive_message_json_path(&archive_uid),
                        )),
                    );
                    issue.refs = vec![archive_uid.clone(), item.message_id];
                    issues.push(issue);
                }
            }
        }
        Ok(())
    }

    fn check_push_overlay(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let items = crate::push_queue::pending_items(&self.root)?;
        let mut pending_by_message: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for item in items {
            for message_id in item.message_ids() {
                pending_by_message
                    .entry(message_id.clone())
                    .or_default()
                    .insert(item.push_id.clone());
                if !self.message_path(message_id).is_file() {
                    issues.push(DoctorIssue::error(
                        "push_message_ref_broken",
                        format!(
                            "push {} references missing message {message_id}",
                            item.push_id
                        ),
                        Some(format!(".afmail/push/{}.json", item.push_id)),
                    ));
                }
            }
        }
        for path in message_json_paths(&self.root)? {
            let Ok(message) = read_message(&path) else {
                continue;
            };
            let expected = pending_by_message
                .remove(&message.message_id)
                .unwrap_or_default();
            let actual = message
                .workspace
                .push
                .as_ref()
                .map(|push| {
                    push.pending
                        .iter()
                        .map(|pending| pending.push_id.clone())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            if expected != actual && (!expected.is_empty() || !actual.is_empty()) {
                issues.push(DoctorIssue::warning(
                    "push_overlay_drift",
                    format!(
                        "message {} push overlay differs from queue",
                        message.message_id
                    ),
                    Some(rel_path(&self.root, &path)),
                    true,
                ));
            }
        }
        Ok(())
    }

    fn check_templates(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let language = self.template_language()?;
        let mut renderer = MarkdownTemplateRenderer::new(&self.root, language);
        for key in TemplateKey::ALL {
            if let Err(err) = renderer.render(key, &minimal_template_context(language)) {
                issues.push(DoctorIssue::error(
                    "template_render_failed",
                    err.message,
                    Some(key.as_str().to_string()),
                ));
            }
        }
        Ok(())
    }
}

fn read_optional_dir(path: &Path, context: &str) -> Result<Vec<fs::DirEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    read_dir(path, context)
}

fn minimal_template_context(language: TemplateLanguage) -> Value {
    json!({
        "language": language.as_str(),
        "frontmatter": {},
        "message_id": "message_doctor",
        "case_uid": "c20260609001",
        "case_name": "doctor",
        "archive_uid": "a20260609001",
        "archive_name": "doctor",
        "title": "doctor",
        "status": "active",
        "message_count": 0,
        "attachment_count": 0,
        "generated_rfc3339": "2026-06-09T00:00:00Z",
        "archived_rfc3339": "2026-06-09T00:00:00Z",
        "summary": "doctor",
        "conversation": "",
        "items": [],
        "messages": [],
        "related_messages": [],
        "suggested_case_uids": [],
        "suggested_reason": "",
        "suggested_reason_yaml": "",
        "body_text_visible_block": "\n",
        "body_text_fence": "```",
        "display_heading": "doctor",
        "from": "",
        "subject": "",
        "to": [],
        "cc": [],
        "bcc": [],
        "security": {
            "authentication": {
                "check": false,
                "has_results": false,
                "spf": "missing",
                "dkim": "missing",
                "dmarc": "missing",
                "dmarc_policy": null,
                "authenticated_domain": null,
                "from_domain": null,
                "alignment": "unknown",
            },
            "possible_bcc": false,
            "reply_to_differs": false,
            "reply_to_recipients": "",
            "sender_differs": false,
            "sender": "",
            "mailing_list": "",
            "mailing_list_headers": "",
        },
        "hints": [],
        "attachments": [],
        "sender": "",
        "quoted": "",
        "config": {
            "archive": {
                "message_index": {
                    "item_fields": [],
                },
            },
        },
        "message": {
            "schema_name": "message",
            "schema_version": 1,
            "message_id": "message_doctor",
            "workspace": {"status": "triage"},
        },
        "case": {},
    })
}
