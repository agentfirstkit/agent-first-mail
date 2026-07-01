use super::*;
use crate::types::{ContactFrontmatter, MessageContact};

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
        } = params;
        self.require_workspace()?;
        validate_name("contact_name", name)?;
        let config = MailConfig::load(&self.root)?;
        let group = group.unwrap_or(config.contact.default_group.as_str());
        validate_id("group", group)?;

        let normalized_emails: Vec<String> = emails
            .iter()
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();

        for email in &normalized_emails {
            self.assert_email_unique(email, None)?;
        }

        let date = workspace_local_date(&config.resolved_timezone_offset());
        let uid = self.next_contact_uid_for_date(&date)?;
        let contact_dir = self.root.join("contacts").join(group);
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

        let default_notes = format!("## {name}\n\n");
        write_contact_md(&path, &fm, &default_notes)?;

        let contact_path = rel_path(&self.root, &path);
        self.append_audit_event(
            "contact_created",
            vec![audit_target("contact", &uid)],
            None,
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
            "code": "contact_created",
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
            if let Some(g) = group {
                if entry_group != g {
                    continue;
                }
            }
            let content = read_to_string(&path, "read contact file")?;
            let (fm, _) = read_contact_md_content(&content)?;
            if let Some(t) = tag {
                if !fm.tags.iter().any(|x| x == t) {
                    continue;
                }
            }
            if let Some(o) = org {
                if fm.organization != o {
                    continue;
                }
            }
            if let Some(r) = role {
                if fm.role != r {
                    continue;
                }
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
            "code": "contact_list",
            "contacts": contacts,
            "count": count,
        }))
    }

    pub fn contact_show(&self, contact_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let (is_archived, path) = self.find_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact file")?;
        let group = if is_archived {
            None
        } else {
            Some(contact_group_from_path(&self.root, &path))
        };
        Ok(json!({
            "code": "contact_show",
            "contact_uid": uid,
            "is_archived": is_archived,
            "group": group,
            "contact_path": rel_path(&self.root, &path),
            "content": content,
        }))
    }

    pub fn contact_move(&self, contact_ref: &str, new_group: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        validate_id("group", new_group)?;
        let (group, src_path) = self.find_active_contact_path(&uid)?;
        if group == new_group {
            return Ok(json!({
                "code": "contact_moved",
                "contact_uid": uid,
                "group": new_group,
                "contact_path": rel_path(&self.root, &src_path),
                "changed": false,
            }));
        }
        let dest_dir = self.root.join("contacts").join(new_group);
        create_dir_all(&dest_dir)?;
        let file_name = path_file_name(&src_path);
        let dest_path = dest_dir.join(&file_name);
        fs::rename(&src_path, &dest_path).map_err(|e| AppError::io("move contact", &e))?;
        // Remove the now-empty source group dir if nothing is left.
        let src_group_dir = self.root.join("contacts").join(&group);
        let _ = fs::remove_dir(&src_group_dir);
        self.append_audit_event(
            "contact_moved",
            vec![audit_target("contact", &uid)],
            None,
            json!({"contact_uid": uid, "from_group": group, "to_group": new_group}),
        )?;
        Ok(json!({
            "code": "contact_moved",
            "contact_uid": uid,
            "group": new_group,
            "contact_path": rel_path(&self.root, &dest_path),
            "changed": true,
        }))
    }

    pub fn contact_rename(&self, contact_ref: &str, new_name: &str) -> Result<Value> {
        self.require_workspace()?;
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
            None,
            json!({"contact_uid": uid, "old_name": old_name, "new_name": new_name}),
        )?;
        if !fm.emails.is_empty() {
            self.refresh_views_after_contact_change()?;
        }
        Ok(json!({
            "code": "contact_renamed",
            "contact_uid": uid,
            "display_name": new_name,
            "group": group,
            "contact_path": rel_path(&self.root, &dest_path),
        }))
    }

    pub fn contact_email_add(&self, contact_ref: &str, email: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let normalized = email.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(AppError::new("invalid_request", "email must not be empty"));
        }
        self.assert_email_unique(&normalized, Some(&uid))?;
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
        Ok(json!({
            "code": "contact_email_added",
            "contact_uid": uid,
            "email": normalized,
            "emails": fm.emails,
        }))
    }

    pub fn contact_email_remove(&self, contact_ref: &str, email: &str) -> Result<Value> {
        self.require_workspace()?;
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
        Ok(json!({
            "code": "contact_email_removed",
            "contact_uid": uid,
            "email": normalized,
            "emails": fm.emails,
        }))
    }

    pub fn contact_phone_add(&self, contact_ref: &str, phone: &str) -> Result<Value> {
        self.require_workspace()?;
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
        Ok(json!({
            "code": "contact_phone_added",
            "contact_uid": uid,
            "phone": phone,
            "phones": fm.phones,
        }))
    }

    pub fn contact_phone_remove(&self, contact_ref: &str, phone: &str) -> Result<Value> {
        self.require_workspace()?;
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
        Ok(json!({
            "code": "contact_phone_removed",
            "contact_uid": uid,
            "phone": phone,
            "phones": fm.phones,
        }))
    }

    pub fn contact_tag(&self, contact_ref: &str, tag: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let tag = tag.trim().to_string();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        merge_string(&mut fm.tags, &tag);
        fm.updated_rfc3339 = now_rfc3339();
        write_contact_md(&path, &fm, &body)?;
        Ok(json!({
            "code": "contact_tagged",
            "contact_uid": uid,
            "tag": tag,
            "tags": fm.tags,
        }))
    }

    pub fn contact_untag(&self, contact_ref: &str, tag: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let tag = tag.trim().to_string();
        let (_, path) = self.find_active_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (mut fm, body) = read_contact_md_content(&content)?;
        fm.tags.retain(|t| t != &tag);
        fm.updated_rfc3339 = now_rfc3339();
        write_contact_md(&path, &fm, &body)?;
        Ok(json!({
            "code": "contact_untagged",
            "contact_uid": uid,
            "tag": tag,
            "tags": fm.tags,
        }))
    }

    pub fn contact_notes_show(&self, contact_ref: &str) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let (_, path) = self.find_contact_path(&uid)?;
        let content = read_to_string(&path, "read contact")?;
        let (_, body) = read_contact_md_content(&content)?;
        Ok(json!({
            "code": "contact_notes_show",
            "contact_uid": uid,
            "notes": body,
        }))
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
        Ok(json!({
            "code": "contact_notes_appended",
            "contact_uid": uid,
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
        Ok(json!({
            "code": "contact_notes_replaced",
            "contact_uid": uid,
        }))
    }

    pub fn contact_archive_contact(
        &self,
        contact_ref: &str,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let reason = self.checked_reason(reason)?;
        let (group, src_path) = self.find_active_contact_path(&uid)?;
        let archive_dir = self.root.join("archive").join("contacts");
        create_dir_all(&archive_dir)?;
        let file_name = path_file_name(&src_path);
        let dest_path = unique_dest_path(&archive_dir, &file_name);
        fs::rename(&src_path, &dest_path).map_err(|e| AppError::io("archive contact", &e))?;
        // Remove empty group dir if nothing left
        let group_dir = self.root.join("contacts").join(&group);
        let _ = fs::remove_dir(&group_dir);
        self.append_audit_event(
            "contact_archived",
            vec![audit_target("contact", &uid)],
            reason,
            json!({"contact_uid": uid, "from_group": group}),
        )?;
        Ok(json!({
            "code": "contact_archived",
            "contact_uid": uid,
            "contact_path": rel_path(&self.root, &dest_path),
        }))
    }

    pub fn contact_reopen_contact(
        &self,
        contact_ref: &str,
        group: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Value> {
        self.require_workspace()?;
        let uid = parse_contact_ref(contact_ref)?;
        let reason = self.checked_reason(reason)?;
        let src_path = self.find_archived_contact_path(&uid)?;
        let config = MailConfig::load(&self.root)?;
        let target_group = group.unwrap_or(config.contact.default_group.as_str());
        validate_id("group", target_group)?;
        let dest_dir = self.root.join("contacts").join(target_group);
        create_dir_all(&dest_dir)?;
        let file_name = path_file_name(&src_path);
        let dest_path = unique_dest_path(&dest_dir, &file_name);
        fs::rename(&src_path, &dest_path).map_err(|e| AppError::io("reopen contact", &e))?;
        // Remove empty archive/contacts dir (ok to fail)
        let archive_dir = self.root.join("archive").join("contacts");
        let _ = fs::remove_dir(&archive_dir);
        self.append_audit_event(
            "contact_reopened",
            vec![audit_target("contact", &uid)],
            reason,
            json!({"contact_uid": uid, "group": target_group}),
        )?;
        Ok(json!({
            "code": "contact_reopened",
            "contact_uid": uid,
            "group": target_group,
            "contact_path": rel_path(&self.root, &dest_path),
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

        let mut created_uids: Vec<String> = Vec::new();
        let mut skipped_count = 0usize;

        for (display_name, email) in from_headers {
            let email_lc = email.to_ascii_lowercase();
            if self.contact_email_conflict(&email_lc)? {
                skipped_count += 1;
                continue;
            }
            let date = workspace_local_date(&offset);
            let uid = self.next_contact_uid_for_date(&date)?;
            let contact_dir = self.root.join("contacts").join(target_group);
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
            let default_notes = format!("## {actual_name}\n\n");
            write_contact_md(&path, &fm, &default_notes)?;
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
            "code": "contact_extract",
            "created_count": created_count,
            "skipped_count": skipped_count,
            "created_uids": created_uids,
        }))
    }

    /// Build an ephemeral email→contact map from contact files. Active contacts
    /// take priority; archived contacts fill in only addresses not already held
    /// by an active contact. Replaces the former persisted email index.
    pub(super) fn contact_email_map(&self) -> Result<BTreeMap<String, MessageContact>> {
        let mut map: BTreeMap<String, MessageContact> = BTreeMap::new();
        for (_, _, path) in self.all_active_contact_entries()? {
            let content = read_to_string(&path, "read contact for index")?;
            if let Ok((fm, _)) = read_contact_md_content(&content) {
                for email in &fm.emails {
                    let normalized = email.trim().to_ascii_lowercase();
                    if !normalized.is_empty() {
                        map.insert(normalized, message_contact_from(&fm));
                    }
                }
            }
        }
        for (_, path) in self.all_archived_contact_entries()? {
            let content = read_to_string(&path, "read archived contact for index")?;
            if let Ok((fm, _)) = read_contact_md_content(&content) {
                for email in &fm.emails {
                    let normalized = email.trim().to_ascii_lowercase();
                    if !normalized.is_empty() {
                        map.entry(normalized)
                            .or_insert_with(|| message_contact_from(&fm));
                    }
                }
            }
        }
        Ok(map)
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

    fn assert_email_unique(&self, email: &str, excluding_uid: Option<&str>) -> Result<()> {
        let normalized = email.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Ok(());
        }
        if let Some(entry) = self.contact_email_map()?.get(&normalized) {
            if excluding_uid != Some(entry.contact_uid.as_str()) {
                return Err(AppError::new(
                    "email_conflict",
                    format!(
                        "email {email} is already assigned to contact {}",
                        entry.contact_uid
                    ),
                )
                .with_details(json!({"email": email, "existing_contact_uid": entry.contact_uid})));
            }
        }
        Ok(())
    }

    /// True if any active or archived contact already holds this email. Unlike
    /// `assert_email_unique`, IO errors propagate instead of reading as "taken".
    fn contact_email_conflict(&self, email: &str) -> Result<bool> {
        let normalized = email.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Ok(false);
        }
        Ok(self.contact_email_map()?.contains_key(&normalized))
    }

    fn next_contact_uid_for_date(&self, date: &str) -> Result<String> {
        next_uid_for_date(
            'p',
            date,
            self.all_active_contact_entries()?
                .into_iter()
                .map(|(_, uid, _)| uid)
                .chain(
                    self.all_archived_contact_entries()?
                        .into_iter()
                        .map(|(uid, _)| uid),
                ),
        )
    }

    fn all_active_contact_entries(&self) -> Result<Vec<(String, String, PathBuf)>> {
        let contacts_dir = self.root.join("contacts");
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

    fn all_archived_contact_entries(&self) -> Result<Vec<(String, PathBuf)>> {
        let archive_dir = self.root.join("archive").join("contacts");
        if !archive_dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for entry in read_dir(&archive_dir, "list archived contacts")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if let Some(uid) = contact_uid_from_file_name(&stem) {
                entries.push((uid, path));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    fn find_active_contact_path(&self, uid: &str) -> Result<(String, PathBuf)> {
        for (group, entry_uid, path) in self.all_active_contact_entries()? {
            if entry_uid == uid {
                return Ok((group, path));
            }
        }
        Err(AppError::new(
            "not_found",
            format!("active contact {uid} not found"),
        )
        .with_hint(format!(
            "Use `afmail contact list` to see available contacts, or `afmail contact reopen {uid}` if it was archived."
        ))
        .with_details(json!({"contact_uid": uid})))
    }

    fn find_archived_contact_path(&self, uid: &str) -> Result<PathBuf> {
        for (entry_uid, path) in self.all_archived_contact_entries()? {
            if entry_uid == uid {
                return Ok(path);
            }
        }
        Err(
            AppError::new("not_found", format!("archived contact {uid} not found"))
                .with_details(json!({"contact_uid": uid})),
        )
    }

    fn find_contact_path(&self, uid: &str) -> Result<(bool, PathBuf)> {
        for (_, entry_uid, path) in self.all_active_contact_entries()? {
            if entry_uid == uid {
                return Ok((false, path));
            }
        }
        for (entry_uid, path) in self.all_archived_contact_entries()? {
            if entry_uid == uid {
                return Ok((true, path));
            }
        }
        Err(AppError::new(
            "not_found",
            format!("contact {uid} not found (active or archived)"),
        )
        .with_details(json!({"contact_uid": uid})))
    }

    fn collect_from_headers_triage(&self) -> Result<Vec<(String, String)>> {
        let triage_dir = self.root.join("triage");
        if !triage_dir.exists() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for entry in read_dir(&triage_dir, "list triage")? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let Ok((fm, _)) =
                crate::markdown::read_doc::<crate::frontmatter::TriageFrontmatter>(&content)
            else {
                continue;
            };
            let primary_id = if fm.message_id.is_empty() {
                fm.message_ids.first().cloned().unwrap_or_default()
            } else {
                fm.message_id.clone()
            };
            if primary_id.is_empty() {
                continue;
            }
            if let Some(pair) = self.message_sender_pair(&primary_id) {
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
            .join("messages")
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

fn message_contact_from(fm: &ContactFrontmatter) -> MessageContact {
    MessageContact {
        contact_uid: fm.contact_uid.clone(),
        display_name: fm.display_name.clone(),
    }
}

/// Resolve the contact whose email set includes a message's `From` header,
/// using a prebuilt email→contact map (see `contact_email_map`).
pub(super) fn contact_for_from(
    map: &BTreeMap<String, MessageContact>,
    from_header: &str,
) -> Option<MessageContact> {
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

fn contact_group_from_path(_root: &Path, path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| {
            // Skip if parent is "contacts" (shouldn't happen but guard)
            if s == "contacts" {
                "people"
            } else {
                s
            }
        })
        .unwrap_or("people")
        .to_string()
}
