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
                    "messages": true,
                    "contacts": true,
                    "cases": true,
                    "notifications": true,
                    "push_queue": true,
                    "templates": true,
                    "transactions": true,
                    "state_layout": true,
                },
                "issues": issues,
            }),
            succeeded_count,
            failed_count,
        ))
    }

    pub fn doctor_repair(&self, confirm: bool) -> Result<crate::types::CommandResult> {
        self.require_workspace_directory()?;
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
        let before = self.doctor_issues()?;
        self.repair_incomplete_transactions()?;
        let deprecated_state_removed_count = self.remove_deprecated_message_state_sidecars()?;
        let cache = self.rebuild_message_cache_from_eml()?;
        for path in message_json_paths(&self.root)? {
            if let Ok(message) = read_message(&path) {
                self.persist_message_remote(&message)?;
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
        self.check_state_layout(&mut issues)?;
        self.check_transactions(&mut issues)?;
        self.check_idempotency_requests(&mut issues)?;
        self.check_messages(&mut issues)?;
        self.check_contacts(&mut issues)?;
        self.check_case_refs(&mut issues)?;
        self.check_notification_refs(&mut issues)?;
        self.check_push_overlay(&mut issues)?;
        self.check_draft_queue_state(&mut issues)?;
        self.check_templates(&mut issues)?;
        Ok(issues)
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
            issue.repairable = transaction.is_repairable();
            issues.push(issue);
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
            if name.ends_with(".state.json") {
                issues.push(DoctorIssue::warning(
                    "message_state_deprecated",
                    format!("deprecated message state sidecar: {name}"),
                    Some(rel_path(&self.root, &path)),
                    true,
                ));
                continue;
            }
            if let Some(message_id) = name.strip_suffix(".remote.json")
                && !ids.contains(message_id)
                && !self.message_path(message_id).is_file()
            {
                issues.push(DoctorIssue::warning(
                    "message_sidecar_orphaned",
                    format!("sidecar has no materialized message cache: {message_id}"),
                    Some(rel_path(&self.root, &path)),
                    true,
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
pub(super) const DOCTOR_SMOKE_LINK: &str = "../../../triage/message_doctor_9f3c.md";
pub(super) const DOCTOR_SMOKE_CONVERSATION: &str = "Doctor smoke conversation message_doctor_9f3c.";
pub(super) const DOCTOR_SMOKE_OTHER_CASE_UID: &str = "c20260609002";
pub(super) const DOCTOR_SMOKE_OTHER_BODY: &str = "Doctor smoke merge body.";
pub(super) const DOCTOR_SMOKE_QUOTED: &str = "Doctor smoke quoted body.";

// Mirrors the real shape `thread_item_common` (store/render.rs) produces, so
// every field a locale pack's case/notification/status templates might key
// off of (direction_kind OR the richer action_kind) is present.
fn doctor_smoke_item(direction_kind: &str, action: &str) -> Value {
    let is_received = direction_kind == "received";
    let action_kind = match (is_received, action) {
        (true, "reply") => "received_reply",
        (true, "forward") => "received_forward",
        (true, _) => "received",
        (false, "reply") => "sent_reply",
        (false, "forward") => "sent_forward",
        (false, _) => "sent",
    };
    json!({
        "message": {"message_id": DOCTOR_SMOKE_MESSAGE_ID},
        "view": {
            "direction": if is_received { "inbound" } else { "outbound" },
            "direction_kind": direction_kind,
            "direction_symbol": if is_received { "\u{2190}" } else { "\u{2192}" },
            "action": action,
            "action_kind": action_kind,
            "is_reply": action == "reply",
            "is_forward": action == "forward",
            "contact_kind": if is_received { "from" } else { "to" },
            "contact": "doctor-smoke@example.com",
            "display_contact": "doctor-smoke@example.com",
            "display_subject": "Doctor smoke subject",
            "title": "Doctor smoke subject",
            "display_title": "Doctor smoke subject",
            "display_status": "triage",
            "link": DOCTOR_SMOKE_LINK,
            "time": {"datetime": "2026-06-09T00:00:00Z"},
            "time_rfc3339": "2026-06-09T00:00:00Z",
            "display_time": "2026-06-09T00:00:00Z",
        },
    })
}

fn minimal_template_context(language: TemplateLanguage) -> Value {
    json!({
        "items": [
            doctor_smoke_item("received", "reply"),
            doctor_smoke_item("sent", "forward"),
        ],
        "messages": [],
        "workspaces": ["."],
        "other_case_uid": DOCTOR_SMOKE_OTHER_CASE_UID,
        "other_body": DOCTOR_SMOKE_OTHER_BODY,
        "sender": "Doctor Smoke Sender",
        "quoted": DOCTOR_SMOKE_QUOTED,
        "frontmatter": {
            "kind": "doctor",
            "message_id": DOCTOR_SMOKE_MESSAGE_ID,
            "message_ids": [DOCTOR_SMOKE_MESSAGE_ID],
            "case_uid": DOCTOR_SMOKE_CASE_UID,
            "case_name": "doctor",
            "notification_uid": DOCTOR_SMOKE_NOTIFICATION_UID,
            "notification_name": "doctor",
            "status": "active",
            "message_count": 1,
            "attachment_count": 1,
            "generated_rfc3339": "2026-06-09T00:00:00Z",
            "added_rfc3339": "2026-06-09T00:00:00Z",
            "suggested_case_uids": [DOCTOR_SMOKE_CASE_UID],
            "suggested_reason": "doctor smoke reason",
            "suggested_reason_yaml": "doctor smoke reason",
        },
        "message": {
            "schema_name": "message",
            "schema_version": 1,
            "message_id": DOCTOR_SMOKE_MESSAGE_ID,
            "from": "doctor-smoke-from@example.com",
            "subject": "Doctor smoke subject",
            "to": ["doctor-smoke-to@example.com"],
            "cc": ["doctor-smoke-cc@example.com"],
            "bcc": [],
            "contact": {"display_name": "Doctor Smoke Contact", "contact_uid": "doctor-smoke-contact"},
            "identity_match": "me",
            "identity": "me",
            "identity_email": "me@example.com",
            "identity_candidates": ["me"],
            "observed_recipient_emails": ["doctor-smoke-to@example.com"],
            "attachments": [],
            "workspace": {"status": "triage"},
        },
        "view": {
            "language": language.as_str(),
            "title": "doctor",
            "display_title": "doctor",
            "status": "active",
            "message_count": 1,
            "attachment_count": 1,
            "generated_rfc3339": "2026-06-09T00:00:00Z",
            "added_rfc3339": "2026-06-09T00:00:00Z",
            "summary": "doctor smoke summary",
            "conversation": DOCTOR_SMOKE_CONVERSATION,
            "related_messages": [
                {
                    "message": {"message_id": DOCTOR_SMOKE_MESSAGE_ID},
                    "view": {
                        "direction": "inbound",
                        "from": "doctor-smoke-from@example.com",
                        "display_contact": "doctor-smoke-from@example.com",
                        "subject": "Doctor smoke subject",
                        "display_subject": "Doctor smoke subject",
                        "time": "2026-06-09T00:00:00Z",
                        "status": "triage",
                    },
                },
            ],
            "suggested_case_uids": [DOCTOR_SMOKE_CASE_UID],
            "suggested_reason": "doctor smoke reason",
            "message_action": "received",
            "display_counterparty": "Doctor Smoke Counterparty",
            "display_time": "2026-06-09T00:00:00Z",
            "body_text_visible_block": "Doctor smoke body.\n",
            "body_text_fence": "```",
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
            "attachments": [
                {
                    "display_filename": "doctor-smoke.png",
                    "content_type": "image/png",
                    "size_bytes": 128,
                    "part_id": "2",
                    "file_path": ".afmail/messages/message_doctor_9f3c.files/doctor-smoke.png",
                    "preview_path": "../../../.afmail/messages/message_doctor_9f3c.files/doctor-smoke.png",
                    "image_alt": "doctor-smoke.png",
                },
            ],
        },
        "config": {
            "notification": {
                "message_index": {
                    "item_fields": [],
                },
            },
        },
        "notification": {
            "notification_uid": DOCTOR_SMOKE_NOTIFICATION_UID,
            "notification_name": "doctor",
        },
        "case": {
            "case_uid": DOCTOR_SMOKE_CASE_UID,
            "case_name": "doctor",
            "collection_uid": DOCTOR_SMOKE_CASE_UID,
            "collection_name": "doctor",
            "status": "active",
        },
        "item": {},
    })
}

// Sentinel strings a correctly-wired render of `key` must surface. A template
// that renders without error but references the wrong field (e.g. a sibling
// key that happens to also resolve) would pass a bare `is_ok()` check but
// fail here, since the expected identifying value never made it into text.
#[cfg(test)]
fn doctor_smoke_expectations(key: TemplateKey) -> &'static [&'static str] {
    match key {
        TemplateKey::WorkspaceAgents
        | TemplateKey::WorkspaceDoNotEdit
        | TemplateKey::NotesDefault
        | TemplateKey::DraftNew => &[],
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
        TemplateKey::NotificationIndex => &[
            DOCTOR_SMOKE_MESSAGE_ID,
            DOCTOR_SMOKE_LINK,
            DOCTOR_SMOKE_NOTIFICATION_UID,
        ],
        TemplateKey::NotificationMessage => &[
            DOCTOR_SMOKE_MESSAGE_ID,
            DOCTOR_SMOKE_NOTIFICATION_UID,
            DOCTOR_SMOKE_CONVERSATION,
        ],
        TemplateKey::StatusIndex => &[DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_LINK],
        TemplateKey::StatusMessage => &[DOCTOR_SMOKE_MESSAGE_ID, DOCTOR_SMOKE_CONVERSATION],
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
            let mut renderer = MarkdownTemplateRenderer::builtin(language);
            for key in TemplateKey::ALL {
                let result = renderer.render(key, &minimal_template_context(language));
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
}
