use super::*;
use crate::types::ContactFrontmatter;
use serde::Serialize;

pub enum ContactExtractSource {
    FromTriage,
    FromCase(String),
    All,
}

/// Fields for `contact_create`, grouped so the entry point stays a single
/// argument instead of a long positional list.
pub struct NewContact<'a> {
    pub name: &'a str,
    pub group: Option<&'a str>,
    pub emails: &'a [String],
    pub phones: &'a [String],
    pub org: Option<&'a str>,
    pub role: Option<&'a str>,
    pub tags: &'a [String],
    pub reason: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ContactIndexDiagnostic {
    pub code: String,
    pub message: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicting_contact_uids: Vec<String>,
}

struct ContactEmailIndex {
    map: BTreeMap<String, ContactFrontmatter>,
    diagnostics: Vec<ContactIndexDiagnostic>,
}

fn contact_default_body(
    root: &Path,
    language: TemplateLanguage,
    group: &str,
    path: &Path,
    frontmatter: &ContactFrontmatter,
) -> Result<String> {
    render_template(
        root,
        language,
        TemplateKey::ContactDefault,
        &ContactRenderDocument {
            schema_name: "afmail_contact_render_document".to_string(),
            schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
            locator: ContactLocator {
                group: group.to_string(),
                contact_path: rel_path(root, path),
            },
            contact: frontmatter.clone(),
            notes_markdown: String::new(),
        },
    )
}

impl Workspace {
    pub fn contact_create(&self, params: NewContact<'_>) -> Result<Value> {
        let NewContact {
            name,
            group,
            emails,
            phones,
            org,
            role,
            tags,
            reason,
        } = params;
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        validate_name("contact_name", name)?;
        let config = MailConfig::load(&self.root)?;
        let group = group.unwrap_or(config.contact.default_group.as_str());
        validate_id("group", group)?;

        let normalized_emails: Vec<String> = emails
            .iter()
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();

        let contact_index = self.strict_contact_email_map()?;
        for email in &normalized_emails {
            assert_email_unique_in_map(&contact_index, email, None)?;
        }

        let date = workspace_local_date(&config.resolved_timezone_offset());
        let uid = self.next_contact_uid_for_date(&date)?;
        let contact_dir = self.root.join(layout::CONTACTS).join(group);
        create_dir_all(&contact_dir)?;

        let file_name = format!("{}.md", contact_file_name(&uid, name));
        let path = contact_dir.join(&file_name);

        let now = now_rfc3339();
        let mut fm = ContactFrontmatter::new(&uid, name, &now);
        fm.emails = normalized_emails.clone();
        fm.phones = phones.to_vec();
        fm.organization = org.unwrap_or("").to_string();
        fm.role = role.unwrap_or("").to_string();
        fm.tags = tags.to_vec();

        let default_notes =
            contact_default_body(&self.root, config.template_language(), group, &path, &fm)?;
        write_contact_md(&path, &fm, &default_notes)?;

        let contact_path = rel_path(&self.root, &path);
        self.append_audit_event(
            "contact_created",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "display_name": name,
                "group": group,
                "emails": normalized_emails,
            }),
        )?;
        if !normalized_emails.is_empty() {
            self.refresh_views_after_contact_change()?;
        }
        Ok(json!({
            "contact_uid": uid,
            "display_name": name,
            "group": group,
            "emails": normalized_emails,
            "contact_path": contact_path,
        }))
    }

    pub fn contact_list(
        &self,
        group: Option<&str>,
        tag: Option<&str>,
        org: Option<&str>,
        role: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let entries = self.all_active_contact_entries()?;
        let mut contacts = Vec::new();
        for (entry_group, uid, path) in entries {
            if let Some(g) = group
                && entry_group != g
            {
                continue;
            }
            let content = read_to_string(&path, "read contact file")?;
            let (fm, _) = read_contact_md_content(&content)?;
            if let Some(t) = tag
                && !fm.tags.iter().any(|x| x == t)
            {
                continue;
            }
            if let Some(o) = org
                && fm.organization != o
            {
                continue;
            }
            if let Some(r) = role
                && fm.role != r
            {
                continue;
            }
            contacts.push(json!({
                "contact_uid": uid,
                "display_name": fm.display_name,
                "group": entry_group,
                "emails": fm.emails,
                "phones": fm.phones,
                "organization": fm.organization,
                "role": fm.role,
                "tags": fm.tags,
                "contact_path": rel_path(&self.root, &path),
            }));
        }
        let count = contacts.len();
        Ok(json!({
            "contacts": contacts,
            "count": count,
        }))
    }

    pub fn contact_show(&self, contact_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let (group, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact file")?;
        let (contact, notes_markdown) = read_contact_md_content(&content)?;
        serde_json::to_value(contact_render_document(
            &self.root,
            &group,
            &path,
            contact,
            notes_markdown,
        ))
        .map_err(|error| AppError::json("serialize contact render document", &error))
    }

    pub fn contact_move(
        &self,
        contact_ref: &str,
        new_group: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        validate_id("group", new_group)?;
        let (group, src_path) = self.find_active_contact_path(&uid)?;
        if group == new_group {
            return Ok(json!({
                "contact_uid": uid,
                "group": new_group,
                "contact_path": rel_path(&self.root, &src_path),
                "changed": false,
            }));
        }
        let dest_dir = self.root.join(layout::CONTACTS).join(new_group);
        create_dir_all(&dest_dir)?;
        let file_name = path_file_name(&src_path);
        let dest_path = dest_dir.join(&file_name);
        fs::rename(&src_path, &dest_path).map_err(|e| AppError::io("move contact", &e))?;
        // Remove the now-empty source group dir if nothing is left.
        let src_group_dir = self.root.join(layout::CONTACTS).join(&group);
        let _ = fs::remove_dir(&src_group_dir);
        self.append_audit_event(
            "contact_moved",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "from_group": group,
                "to_group": new_group,
                "changed": true
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "group": new_group,
            "contact_path": rel_path(&self.root, &dest_path),
            "changed": true,
        }))
    }

    pub fn contact_rename(
        &self,
        contact_ref: &str,
        new_name: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        validate_name("contact_name", new_name)?;
        let (group, src_path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&src_path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        let old_name = fm.display_name.clone();
        fm.display_name = new_name.to_string();
        fm.updated_rfc3339 = now_rfc3339();

        let new_file_name = format!("{}.md", contact_file_name(&uid, new_name));
        let dest_path = src_path.parent().unwrap_or(&self.root).join(&new_file_name);
        write_contact_md(&dest_path, &fm, &body)?;
        if src_path != dest_path {
            remove_file(&src_path)?;
        }
        self.append_audit_event(
            "contact_renamed",
            vec![audit_target("contact", &uid)],
            reason,
            json!({"contact_uid": uid, "old_name": old_name, "new_name": new_name}),
        )?;
        if !fm.emails.is_empty() {
            self.refresh_views_after_contact_change()?;
        }
        Ok(json!({
            "contact_uid": uid,
            "display_name": new_name,
            "group": group,
            "contact_path": rel_path(&self.root, &dest_path),
        }))
    }

    pub fn contact_email_add(
        &self,
        contact_ref: &str,
        email: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        let normalized = email.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(AppError::new("invalid_request", "email must not be empty"));
        }
        let contact_index = self.strict_contact_email_map()?;
        assert_email_unique_in_map(&contact_index, &normalized, Some(&uid))?;
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        if !fm
            .emails
            .iter()
            .any(|e| e.to_ascii_lowercase() == normalized)
        {
            fm.emails.push(normalized.clone());
            fm.updated_rfc3339 = now_rfc3339();
            write_contact_md(&path, &fm, &body)?;
            self.refresh_views_after_contact_change()?;
        }
        self.append_audit_event(
            "contact_email_added",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "email": normalized,
                "emails": fm.emails,
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "email": normalized,
            "emails": fm.emails,
        }))
    }

    pub fn contact_email_remove(
        &self,
        contact_ref: &str,
        email: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        let normalized = email.trim().to_ascii_lowercase();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        let before = fm.emails.len();
        fm.emails.retain(|e| e.to_ascii_lowercase() != normalized);
        if fm.emails.len() != before {
            fm.updated_rfc3339 = now_rfc3339();
            write_contact_md(&path, &fm, &body)?;
            self.refresh_views_after_contact_change()?;
        }
        self.append_audit_event(
            "contact_email_removed",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "email": normalized,
                "emails": fm.emails,
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "email": normalized,
            "emails": fm.emails,
        }))
    }

    pub fn contact_phone_add(
        &self,
        contact_ref: &str,
        phone: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        let phone = phone.trim().to_string();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        if !fm.phones.iter().any(|p| p == &phone) {
            fm.phones.push(phone.clone());
            fm.updated_rfc3339 = now_rfc3339();
            write_contact_md(&path, &fm, &body)?;
        }
        self.append_audit_event(
            "contact_phone_added",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "phone": phone,
                "phones": fm.phones,
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "phone": phone,
            "phones": fm.phones,
        }))
    }

    pub fn contact_phone_remove(
        &self,
        contact_ref: &str,
        phone: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        let phone = phone.trim().to_string();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        let before = fm.phones.len();
        fm.phones.retain(|p| p != &phone);
        if fm.phones.len() != before {
            fm.updated_rfc3339 = now_rfc3339();
            write_contact_md(&path, &fm, &body)?;
        }
        self.append_audit_event(
            "contact_phone_removed",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "phone": phone,
                "phones": fm.phones,
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "phone": phone,
            "phones": fm.phones,
        }))
    }

    pub fn contact_tag(&self, contact_ref: &str, tag: &str, reason: Option<&str>) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        let tag = tag.trim().to_string();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        merge_string(&mut fm.tags, &tag);
        fm.updated_rfc3339 = now_rfc3339();
        write_contact_md(&path, &fm, &body)?;
        self.append_audit_event(
            "contact_tagged",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "tag": tag,
                "tags": fm.tags,
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "tag": tag,
            "tags": fm.tags,
        }))
    }

    pub fn contact_untag(
        &self,
        contact_ref: &str,
        tag: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let reason = self.checked_reason(reason)?;
        let uid = parse_contact_ref(contact_ref)?;
        let tag = tag.trim().to_string();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        fm.tags.retain(|t| t != &tag);
        fm.updated_rfc3339 = now_rfc3339();
        write_contact_md(&path, &fm, &body)?;
        self.append_audit_event(
            "contact_untagged",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "tag": tag,
                "tags": fm.tags,
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "tag": tag,
            "tags": fm.tags,
        }))
    }

    pub fn contact_notes_show(&self, contact_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let (group, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (contact, notes_markdown) = read_contact_md_content(&content)?;
        serde_json::to_value(contact_render_document(
            &self.root,
            &group,
            &path,
            contact,
            notes_markdown,
        ))
        .map_err(|error| AppError::json("serialize contact notes document", &error))
    }

    pub fn contact_notes_append(&self, contact_ref: &str, text: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        let new_body = if body.trim().is_empty() {
            format!("{}\n", text.trim_end())
        } else {
            format!("{}\n\n{}\n", body.trim_end(), text.trim_end())
        };
        fm.updated_rfc3339 = now_rfc3339();
        write_contact_md(&path, &fm, &new_body)?;
        self.append_audit_event(
            "contact_notes_appended",
            vec![audit_target("contact", &uid)],
            None,
            json!({
                "operation": "append",
                "notes_path": rel_path(&self.root, &path),
                "text_len_bytes": text.len(),
                "text_hash": stable_text_hash(text),
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "notes_path": rel_path(&self.root, &path),
            "text_len_bytes": text.len(),
            "text_hash": stable_text_hash(text),
        }))
    }

    pub fn contact_notes_replace(&self, contact_ref: &str, text: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, _) = read_contact_md_content(&content)?;
        let new_body = format!("{}\n", text.trim_end());
        fm.updated_rfc3339 = now_rfc3339();
        write_contact_md(&path, &fm, &new_body)?;
        self.append_audit_event(
            "contact_notes_replaced",
            vec![audit_target("contact", &uid)],
            None,
            json!({
                "operation": "replace",
                "notes_path": rel_path(&self.root, &path),
                "text_len_bytes": text.len(),
                "text_hash": stable_text_hash(text),
            }),
        )?;
        Ok(json!({
            "contact_uid": uid,
            "notes_path": rel_path(&self.root, &path),
            "text_len_bytes": text.len(),
            "text_hash": stable_text_hash(text),
        }))
    }

    pub fn contact_delete(
        &self,
        contact_ref: &str,
        allow_notes: bool,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let reason = self.checked_reason(reason)?;
        let (group, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (fm, body) = read_contact_md_content(&content)?;
        let notes_present = !body.trim().is_empty();
        if notes_present && !allow_notes {
            return Err(AppError::new(
                "contact_notes_present",
                format!("contact {uid} has body content"),
            )
            .with_hint(
                "Inspect the contact notes, then rerun with `--allow-notes` if deleting them is intended.",
            )
            .with_details(json!({
                "contact_uid": uid,
                "contact_path": rel_path(&self.root, &path),
                "notes_len_bytes": body.len(),
                "notes_hash": stable_text_hash(&body),
                "suggested_commands": [
                    format!("afmail contact notes show {uid}"),
                    format!("afmail contact delete {uid} --allow-notes --reason TEXT")
                ],
            })));
        }
        let contact_path = rel_path(&self.root, &path);
        remove_file(&path)?;
        let group_dir = self.root.join(layout::CONTACTS).join(&group);
        let _ = fs::remove_dir(&group_dir);
        self.append_audit_event(
            "contact_deleted",
            vec![audit_target("contact", &uid)],
            reason,
            json!({
                "contact_uid": uid,
                "display_name": fm.display_name,
                "group": group,
                "contact_path": contact_path,
                "notes_present": notes_present,
                "notes_len_bytes": body.len(),
                "notes_hash": stable_text_hash(&body),
            }),
        )?;
        if !fm.emails.is_empty() {
            self.refresh_views_after_contact_change()?;
        }
        Ok(json!({
            "contact_uid": uid,
            "display_name": fm.display_name,
            "group": group,
            "contact_path": contact_path,
            "deleted": true,
            "notes_present": notes_present,
        }))
    }

    pub fn contact_extract(
        &self,
        source: ContactExtractSource,
        group: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let config = MailConfig::load(&self.root)?;
        let offset = config.resolved_timezone_offset();
        let target_group = group.unwrap_or(config.contact.default_group.as_str());
        validate_id("group", target_group)?;

        let from_headers = match source {
            ContactExtractSource::FromTriage => self.collect_from_headers_triage()?,
            ContactExtractSource::FromCase(case_ref) => {
                let uid = parse_case_ref(&case_ref)?;
                self.collect_from_headers_case(&uid)?
            }
            ContactExtractSource::All => {
                let mut headers = self.collect_from_headers_triage()?;
                for (case_uid, _) in self.all_case_entries()? {
                    headers.extend(self.collect_from_headers_case(&case_uid)?);
                }
                headers
            }
        };

        let mut contact_index = self.strict_contact_email_map()?;
        let mut created_uids: Vec<String> = Vec::new();
        let mut skipped_count = 0usize;

        for (display_name, email) in from_headers {
            let email_lc = email.to_ascii_lowercase();
            if contact_index.contains_key(&email_lc) {
                skipped_count += 1;
                continue;
            }
            let date = workspace_local_date(&offset);
            let uid = self.next_contact_uid_for_date(&date)?;
            let contact_dir = self.root.join(layout::CONTACTS).join(target_group);
            create_dir_all(&contact_dir)?;
            let file_name = format!(
                "{}.md",
                contact_file_name(
                    &uid,
                    if display_name.is_empty() {
                        &email_lc
                    } else {
                        &display_name
                    }
                )
            );
            let path = contact_dir.join(&file_name);
            let now = now_rfc3339();
            let actual_name = if display_name.is_empty() {
                email_lc.clone()
            } else {
                display_name.clone()
            };
            let mut fm = ContactFrontmatter::new(&uid, &actual_name, &now);
            fm.emails = vec![email_lc.clone()];
            let default_notes = contact_default_body(
                &self.root,
                config.template_language(),
                target_group,
                &path,
                &fm,
            )?;
            write_contact_md(&path, &fm, &default_notes)?;
            contact_index.insert(email_lc.clone(), fm.clone());
            self.append_audit_event(
                "contact_created",
                vec![audit_target("contact", &uid)],
                None,
                json!({"contact_uid": uid, "display_name": actual_name, "group": target_group, "emails": [&email_lc], "via": "extract"}),
            )?;
            created_uids.push(uid);
        }

        let created_count = created_uids.len();
        if created_count > 0 {
            self.refresh_views_after_contact_change()?;
        }
        Ok(json!({
            "created_count": created_count,
            "skipped_count": skipped_count,
            "created_uids": created_uids,
        }))
    }

    /// Build an ephemeral email→contact map from active contact files. Replaces
    /// the former persisted email index.
    pub(super) fn contact_email_map(&self) -> Result<BTreeMap<String, ContactFrontmatter>> {
        self.strict_contact_email_map()
    }

    pub(super) fn contact_index_diagnostics(&self) -> Result<Vec<ContactIndexDiagnostic>> {
        Ok(self.build_contact_email_index()?.diagnostics)
    }

    fn strict_contact_email_map(&self) -> Result<BTreeMap<String, ContactFrontmatter>> {
        let index = self.build_contact_email_index()?;
        if !index.diagnostics.is_empty() {
            return Err(contact_index_invalid_error(&index.diagnostics));
        }
        Ok(index.map)
    }

    fn build_contact_email_index(&self) -> Result<ContactEmailIndex> {
        let mut map: BTreeMap<String, ContactFrontmatter> = BTreeMap::new();
        let mut diagnostics = Vec::new();
        for (_, entry_uid, path) in self.all_active_contact_entries()? {
            let rel = rel_path(&self.root, &path);
            let content = read_to_string(&path, "read contact for index")?;
            let (fm, _) = match read_contact_md_content(&content) {
                Ok(parsed) => parsed,
                Err(err) => {
                    diagnostics.push(ContactIndexDiagnostic {
                        code: "contact_invalid".to_string(),
                        message: err.message,
                        path: rel,
                        contact_uid: Some(entry_uid),
                        email: None,
                        conflicting_contact_uids: Vec::new(),
                    });
                    continue;
                }
            };
            if fm.contact_uid != entry_uid {
                diagnostics.push(ContactIndexDiagnostic {
                    code: "contact_uid_mismatch".to_string(),
                    message: format!(
                        "contact file name uid {entry_uid} does not match frontmatter uid {}",
                        fm.contact_uid
                    ),
                    path: rel.clone(),
                    contact_uid: Some(fm.contact_uid.clone()),
                    email: None,
                    conflicting_contact_uids: vec![entry_uid.clone(), fm.contact_uid.clone()],
                });
            }
            for email in &fm.emails {
                let normalized = email.trim().to_ascii_lowercase();
                if normalized.is_empty() {
                    continue;
                }
                if let Some(existing) = map.get(&normalized) {
                    diagnostics.push(ContactIndexDiagnostic {
                        code: "contact_email_duplicate".to_string(),
                        message: format!(
                            "email {normalized} is assigned to contacts {} and {}",
                            existing.contact_uid, fm.contact_uid
                        ),
                        path: rel.clone(),
                        contact_uid: Some(fm.contact_uid.clone()),
                        email: Some(normalized.clone()),
                        conflicting_contact_uids: vec![
                            existing.contact_uid.clone(),
                            fm.contact_uid.clone(),
                        ],
                    });
                    continue;
                }
                map.insert(normalized, fm.clone());
            }
        }
        Ok(ContactEmailIndex { map, diagnostics })
    }

    /// Regenerate every read view that embeds the message section so newly
    /// materialized contact links propagate. `refresh_triage_views` recomputes
    /// `message.contact` for every message JSON first; the case/disposition/
    /// archive refreshers then read the updated JSON.
    pub(super) fn refresh_views_after_contact_change(&self) -> Result<()> {
        self.refresh_triage_views()?;
        self.refresh_disposition_views()?;
        self.refresh_all_case_message_views()?;
        self.refresh_notification_indexes()?;
        Ok(())
    }

    fn next_contact_uid_for_date(&self, date: &str) -> Result<String> {
        next_uid_for_date(
            'p',
            date,
            self.all_active_contact_entries()?
                .into_iter()
                .map(|(_, uid, _)| uid),
        )
    }

    fn all_active_contact_entries(&self) -> Result<Vec<(String, String, PathBuf)>> {
        let contacts_dir = self.root.join(layout::CONTACTS);
        if !contacts_dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for group_entry in read_dir(&contacts_dir, "list contact groups")? {
            if !group_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let group = group_entry.file_name().to_string_lossy().to_string();
            for file_entry in read_dir(&group_entry.path(), "list contact files")? {
                let path = file_entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(uid) = contact_uid_from_file_name(&stem) {
                    entries.push((group.clone(), uid, path));
                }
            }
        }
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        Ok(entries)
    }

    fn find_active_contact_path(&self, uid: &str) -> Result<(String, PathBuf)> {
        for (group, entry_uid, path) in self.all_active_contact_entries()? {
            if entry_uid == uid {
                return Ok((group, path));
            }
        }
        Err(
            AppError::new("not_found", format!("active contact {uid} not found"))
                .with_hint("Use `afmail contact list` to see available contacts.")
                .with_details(json!({"contact_uid": uid})),
        )
    }

    fn collect_from_headers_triage(&self) -> Result<Vec<(String, String)>> {
        let triage_dir = self.root.join(layout::TRIAGE);
        if !triage_dir.exists() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for entry in read_dir(&triage_dir, "list triage")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(message_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Some(pair) = self.message_sender_pair(message_id) {
                results.push(pair);
            }
        }
        Ok(results)
    }

    fn collect_from_headers_case(&self, case_uid: &str) -> Result<Vec<(String, String)>> {
        let (_, case_path) = self.find_active_case_entry(case_uid)?;
        let case_data = read_case_file(&case_path)?;
        let mut results = Vec::new();
        for item in &case_data.items {
            if let Some(pair) = self.message_sender_pair(&item.message_id) {
                results.push(pair);
            }
        }
        Ok(results)
    }

    fn message_sender_pair(&self, message_id: &str) -> Option<(String, String)> {
        let msg_path = self
            .root
            .join(layout::MESSAGES)
            .join(format!("{message_id}.json"));
        let content = fs::read_to_string(&msg_path).ok()?;
        let msg: Value = serde_json::from_str(&content).ok()?;
        let from = msg["from"].as_str()?;
        let email = email_address(from);
        if email.is_empty() {
            return None;
        }
        let display_name = extract_from_display_name(from);
        Some((display_name, email))
    }

    fn find_active_case_entry(&self, case_uid: &str) -> Result<(String, PathBuf)> {
        for (uid, path) in self.all_case_entries()? {
            if uid == case_uid {
                return Ok((uid, path));
            }
        }
        Err(AppError::new(
            "not_found",
            format!("case {case_uid} not found"),
        ))
    }
}

fn assert_email_unique_in_map(
    map: &BTreeMap<String, ContactFrontmatter>,
    email: &str,
    excluding_uid: Option<&str>,
) -> Result<()> {
    let normalized = email.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(());
    }
    if let Some(entry) = map.get(&normalized)
        && excluding_uid != Some(entry.contact_uid.as_str())
    {
        return Err(AppError::new(
            "email_conflict",
            format!(
                "email {email} is already assigned to contact {}",
                entry.contact_uid
            ),
        )
        .with_details(json!({"email": email, "existing_contact_uid": entry.contact_uid})));
    }
    Ok(())
}

fn contact_index_invalid_error(diagnostics: &[ContactIndexDiagnostic]) -> AppError {
    AppError::new(
        "contact_index_invalid",
        "contact index has invalid or conflicting contact files",
    )
    .with_hint("Run `afmail doctor` to inspect contact file errors before changing contacts.")
    .with_details(json!({
        "diagnostic_count": diagnostics.len(),
        "diagnostics": diagnostics,
    }))
}

/// Resolve the contact whose email set includes a message's `From` header,
/// using a prebuilt email→contact map (see `contact_email_map`).
pub(super) fn contact_for_from(
    map: &BTreeMap<String, ContactFrontmatter>,
    from_header: &str,
) -> Option<ContactFrontmatter> {
    if from_header.is_empty() {
        return None;
    }
    let email = email_address(from_header);
    if email.is_empty() {
        return None;
    }
    map.get(&email).cloned()
}

fn write_contact_md(path: &Path, fm: &ContactFrontmatter, body: &str) -> Result<()> {
    let rendered = crate::markdown::render_frontmatter(fm, body)?;
    write_string(path, &rendered)
}

fn read_contact_md_content(content: &str) -> Result<(ContactFrontmatter, String)> {
    crate::markdown::read_doc::<ContactFrontmatter>(content)
        .map_err(|e| AppError::new("invalid_contact", format!("parse contact file: {e}")))
}

fn contact_render_document(
    root: &Path,
    group: &str,
    path: &Path,
    contact: ContactFrontmatter,
    notes_markdown: String,
) -> ContactRenderDocument {
    ContactRenderDocument {
        schema_name: "afmail_contact_render_document".to_string(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        locator: ContactLocator {
            group: group.to_string(),
            contact_path: rel_path(root, path),
        },
        contact,
        notes_markdown,
    }
}
