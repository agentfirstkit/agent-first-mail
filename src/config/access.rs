use super::*;
use agent_first_data::normalize_utc_offset;

impl MailConfig {
    pub fn get_key(&self, key: &str) -> Result<Value> {
        match key {
            "schema_name" => Ok(json!(self.schema_name)),
            "schema_version" => Ok(json!(self.schema_version)),
            "imap.host" => Ok(json!(self.imap.host)),
            "imap.port" => Ok(json!(self.imap.port)),
            "imap.tls" => Ok(json!(self.imap.tls)),
            "imap.username" => Ok(json!(self.imap.username)),
            "imap.password_secret" => Ok(json!(self.imap.password_secret)),
            "imap.password_secret_env" => Ok(json!(self.imap.password_secret_env)),
            "mailboxes" => Ok(json!(self.mailboxes)),
            key if key.starts_with("mailboxes.") => self.get_mailbox_key(key),
            "actions" => Ok(json!(self.actions)),
            "actions.pull.default_mailbox_ids" => Ok(json!(self.actions.pull.default_mailbox_ids)),
            key if key.starts_with("actions.pull.by_mailbox_id.") => {
                self.get_pull_mailbox_action_key(key)
            }
            key if key.starts_with("actions.message.archive.by_source_mailbox_id.") => {
                self.get_archive_action_key(key)
            }
            "actions.case.add.steps" => Ok(json!(self.actions.case_add.steps)),
            "actions.draft.save.steps" => Ok(json!(self.actions.draft_save.steps)),
            "actions.draft.send.steps" => Ok(json!(self.actions.draft_send.steps)),
            "actions.message.spam.steps" => Ok(json!(self.actions.message_spam.steps)),
            "actions.message.trash.steps" => Ok(json!(self.actions.message_trash.steps)),
            "case.default_group" => Ok(json!(self.case.default_group)),
            "archive" => Ok(json!(self.archive)),
            "archive.message_index.item_fields" => {
                Ok(json!(self.archive.message_index.item_fields))
            }
            "archive.message_index.sort" => Ok(json!(self.archive.message_index.sort.as_str())),
            "audit.reason_mode" => Ok(json!(self.audit.reason_mode.as_str())),
            "smtp.host" => Ok(json!(self.smtp.host)),
            "smtp.port" => Ok(json!(self.smtp.port)),
            "smtp.starttls" => Ok(json!(self.smtp.starttls)),
            "smtp.tls_wrapper" => Ok(json!(self.smtp.tls_wrapper)),
            "smtp.username" => Ok(json!(self.smtp.username)),
            "smtp.password_secret" => Ok(json!(self.smtp.password_secret)),
            "smtp.password_secret_env" => Ok(json!(self.smtp.password_secret_env)),
            "smtp.from" => Ok(json!(self.smtp.from)),
            "workspace.language_bcp47" => Ok(json!(self.workspace.language_bcp47)),
            "workspace.timezone_utc_offset" => Ok(json!(self.workspace.timezone_utc_offset)),
            _ => Err(AppError::new(
                "invalid_request",
                format!("unknown config key: {key}"),
            )),
        }
    }

    fn get_mailbox_key(&self, key: &str) -> Result<Value> {
        let rest = &key["mailboxes.".len()..];
        let (id, field) = rest.split_once('.').ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "mailboxes key expects mailboxes.<id>.<field>",
            )
        })?;
        let mailbox = self.mailbox(id)?;
        match field {
            "mailbox_name" => Ok(json!(mailbox.mailbox_name)),
            "special_use" => Ok(json!(mailbox.special_use)),
            _ => Err(AppError::new(
                "invalid_request",
                format!("unknown config key: {key}"),
            )),
        }
    }

    pub fn set_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        if values.is_empty() {
            return Err(AppError::new(
                "invalid_request",
                "config set requires at least one value",
            ));
        }
        match key {
            "imap.host" => self.imap.host = Some(single(values, key)?),
            "imap.port" => self.imap.port = parse_u16(&single(values, key)?, key)?,
            "imap.tls" => self.imap.tls = parse_bool(&single(values, key)?, key)?,
            "imap.username" => self.imap.username = Some(single(values, key)?),
            "imap.password_secret" => {
                self.imap.password_secret = Some(single(values, key)?);
                self.imap.password_secret_env = None;
            }
            "imap.password_secret_env" => {
                self.imap.password_secret_env = Some(single(values, key)?);
                self.imap.password_secret = None;
            }
            "actions.pull.default_mailbox_ids" => {
                let mut ids = Vec::new();
                for value in values {
                    validate_config_id("actions.pull.default_mailbox_ids id", value)?;
                    if !ids.iter().any(|existing| existing == value) {
                        ids.push(value.clone());
                    }
                }
                self.actions.pull.default_mailbox_ids = ids;
            }
            "case.default_group" => self.case.default_group = single(values, key)?,
            "audit.reason_mode" => {
                self.audit.reason_mode = ReasonMode::parse(&single(values, key)?)?;
            }
            "smtp.host" => self.smtp.host = Some(single(values, key)?),
            "smtp.port" => self.smtp.port = parse_u16(&single(values, key)?, key)?,
            "smtp.starttls" => self.smtp.starttls = parse_bool(&single(values, key)?, key)?,
            "smtp.tls_wrapper" => self.smtp.tls_wrapper = parse_bool(&single(values, key)?, key)?,
            "smtp.username" => self.smtp.username = Some(single(values, key)?),
            "smtp.password_secret" => {
                self.smtp.password_secret = Some(single(values, key)?);
                self.smtp.password_secret_env = None;
            }
            "smtp.password_secret_env" => {
                self.smtp.password_secret_env = Some(single(values, key)?);
                self.smtp.password_secret = None;
            }
            "smtp.from" => self.smtp.from = Some(single(values, key)?),
            "workspace.language_bcp47" => {
                let value = single(values, key)?;
                self.workspace.language_bcp47 = parse_optional_language_bcp47(&value)?;
            }
            "workspace.timezone_utc_offset" => {
                let value = single(values, key)?;
                self.workspace.timezone_utc_offset = parse_optional_timezone_utc_offset(&value)?;
            }
            key if key.starts_with("mailboxes.") => self.set_mailbox_key(key, values)?,
            key if key.starts_with("actions.pull.by_mailbox_id.") => {
                self.set_pull_mailbox_action_key(key, values)?
            }
            key if key.starts_with("actions.message.archive.by_source_mailbox_id.") => {
                self.set_archive_action_key(key, values)?
            }
            "archive.message_index.item_fields" => {
                self.archive.message_index.item_fields = values
                    .iter()
                    .map(|value| ArchiveMessageIndexField::parse(value))
                    .collect::<Result<Vec<_>>>()?;
            }
            "archive.message_index.sort" => {
                self.archive.message_index.sort =
                    ArchiveMessageIndexSort::parse(&single(values, key)?)?;
            }
            _ => {
                return Err(AppError::new(
                    "invalid_request",
                    format!("unknown config key: {key}"),
                ))
            }
        }
        self.validate()?;
        Ok(())
    }

    fn set_mailbox_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        let rest = &key["mailboxes.".len()..];
        let (id, field) = rest.split_once('.').ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "mailboxes key expects mailboxes.<id>.<field>",
            )
        })?;
        validate_config_id("mailboxes id", id)?;
        let mailbox = self
            .mailboxes
            .entry(id.to_string())
            .or_insert_with(ImapMailboxConfig::empty);
        match field {
            "mailbox_name" => {
                mailbox.mailbox_name = Some(single(values, key)?);
                mailbox.special_use = None;
            }
            "special_use" => {
                mailbox.special_use = Some(single(values, key)?);
                mailbox.mailbox_name = None;
            }
            _ => {
                return Err(AppError::new(
                    "invalid_request",
                    format!("unknown config key: {key}"),
                ));
            }
        }
        Ok(())
    }

    fn get_pull_mailbox_action_key(&self, key: &str) -> Result<Value> {
        let rest = &key["actions.pull.by_mailbox_id.".len()..];
        let (id, field) = rest.split_once('.').ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "actions pull key expects actions.pull.by_mailbox_id.<id>.<field>",
            )
        })?;
        let action = self.pull_action(id)?;
        match field {
            "import_as" => Ok(json!(action.import_as.as_str())),
            "direction" => Ok(json!(action.direction.as_str())),
            _ => Err(AppError::new(
                "invalid_request",
                format!("unknown config key: {key}"),
            )),
        }
    }

    fn set_pull_mailbox_action_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        let rest = &key["actions.pull.by_mailbox_id.".len()..];
        let (id, field) = rest.split_once('.').ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "actions pull key expects actions.pull.by_mailbox_id.<id>.<field>",
            )
        })?;
        validate_config_id("actions.pull.by_mailbox_id id", id)?;
        let action = self
            .actions
            .pull
            .by_mailbox_id
            .entry(id.to_string())
            .or_default();
        match field {
            "import_as" => action.import_as = PullImportAs::parse(&single(values, key)?)?,
            "direction" => action.direction = MailDirection::parse(&single(values, key)?)?,
            _ => {
                return Err(AppError::new(
                    "invalid_request",
                    format!("unknown config key: {key}"),
                ));
            }
        }
        Ok(())
    }

    fn get_archive_action_key(&self, key: &str) -> Result<Value> {
        let rest = &key["actions.message.archive.by_source_mailbox_id.".len()..];
        let (id, field) = rest.split_once('.').ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "archive action key expects actions.message.archive.by_source_mailbox_id.<id>.<field>",
            )
        })?;
        let rule = self
            .actions
            .message_archive
            .by_source_mailbox_id
            .get(id)
            .ok_or_else(|| {
                AppError::new(
                    "unknown_mailbox_id",
                    format!("unknown archive source mailbox id: {id}"),
                )
            })?;
        match field {
            "steps" => Ok(json!(rule.steps)),
            "move_to_mailbox_id" => Ok(json!(first_move_to_mailbox_id(&rule.steps))),
            _ => Err(AppError::new(
                "invalid_request",
                format!("unknown config key: {key}"),
            )),
        }
    }

    fn set_archive_action_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        let rest = &key["actions.message.archive.by_source_mailbox_id.".len()..];
        let (id, field) = rest.split_once('.').ok_or_else(|| {
            AppError::new(
                "invalid_request",
                "archive action key expects actions.message.archive.by_source_mailbox_id.<id>.<field>",
            )
        })?;
        validate_config_id("actions.message.archive source id", id)?;
        let rule = self
            .actions
            .message_archive
            .by_source_mailbox_id
            .entry(id.to_string())
            .or_default();
        match field {
            "move_to_mailbox_id" | "steps.move_to_mailbox_id" => {
                rule.steps = vec![ActionStep::move_to_mailbox_id(single(values, key)?)];
            }
            "steps" if values == ["none"] || values == ["[]"] => {
                rule.steps.clear();
            }
            _ => {
                return Err(AppError::new(
                    "invalid_request",
                    format!("unknown config key: {key}"),
                ));
            }
        }
        Ok(())
    }
}

fn parse_optional_language_bcp47(value: &str) -> Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("null") {
        return Ok(None);
    }
    validate_language_bcp47("workspace.language_bcp47", trimmed, "invalid_request")?;
    Ok(Some(trimmed.to_string()))
}

fn parse_optional_timezone_utc_offset(value: &str) -> Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("null") {
        return Ok(None);
    }
    normalize_utc_offset(trimmed).map(Some).ok_or_else(|| {
        AppError::new(
            "invalid_request",
            "workspace.timezone_utc_offset expects UTC or a fixed offset like +08:00",
        )
    })
}
