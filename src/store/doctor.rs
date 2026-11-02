use super::*;

#[derive(Clone, Debug, Serialize)]
pub(super) struct DoctorIssue {
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
    pub(super) fn error(code: &str, message: impl Into<String>, path: Option<String>) -> Self {
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
    pub fn doctor(&self) -> Result<crate::types::CommandResult> {
        self.require_workspace_directory()?;
        let issues = self.doctor_issues()?;
        let repairable_count = issues.iter().filter(|issue| issue.repairable).count();
        let (error_count, warning_count) = doctor_issue_counts(&issues);
        let (succeeded_count, failed_count) = doctor_counts(error_count, warning_count);
        Ok(crate::types::CommandResult::aggregate(
            json!({
                "ok": issues.is_empty(),
                "issue_count": issues.len(),
                "error_count": error_count,
                "warning_count": warning_count,
                "repairable_count": repairable_count,
                "checks": {
                    "git_checked": false,
                    "mail_source_configured": true,
                    "messages": true,
                    "contacts": true,
                    "cases": true,
                    "notifications": true,
                    "push_queue": true,
                    "templates": true,
                    "transactions": true,
                    "state_layout": true,
                    "cloudflare_consumer": true,
                },
                "issues": issues,
            }),
            succeeded_count,
            failed_count,
        ))
    }

    /// `--confirm` is part of the only registered `doctor repair` shape, so an
    /// unconfirmed repair never reaches this far.
    pub fn doctor_repair(&self) -> Result<crate::types::CommandResult> {
        self.require_workspace_directory()?;
        let before = self.doctor_issues()?;
        self.repair_incomplete_transactions()?;
        let deprecated_state_removed_count = self.remove_deprecated_message_state_sidecars()?;
        let cache = self.rebuild_message_cache_from_eml()?;
        for path in message_json_paths(&self.root)? {
            if let Ok(message) = read_message(&path) {
                self.persist_message_metadata(&message)?;
            }
        }
        let rendered = self.render_refresh()?;
        let after = self.doctor_issues()?;
        let (before_error_count, before_warning_count) = doctor_issue_counts(&before);
        let (after_error_count, after_warning_count) = doctor_issue_counts(&after);
        let (succeeded_count, failed_count) = doctor_counts(after_error_count, after_warning_count);
        Ok(crate::types::CommandResult::aggregate(
            json!({
                "confirmed": true,
                "before_issue_count": before.len(),
                "after_issue_count": after.len(),
                "before_error_count": before_error_count,
                "before_warning_count": before_warning_count,
                "after_error_count": after_error_count,
                "after_warning_count": after_warning_count,
                "message_cache_rebuilt_count": cache.rebuilt_count,
                "text_cache_removed_count": cache.removed_text_cache_count,
                "deprecated_state_removed_count": deprecated_state_removed_count,
                "render": rendered,
                "remaining_issues": after,
            }),
            succeeded_count,
            failed_count,
        ))
    }

    fn doctor_issues(&self) -> Result<Vec<DoctorIssue>> {
        let mut issues = Vec::new();
        if let Ok(config) = MailConfig::load(&self.root)
            && crate::sources::configured(&config).is_empty()
        {
            issues.push(DoctorIssue::warning(
                "no_mail_source",
                "No IMAP, Cloudflare, or file mail source is configured",
                Some(".afmail/config.json".into()),
                false,
            ));
        }
        self.check_state_layout(&mut issues)?;
        self.check_transactions(&mut issues)?;
        self.check_idempotency_requests(&mut issues)?;
        self.check_messages(&mut issues)?;
        self.check_contacts(&mut issues)?;
        self.check_case_refs(&mut issues)?;
        self.check_notification_refs(&mut issues)?;
        self.check_push_overlay(&mut issues)?;
        self.check_draft_queue_state(&mut issues)?;
        self.check_cloudflare_consumer(&mut issues)?;
        self.check_templates(&mut issues)?;
        Ok(issues)
    }

    fn check_cloudflare_consumer(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        use crate::sources::cloudflare_journal::{self as journal, Journal, TaskStatus};
        let path = ".afmail/cloudflare-consumer/journal.json";
        let mut state = match Journal::load(&self.root) {
            Ok(state) => state,
            Err(error) => {
                issues.push(DoctorIssue::error(
                    error.error_code,
                    error.message,
                    Some(path.into()),
                ));
                return Ok(());
            }
        };
        if let Err(error) = journal::apply_completed(&self.root, &mut state) {
            let issue_path = error
                .details
                .as_ref()
                .and_then(|details| details["path"].as_str())
                .unwrap_or(path);
            issues.push(DoctorIssue::error(
                error.error_code,
                &error.message,
                Some(issue_path.to_string()),
            ));
        }
        for task in state
            .tasks
            .values()
            .filter(|task| task.status == TaskStatus::Pending)
        {
            if !self
                .root
                .join(format!(".afmail/messages/{}.eml", task.message_id))
                .is_file()
            {
                let mut issue = DoctorIssue::warning(
                    "consumer_message_missing",
                    "Pending consumer task has no raw mail evidence; the next consume tick will drop it",
                    Some(path.into()),
                    false,
                );
                issue.refs.push(task.message_id.clone());
                issues.push(issue);
            }
        }
        if let Some(batch) = state.active
            && batch.last_error_code.is_some()
        {
            let mut issue = DoctorIssue::warning(
                "consumer_batch_failed",
                "The active consumer batch failed and blocks later batches; inspect status.cloudflare_consumer and fix the configured handler before retrying",
                Some(path.into()),
                false,
            );
            issue.refs.push(batch.task_id);
            issues.push(issue);
        }
        Ok(())
    }

    fn require_workspace_directory(&self) -> Result<()> {
        if self.root.join(layout::AFMAIL).is_dir() {
            Ok(())
        } else {
            Err(AppError::new(
                "workspace_not_found",
                "no .afmail directory found at workspace root",
            ))
        }
    }

    fn check_state_layout(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let afmail = self.root.join(layout::AFMAIL);
        for name in layout::LEGACY_AFMAIL_FILES {
            let path = afmail.join(name);
            if path.exists() {
                issues.push(DoctorIssue::error(
                    "workspace_layout_outdated",
                    format!("obsolete root-level .afmail state: {name}"),
                    Some(format!(".afmail/{name}")),
                ));
            }
        }
        for dirname in [layout::AFMAIL_PULL, layout::AFMAIL_CACHE] {
            let path = afmail.join(dirname);
            if !path.is_dir() {
                issues.push(DoctorIssue::error(
                    "workspace_layout_directory_missing",
                    format!("required .afmail directory is missing: {dirname}"),
                    Some(format!(".afmail/{dirname}")),
                ));
            }
        }
        let allowed_root_files = ["DO_NOT_EDIT.txt", "config.json", "workspace.json"];
        for entry in read_optional_dir(&afmail, "read .afmail state layout")? {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path_file_name(&path);
            if !allowed_root_files.contains(&name.as_str())
                && !layout::LEGACY_AFMAIL_FILES.contains(&name.as_str())
            {
                issues.push(DoctorIssue::error(
                    "workspace_layout_unexpected_root_file",
                    format!("unexpected root-level .afmail file: {name}"),
                    Some(format!(".afmail/{name}")),
                ));
            }
        }
        Ok(())
    }

    fn check_transactions(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for transaction in self.incomplete_transactions()? {
            let mut issue = DoctorIssue::error(
                "transaction_incomplete",
                format!(
                    "incomplete local transaction {} ({})",
                    transaction.transaction_id, transaction.kind
                ),
                Some(rel_path(&self.root, &transaction.path)),
            );
            issue.repairable = transaction.is_repairable(self)?;
            issues.push(issue);
        }
        Ok(())
    }

    fn check_messages(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let mut ids = BTreeSet::new();
        let mut rebuild_context = None;
        for path in message_json_paths(&self.root)? {
            let rel = rel_path(&self.root, &path);
            let message = match read_message(&path) {
                Ok(message) => message,
                Err(err) => {
                    let mut issue =
                        DoctorIssue::error("message_cache_invalid", err.message, Some(rel));
                    if let Some(message_id) = path.file_stem().and_then(|stem| stem.to_str()) {
                        let context =
                            rebuild_context.get_or_insert_with(|| self.materialization_context());
                        issue.repairable = context.as_ref().is_ok_and(|context| {
                            self.validate_message_cache_rebuild(message_id, context)
                                .is_ok()
                        });
                    }
                    issues.push(issue);
                    continue;
                }
            };
            ids.insert(message.message_id.clone());
            let eml = if message.origin.is_some() {
                self.root
                    .join(".afmail/messages")
                    .join(format!("{}.message", message.message_id))
            } else {
                super::messages::message_eml_path(&self.root, &message.message_id)
            };
            if !eml.is_file() {
                issues.push(DoctorIssue::error(
                    if message.origin.is_some() {
                        "message_evidence_missing"
                    } else {
                        "message_eml_missing"
                    },
                    format!(
                        "missing canonical message evidence for {}",
                        message.message_id
                    ),
                    Some(rel_path(&self.root, &eml)),
                ));
            }
            let metadata = self
                .root
                .join(format!(".afmail/messages/{}.json", message.message_id));
            if !metadata.is_file() {
                issues.push(DoctorIssue::warning(
                    "message_metadata_missing",
                    format!("missing canonical metadata for {}", message.message_id),
                    Some(rel_path(&self.root, &metadata)),
                    true,
                ));
            }
        }
        for entry in read_optional_dir(&self.root.join(".afmail/messages"), "read message state")? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.ends_with(".state.json") {
                issues.push(DoctorIssue::warning(
                    "message_state_deprecated",
                    format!("deprecated message state sidecar: {name}"),
                    Some(rel_path(&self.root, &path)),
                    true,
                ));
                continue;
            }
            if let Some(message_id) = name.strip_suffix(".json")
                && !ids.contains(message_id)
                && !self.message_path(message_id).is_file()
            {
                // Repair rebuilds the cache from the raw message; without one
                // there is nothing to rebuild from, and the metadata may be the
                // only record left, so repair must not claim or delete it.
                let raw_present = ["eml", "message"].iter().any(|extension| {
                    path.with_file_name(format!("{message_id}.{extension}"))
                        .is_file()
                });
                issues.push(DoctorIssue::warning(
                    "message_metadata_orphaned",
                    format!("metadata has no materialized message cache: {message_id}"),
                    Some(rel_path(&self.root, &path)),
                    raw_present,
                ));
            }
        }
        Ok(())
    }

    fn check_contacts(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for diagnostic in self.contact_index_diagnostics()? {
            let mut issue =
                DoctorIssue::error(&diagnostic.code, diagnostic.message, Some(diagnostic.path));
            if let Some(contact_uid) = diagnostic.contact_uid {
                issue.refs.push(contact_uid);
            }
            issue.refs.extend(diagnostic.conflicting_contact_uids);
            issue.refs.sort();
            issue.refs.dedup();
            issues.push(issue);
        }
        Ok(())
    }

    fn remove_deprecated_message_state_sidecars(&self) -> Result<usize> {
        let mut removed = 0usize;
        for entry in read_optional_dir(&self.root.join(".afmail/messages"), "read message state")? {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".state.json"))
            {
                remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn check_case_refs(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for (case_uid, case_path) in self.all_case_entries()? {
            let messages = read_case_messages(&case_path, &case_uid)?;
            for message_id in messages.message_ids() {
                if !self.message_path(&message_id).is_file() {
                    let mut issue = DoctorIssue::error(
                        "case_message_ref_broken",
                        format!("case {case_uid} references missing message {message_id}"),
                        Some(rel_path(&self.root, &case_json_path(&case_path))),
                    );
                    issue.refs = vec![case_uid.clone(), message_id];
                    issues.push(issue);
                }
            }
        }
        Ok(())
    }

    fn check_notification_refs(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for notification_uid in self.notification_ids()? {
            let notification = self.read_notifications(&notification_uid)?;
            for item in notification.items {
                if !self.message_path(&item.message_id).is_file() {
                    let mut issue = DoctorIssue::error(
                        "notification_message_ref_broken",
                        format!(
                            "notification {notification_uid} references missing message {}",
                            item.message_id
                        ),
                        Some(rel_path(
                            &self.root,
                            &self.notification_json_path(&notification_uid),
                        )),
                    );
                    issue.refs = vec![notification_uid.clone(), item.message_id];
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
            if item.audit_pending.is_some() {
                issues.push(DoctorIssue::warning(
                    "push_audit_pending",
                    format!(
                        "push {} completed its remote effect but still needs its audit record",
                        item.push_id
                    ),
                    Some(format!(".afmail/push/{}.json", item.push_id)),
                    false,
                ));
            }
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

    fn check_draft_queue_state(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        for (case_uid, case_path) in self.all_case_entries()? {
            for entry in read_optional_dir(&case_path.join("drafts"), "read case drafts")? {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("md") {
                    continue;
                }
                let draft_name = path_file_name(&path);
                let text = match read_to_string(&path, "read draft") {
                    Ok(text) => text,
                    Err(err) => {
                        issues.push(DoctorIssue::error(
                            "draft_invalid",
                            err.message,
                            Some(rel_path(&self.root, &path)),
                        ));
                        continue;
                    }
                };
                let (frontmatter, _) = match read_doc::<DraftFrontmatter>(&text) {
                    Ok(value) => value,
                    Err(err) => {
                        issues.push(DoctorIssue::error(
                            "draft_invalid",
                            format!("invalid draft frontmatter: {err}"),
                            Some(rel_path(&self.root, &path)),
                        ));
                        continue;
                    }
                };
                let queued =
                    crate::push_queue::find_outbound_for_draft(&self.root, &case_uid, &draft_name)?;
                let projection =
                    DraftStateProjection::from_queue(frontmatter.sync_intent, queued.as_ref());
                if let Some(issue) = projection.issue {
                    let mut diagnostic = DoctorIssue::error(
                        "draft_queue_state_inconsistent",
                        issue.message(),
                        Some(rel_path(&self.root, &path)),
                    );
                    diagnostic.refs = vec![case_uid.clone(), draft_name];
                    issues.push(diagnostic);
                }
            }
        }
        Ok(())
    }

    fn check_templates(&self, issues: &mut Vec<DoctorIssue>) -> Result<()> {
        let legacy_templates = self.root.join(".afmail/templates");
        if legacy_templates.exists() {
            issues.push(DoctorIssue::warning(
                "legacy_template_dir",
                ".afmail/templates is obsolete; workspace templates now live under templates/",
                Some(".afmail/templates".to_string()),
                false,
            ));
        }
        let language = self.template_language()?;
        let mut renderer = TemplateRenderer::new(&self.root, language);
        for key in TemplateKey::ALL
            .into_iter()
            .filter(|key| !key.is_review_ui())
        {
            smoke_render(&mut renderer, key, None, issues);
        }
        self.check_review_ui_templates(language, issues);
        Ok(())
    }

    /// Render each Review UI surface the way `review` would serve it.
    ///
    /// Through `UiPage`, not a second renderer: the serving path assembles the
    /// page with AFUI's escaping policy, afmail's own filters, the rendered
    /// markup guard and the element/runtime-marker contract, and a template
    /// that satisfies a bare MiniJinja render can still fail every one of
    /// those. Validating through the same engine is the only way this promise
    /// means anything.
    ///
    /// One render per surface covers that surface's whole tree: the page
    /// includes the message, the editors, the workbench and the stylesheet,
    /// which is inlined rather than linked.
    ///
    /// A person who installed a frontend wants to hear about a broken one
    /// here, before a review command opens a window on it. Discovery failures
    /// are reported rather than propagated so that one unloadable frontend
    /// does not hide every other thing doctor has to say.
    #[cfg(feature = "review")]
    fn check_review_ui_templates(
        &self,
        _language: TemplateLanguage,
        issues: &mut Vec<DoctorIssue>,
    ) {
        for ui_kind in crate::review_frontend::UI_KINDS {
            let frontend = match crate::review_frontend::review_ui_frontend(&self.root, ui_kind) {
                Ok(frontend) => frontend,
                Err(error) => {
                    issues.push(DoctorIssue::error(
                        error.error_code,
                        error.message,
                        Some(ui_kind.to_string()),
                    ));
                    continue;
                }
            };
            let model = match doctor_review_context(ui_kind) {
                Ok(model) => model,
                Err(err) => {
                    issues.push(DoctorIssue::error(
                        "template_context_failed",
                        err.message,
                        Some(ui_kind.to_string()),
                    ));
                    continue;
                }
            };
            if let Err(err) = crate::runner::render_review_page_from(
                self,
                &crate::templates::json_value_to_minijinja(&model),
                DOCTOR_SMOKE_NONCE,
                &frontend,
            ) {
                issues.push(DoctorIssue::error(
                    err.error_code,
                    err.message,
                    Some(ui_kind.to_string()),
                ));
            }
        }
    }

    /// A build with the Review UI compiled out has no page to render and no
    /// frontend to resolve, so there is nothing here to check.
    #[cfg(not(feature = "review"))]
    fn check_review_ui_templates(
        &self,
        _language: TemplateLanguage,
        _issues: &mut Vec<DoctorIssue>,
    ) {
    }
}

/// The nonce a doctor render binds its trusted runtime to.
///
/// Fixed rather than generated: nothing serves this page, and a smoke render
/// that differed run to run would be harder to compare than to trust.
#[cfg(feature = "review")]
const DOCTOR_SMOKE_NONCE: &str = "doctor-smoke-nonce";

fn smoke_render(
    renderer: &mut TemplateRenderer<'_>,
    key: TemplateKey,
    ui_kind: Option<&str>,
    issues: &mut Vec<DoctorIssue>,
) {
    let subject = match ui_kind {
        Some(ui_kind) => format!("{ui_kind}:{}", key.as_str()),
        None => key.as_str().to_string(),
    };
    let context = match minimal_template_context(key) {
        Ok(context) => context,
        Err(err) => {
            issues.push(DoctorIssue::error(
                "template_context_failed",
                err.message,
                Some(subject),
            ));
            return;
        }
    };
    if let Err(err) = renderer.render(key, &context) {
        issues.push(DoctorIssue::error(
            "template_render_failed",
            err.message,
            Some(subject),
        ));
    }
}

fn doctor_issue_counts(issues: &[DoctorIssue]) -> (usize, usize) {
    let error_count = issues
        .iter()
        .filter(|issue| issue.severity == "error")
        .count();
    let warning_count = issues
        .iter()
        .filter(|issue| issue.severity == "warning")
        .count();
    (error_count, warning_count)
}

fn doctor_counts(error_count: usize, warning_count: usize) -> (usize, usize) {
    if error_count > 0 {
        (0, error_count)
    } else if warning_count > 0 {
        (1, warning_count)
    } else {
        (1, 0)
    }
}

fn read_optional_dir(path: &Path, context: &str) -> Result<Vec<fs::DirEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    read_dir(path, context)
}

// Identifying values the render matrix test greps for in output, so a typo'd
// template variable that happens to still render (e.g. references a sibling
// field instead of the intended one) doesn't slip through on "no error" alone.
pub(super) const DOCTOR_SMOKE_MESSAGE_ID: &str = "message_doctor_9f3c";
pub(super) const DOCTOR_SMOKE_CASE_UID: &str = "c20260609001";
pub(super) const DOCTOR_SMOKE_NOTIFICATION_UID: &str = "n20260609001";
pub(super) const DOCTOR_SMOKE_CONVERSATION: &str = "Doctor smoke conversation message_doctor_9f3c.";
pub(super) const DOCTOR_SMOKE_OTHER_CASE_UID: &str = "c20260609002";
pub(super) const DOCTOR_SMOKE_OTHER_BODY: &str = "Doctor smoke merge body.";
pub(super) const DOCTOR_SMOKE_QUOTED: &str = "Doctor smoke quoted body.";

fn doctor_message(status: &str, message_id: &str, body_text: &str) -> MessageFile {
    MessageFile {
        schema_name: "message".to_string(),
        schema_version: MESSAGE_SCHEMA_VERSION,
        message_id: message_id.to_string(),
        origin: None,
        rfc822_message_id: Some(format!("<{message_id}@example.com>")),
        in_reply_to: None,
        references: Vec::new(),
        remote: None,
        agent_suggestion: None,
        human_suggestion: None,
        case_relationship_suggestion: (message_id == DOCTOR_SMOKE_MESSAGE_ID).then(|| {
            CaseRelationshipSuggestion {
                case_uids: vec![DOCTOR_SMOKE_CASE_UID.to_string()],
                evidence: crate::types::CaseRelationshipEvidence::ReplyHeadersMatchedLocalCase,
            }
        }),
        direction: Some("inbound".to_string()),
        subject: Some("Doctor smoke subject".to_string()),
        from: Some("Doctor Smoke Sender <doctor-smoke-from@example.com>".to_string()),
        to: vec!["doctor-smoke-to@example.com".to_string()],
        cc: vec!["doctor-smoke-cc@example.com".to_string()],
        bcc: Vec::new(),
        reply_to: Vec::new(),
        sender: None,
        delivered_to: Vec::new(),
        x_original_to: Vec::new(),
        envelope_to: Vec::new(),
        list_id: None,
        mailing_list_headers: Vec::new(),
        authentication: crate::types::MessageAuthentication {
            spf: crate::types::AuthVerdict::Pass,
            dkim: crate::types::AuthVerdict::Pass,
            dmarc: crate::types::AuthVerdict::Pass,
            dmarc_policy: None,
            authenticated_domain: Some("example.com".to_string()),
            from_domain: Some("example.com".to_string()),
            alignment: crate::types::AuthAlignment::Aligned,
            raw: vec!["doctor-smoke authentication".to_string()],
        },
        received_rfc3339: Some("2026-06-09T00:00:00Z".to_string()),
        sent_rfc3339: None,
        body_text: body_text.to_string(),
        body_markdown: None,
        eml_path: Some(format!(".afmail/messages/{message_id}.eml")),
        attachments: vec![AttachmentRef {
            part_id: "2".to_string(),
            filename: "doctor-smoke.png".to_string(),
            content_type: "image/png".to_string(),
            size_bytes: 128,
            file_path: Some(format!(
                ".afmail/messages/{message_id}.files/doctor-smoke.png"
            )),
            source_path: None,
        }],
        contact: Some(ContactFrontmatter {
            kind: crate::types::CONTACT_KIND.to_string(),
            contact_uid: "doctor-smoke-contact".to_string(),
            display_name: "Doctor Smoke Contact".to_string(),
            emails: vec!["doctor-smoke-from@example.com".to_string()],
            phones: Vec::new(),
            organization: String::new(),
            role: String::new(),
            tags: Vec::new(),
            created_rfc3339: "2026-06-09T00:00:00Z".to_string(),
            updated_rfc3339: "2026-06-09T00:00:00Z".to_string(),
        }),
        identity: Some("me".to_string()),
        identity_email: Some("me@example.com".to_string()),
        identity_match: Some("me".to_string()),
        identity_candidates: vec!["me".to_string()],
        observed_recipient_emails: vec!["doctor-smoke-to@example.com".to_string()],
        workspace: WorkspaceState {
            status: status.to_string(),
            notification_uid: None,
            notification_rfc3339: None,
            origin: None,
            remote_sync: None,
            push: None,
        },
    }
}

fn doctor_message_document(status: &str) -> MessageRenderDocument {
    build_message_render_document(
        &doctor_message(status, DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_CONVERSATION),
        Some("me@example.com"),
    )
}

fn doctor_reply_message_document() -> MessageRenderDocument {
    build_message_render_document(
        &doctor_message("triage", DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_QUOTED),
        Some("me@example.com"),
    )
}

fn doctor_collection(schema_name: &str, uid: &str, status: &str) -> MessageCollection {
    let timestamp = "2026-06-09T00:00:00Z";
    let mut collection = match schema_name {
        CASE_SCHEMA_NAME => MessageCollection::new_case(uid, "Doctor smoke collection", timestamp),
        NOTIFICATION_SCHEMA_NAME => {
            MessageCollection::new_notification(uid, "Doctor smoke collection", timestamp)
        }
        _ => MessageCollection::new(schema_name, uid, status),
    };
    collection.status = status.to_string();
    collection.tags = vec!["doctor-smoke".to_string()];
    collection.upsert_item(
        DOCTOR_SMOKE_MESSAGE_ID,
        Some("Doctor smoke summary"),
        timestamp,
    );
    collection
}

fn doctor_collection_location(schema_name: &str) -> Option<CollectionLocation> {
    (schema_name == CASE_SCHEMA_NAME).then(|| CollectionLocation {
        group: "support".to_string(),
    })
}

fn doctor_collection_document(
    schema_name: &str,
    uid: &str,
    status: &str,
) -> CollectionRenderDocument {
    let message_status = match schema_name {
        CASE_SCHEMA_NAME => "case",
        NOTIFICATION_SCHEMA_NAME => "notification",
        _ => status,
    };
    collection_render_document(
        doctor_collection(schema_name, uid, status),
        vec![doctor_message_document(message_status)],
        doctor_collection_location(schema_name),
    )
}

fn doctor_collection_message_document(
    schema_name: &str,
    uid: &str,
) -> CollectionMessageRenderDocument {
    let status = if schema_name == NOTIFICATION_SCHEMA_NAME {
        "notification"
    } else {
        "case"
    };
    collection_message_render_document(
        doctor_collection(schema_name, uid, "active"),
        doctor_message_document(status),
        doctor_collection_location(schema_name),
    )
}

/// Only a review page shows a draft as an editable document; the Markdown
/// draft templates render from their own source document.
#[cfg(feature = "review")]
fn doctor_draft_document() -> DraftRenderDocument {
    let authored = "Doctor smoke authored reply.";
    let body_markdown = format!("{authored}\n{DOCTOR_SMOKE_QUOTED}");
    DraftRenderDocument {
        schema_name: draft_document_schema_name(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        locator: DraftLocator {
            draft_name: "doctor-smoke.md".to_string(),
            draft_path: "cases/support/c20260609001/drafts/doctor-smoke.md".to_string(),
        },
        draft: DraftSourceDocument {
            frontmatter: DraftFrontmatter {
                reply_route: None,
                kind: Some("draft".to_string()),
                case_uid: DOCTOR_SMOKE_CASE_UID.to_string(),
                send_intent: None,
                sync_intent: None,
                reply_to_message_id: Some(DOCTOR_SMOKE_MESSAGE_ID.to_string()),
                identity: Some("me".to_string()),
                subject: Some("Doctor smoke reply".to_string()),
                to: vec!["doctor-smoke-to@example.com".to_string()],
                cc: Vec::new(),
                bcc: Vec::new(),
                attachments: Vec::new(),
            },
            body_markdown,
        },
        analysis: DraftRenderAnalysis {
            authored_body_range: TextRange {
                start_bytes: 0,
                end_bytes: authored.len(),
            },
            attachments: Vec::new(),
            completeness: DraftCompleteness::Complete,
            status: crate::types::DraftStatus::Editable,
            push_id: None,
        },
    }
}

fn doctor_notes_document() -> NotesRenderDocument {
    NotesRenderDocument {
        schema_name: notes_document_schema_name(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        notes_path: "cases/support/c20260609001/notes.md".to_string(),
        text: DOCTOR_SMOKE_OTHER_BODY.to_string(),
    }
}

fn doctor_empty_document() -> EmptyRenderDocument {
    EmptyRenderDocument {
        schema_name: "afmail_empty_render_document".to_string(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
    }
}

fn doctor_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| AppError::json("serialize doctor context", &error))
}

/// One review surface's smoke document, in the shape the page renders from.
///
/// Every surface gets its own: the page branches on `surface_kind`, so a case
/// document would leave the triage, notification and draft branches — and the
/// includes only they reach — unrendered and unchecked.
#[cfg(feature = "review")]
fn doctor_review_context(ui_kind: &str) -> Result<Value> {
    let notes_state = json!({
        "document": doctor_value(&doctor_notes_document())?,
        "base_hash": "doctor-smoke-notes-hash",
    });
    let draft_state = json!({
        "document": doctor_value(&doctor_draft_document())?,
        "base_hash": "doctor-smoke-draft-hash",
    });
    let contents = match ui_kind {
        "triage_review" => json!({
            "items": [{
                "document": doctor_value(&triage_render_document(
                    doctor_message_document("triage"),
                    vec![build_message_render_document(
                        &doctor_message(
                            "triage",
                            "message_doctor_related",
                            "Doctor smoke related conversation.",
                        ),
                        Some("me@example.com"),
                    )],
                ))?,
                "message_review_base_hash": "doctor-smoke-message-hash",
                "stale": false,
            }],
        }),
        "notification_review" => json!({
            "collection": doctor_value(&doctor_collection_document(
                NOTIFICATION_SCHEMA_NAME,
                DOCTOR_SMOKE_NOTIFICATION_UID,
                "active",
            ))?,
            "notification_review_base_hash": "doctor-smoke-notification-hash",
            "stale": false,
            "notes": notes_state,
        }),
        "draft_review" => json!({ "draft": draft_state }),
        _ => json!({
            "collection": doctor_value(&doctor_collection_document(
                CASE_SCHEMA_NAME,
                DOCTOR_SMOKE_CASE_UID,
                "active",
            ))?,
            "case_review_base_hash": "doctor-smoke-case-hash",
            "stale": false,
            "notes": notes_state,
            "drafts": [draft_state],
        }),
    };
    Ok(json!({
        "schema_name": "afmail_review_render_document",
        "schema_version": RENDER_DOCUMENT_SCHEMA_VERSION,
        "surface_kind": ui_kind,
        "contents": contents,
    }))
}

fn minimal_template_context(key: TemplateKey) -> Result<Value> {
    match key {
        TemplateKey::NotesMergeSection => doctor_value(&NotesMergeRenderDocument {
            schema_name: "afmail_notes_merge_render_document".to_string(),
            schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
            primary: doctor_notes_document(),
            merged_case_uid: DOCTOR_SMOKE_OTHER_CASE_UID.to_string(),
            merged: NotesRenderDocument {
                schema_name: notes_document_schema_name(),
                schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
                notes_path: "cases/support/c20260609002/notes.md".to_string(),
                text: DOCTOR_SMOKE_OTHER_BODY.to_string(),
            },
        }),
        TemplateKey::DraftReply => doctor_value(&DraftReplyRenderDocument {
            schema_name: "afmail_draft_reply_render_document".to_string(),
            schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
            message: doctor_reply_message_document(),
        }),
        TemplateKey::MessageSection | TemplateKey::MarkdownMessageSectionLayout => {
            doctor_value(&doctor_message_document("triage"))
        }
        TemplateKey::TriageView | TemplateKey::MarkdownTriageViewLayout => {
            let related = build_message_render_document(
                &doctor_message(
                    "triage",
                    "message_doctor_related",
                    "Doctor smoke related conversation.",
                ),
                Some("me@example.com"),
            );
            doctor_value(&triage_render_document(
                doctor_message_document("triage"),
                vec![related],
            ))
        }
        TemplateKey::CaseDocument | TemplateKey::MarkdownCaseDocumentLayout => doctor_value(
            &doctor_collection_document(CASE_SCHEMA_NAME, DOCTOR_SMOKE_CASE_UID, "active"),
        ),
        TemplateKey::CaseMessage | TemplateKey::MarkdownCaseMessageLayout => doctor_value(
            &doctor_collection_message_document(CASE_SCHEMA_NAME, DOCTOR_SMOKE_CASE_UID),
        ),
        TemplateKey::NotificationIndex | TemplateKey::MarkdownNotificationIndexLayout => {
            doctor_value(&doctor_collection_document(
                NOTIFICATION_SCHEMA_NAME,
                DOCTOR_SMOKE_NOTIFICATION_UID,
                "active",
            ))
        }
        TemplateKey::NotificationMessage | TemplateKey::MarkdownNotificationMessageLayout => {
            doctor_value(&doctor_collection_message_document(
                NOTIFICATION_SCHEMA_NAME,
                DOCTOR_SMOKE_NOTIFICATION_UID,
            ))
        }
        TemplateKey::StatusIndex | TemplateKey::MarkdownStatusIndexLayout => {
            doctor_value(&doctor_collection_document("spam", "spam", "spam"))
        }
        TemplateKey::StatusMessage | TemplateKey::MarkdownStatusMessageLayout => {
            doctor_value(&doctor_message_document("spam"))
        }
        TemplateKey::ContactDefault => doctor_value(&ContactRenderDocument {
            schema_name: "afmail_contact_render_document".to_string(),
            schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
            locator: ContactLocator {
                group: "people".to_string(),
                contact_path: "contacts/people/p20260609001-doctor-smoke-contact.md".to_string(),
            },
            contact: crate::types::ContactFrontmatter::new(
                "p20260609001",
                "Doctor Smoke Contact",
                "2026-06-09T00:00:00Z",
            ),
            notes_markdown: DOCTOR_SMOKE_OTHER_BODY.to_string(),
        }),
        TemplateKey::WorkspaceDoNotEdit | TemplateKey::NotesDefault | TemplateKey::DraftNew => {
            doctor_value(&doctor_empty_document())
        }
        TemplateKey::DemoHandoff => Ok(json!({
            "workspace_ref": "afmail-demo/",
            "agent_prompt": "doctor-smoke-agent-prompt",
        })),
        TemplateKey::LocaleCatalog => Ok(json!({})),
        // Rendered as part of a page rather than on their own — see
        // `check_review_ui_templates`. Reaching one here would mean a review
        // key had escaped that route.
        key if key.is_review_ui() => Err(AppError::new(
            "template_context_failed",
            format!(
                "{} is rendered through the review page, not on its own",
                key.as_str()
            ),
        )),
        _ => Err(AppError::new(
            "template_context_failed",
            "template has no doctor smoke context",
        )),
    }
}

// Sentinel strings a correctly-wired render of `key` must surface. A template
// that renders without error but references the wrong field (e.g. a sibling
// key that happens to also resolve) would pass a bare `is_ok()` check but
// fail here, since the expected identifying value never made it into text.
#[cfg(test)]
fn doctor_smoke_expectations(key: TemplateKey) -> &'static [&'static str] {
    match key {
        TemplateKey::WorkspaceDoNotEdit | TemplateKey::NotesDefault | TemplateKey::DraftNew => &[],
        TemplateKey::DemoHandoff => &["afmail-demo/", "doctor-smoke-agent-prompt"],
        TemplateKey::ContactDefault => &[DOCTOR_SMOKE_OTHER_BODY],
        TemplateKey::NotesMergeSection => &[DOCTOR_SMOKE_OTHER_CASE_UID, DOCTOR_SMOKE_OTHER_BODY],
        TemplateKey::DraftReply => &[DOCTOR_SMOKE_QUOTED],
        TemplateKey::MessageSection => &[DOCTOR_SMOKE_MESSAGE_ID],
        TemplateKey::TriageView => &[
            DOCTOR_SMOKE_MESSAGE_ID,
            DOCTOR_SMOKE_CASE_UID,
            DOCTOR_SMOKE_CONVERSATION,
        ],
        TemplateKey::CaseDocument => &[
            DOCTOR_SMOKE_MESSAGE_ID,
            DOCTOR_SMOKE_CONVERSATION,
            DOCTOR_SMOKE_CASE_UID,
        ],
        TemplateKey::CaseMessage => &[
            DOCTOR_SMOKE_MESSAGE_ID,
            DOCTOR_SMOKE_CASE_UID,
            DOCTOR_SMOKE_CONVERSATION,
        ],
        TemplateKey::NotificationIndex => &[DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_NOTIFICATION_UID],
        TemplateKey::NotificationMessage => &[
            DOCTOR_SMOKE_MESSAGE_ID,
            DOCTOR_SMOKE_NOTIFICATION_UID,
            DOCTOR_SMOKE_CONVERSATION,
        ],
        TemplateKey::StatusIndex => &[DOCTOR_SMOKE_MESSAGE_ID],
        TemplateKey::StatusMessage => &[DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_CONVERSATION],
        TemplateKey::ReviewPage => &[DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_CASE_UID],
        TemplateKey::ReviewMessage => &[DOCTOR_SMOKE_MESSAGE_ID],
        TemplateKey::ReviewDraftEditor => &["doctor-smoke.md"],
        TemplateKey::ReviewNotesEditor => &[DOCTOR_SMOKE_OTHER_BODY],
        TemplateKey::ReviewStyle
        | TemplateKey::LocaleCatalog
        | TemplateKey::MarkdownMessageSectionLayout
        | TemplateKey::MarkdownTriageViewLayout
        | TemplateKey::MarkdownCaseDocumentLayout
        | TemplateKey::MarkdownCaseMessageLayout
        | TemplateKey::MarkdownNotificationIndexLayout
        | TemplateKey::MarkdownNotificationMessageLayout
        | TemplateKey::MarkdownStatusIndexLayout
        | TemplateKey::MarkdownStatusMessageLayout
        | TemplateKey::ReviewPageLayout
        | TemplateKey::ReviewMessageLayout
        | TemplateKey::ReviewDraftEditorLayout
        | TemplateKey::ReviewNotesEditorLayout
        | TemplateKey::ReviewAgentWorkbenchLayout => &[],
    }
}

/// What a correctly-wired render of one review surface must surface, the same
/// way [`doctor_smoke_expectations`] does for a single template: a page that
/// renders but reads the wrong field would otherwise pass on `is_ok()` alone.
#[cfg(all(test, feature = "review"))]
fn doctor_review_expectations(ui_kind: &str) -> &'static [&'static str] {
    match ui_kind {
        "triage_review" => &[DOCTOR_SMOKE_MESSAGE_ID],
        "notification_review" => &[DOCTOR_SMOKE_NOTIFICATION_UID, DOCTOR_SMOKE_MESSAGE_ID],
        "draft_review" => &["doctor-smoke.md"],
        _ => &[DOCTOR_SMOKE_CASE_UID, DOCTOR_SMOKE_MESSAGE_ID],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_template_renders_in_every_language() {
        // The release gate only renders en-US/zh-CN with real data, so a Jinja
        // syntax error (or a silently-empty typo'd variable, now that
        // MiniJinja runs SemiStrict) in another language pack would otherwise
        // slip through. Render every (language, key) against the smoke
        // context, and confirm the sentinel identifiers this key is expected
        // to surface actually made it into the rendered text.
        for language in TemplateLanguage::ALL {
            let mut renderer = TemplateRenderer::builtin(language);
            for key in TemplateKey::ALL
                .into_iter()
                .filter(|key| !key.is_review_ui())
            {
                let context = minimal_template_context(key).unwrap_or_else(|error| {
                    panic!(
                        "builtin template {}/{} context failed: {}",
                        language.as_str(),
                        key.as_str(),
                        error.message
                    )
                });
                let result = renderer.render(key, &context);
                assert!(
                    result.is_ok(),
                    "builtin template {}/{} failed to render: {}",
                    language.as_str(),
                    key.as_str(),
                    result
                        .as_ref()
                        .err()
                        .map(|e| e.message.as_str())
                        .unwrap_or_default()
                );
                let rendered = result.ok().unwrap_or_default();
                for sentinel in doctor_smoke_expectations(key) {
                    assert!(
                        rendered.contains(sentinel),
                        "builtin template {}/{} rendered but is missing expected sentinel {:?}:\n{}",
                        language.as_str(),
                        key.as_str(),
                        sentinel,
                        rendered
                    );
                }
            }
        }
    }

    /// The Review UI's half of the same promise, through the engine that
    /// serves it.
    ///
    /// The review templates left the loop above when doctor stopped rendering
    /// them one at a time — but the reason that loop exists is that a language
    /// pack nobody runs locally can carry a Jinja error to a release, and that
    /// reason did not change. So every language builds a real workspace and
    /// renders every surface the way `review` would: the page, its includes,
    /// the inlined stylesheet, the markup guard and the element contract.
    #[cfg(feature = "review")]
    #[test]
    fn every_builtin_review_surface_renders_in_every_language() {
        for language in TemplateLanguage::ALL {
            let temp = tempfile::tempdir().expect("a temp workspace");
            let workspace = crate::store::Workspace::at(temp.path().to_path_buf());
            workspace.init().expect("the workspace initializes");
            workspace
                .config_set("workspace.language_bcp47", &[language.as_str().to_string()])
                .expect("the workspace language is set");

            for ui_kind in crate::review_frontend::UI_KINDS {
                let frontend = crate::review_frontend::review_ui_frontend(temp.path(), ui_kind)
                    .expect("afmail's own Review UI resolves");
                let model = doctor_review_context(ui_kind).expect("a smoke document");
                let rendered = crate::runner::render_review_page_from(
                    &workspace,
                    &crate::templates::json_value_to_minijinja(&model),
                    DOCTOR_SMOKE_NONCE,
                    &frontend,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "builtin {} review page in {} failed to render: {}",
                        ui_kind,
                        language.as_str(),
                        error.message
                    )
                });
                for sentinel in doctor_review_expectations(ui_kind) {
                    assert!(
                        rendered.contains(sentinel),
                        "builtin {} review page in {} rendered but is missing {:?}",
                        ui_kind,
                        language.as_str(),
                        sentinel
                    );
                }
            }
        }
    }
}
