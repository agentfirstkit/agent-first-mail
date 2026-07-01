use super::*;
use agent_first_config::{
    add_keyed, get_path, remove_keyed, set_path, ConfigError, KeyedList, Value as AfValue,
};
use agent_first_data::normalize_utc_offset;

/// Arrays addressed by a slug segment rather than a numeric index. Everything
/// else in the config is plain object/scalar traversal that afconfig handles
/// generically — only these need a registration so `identities.<slug>` routes to
/// the element whose `identity` field matches.
const KEYED_LISTS: &[KeyedList] = &[KeyedList {
    prefix: "identities",
    slug_field: "identity",
}];

impl MailConfig {
    /// Read one config key via afconfig dot-path traversal over the serialized config.
    pub fn get_key(&self, key: &str) -> Result<Value> {
        let root = self.as_config_value()?;
        let value = get_path(&root, key, KEYED_LISTS).map_err(|e| config_key_error(key, e))?;
        Ok(Value::from(value))
    }

    /// Set one config key. Keys whose semantics afconfig cannot express — typed
    /// normalization, mutually-exclusive fields, the `move_to_mailbox_id` alias —
    /// are handled by `apply_domain_override`; the rest go through afconfig's
    /// generic coercing setter. Either way the result is re-validated.
    pub fn set_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        if values.is_empty() {
            return Err(AppError::new(
                "invalid_request",
                "config set requires at least one value",
            ));
        }

        // `none` is a CLI sentinel for "clear this step list"; afconfig has no
        // such notion, so rewrite it to an explicit empty array before coercion.
        let cleared;
        let values = if key.ends_with(".steps") && values == ["none"] {
            cleared = [String::from("[]")];
            &cleared[..]
        } else {
            values
        };

        if !self.apply_domain_override(key, values)? {
            let mut root = self.as_config_value()?;
            set_path(&mut root, key, values, KEYED_LISTS).map_err(|e| config_key_error(key, e))?;
            self.replace_from_value(Value::from(root), key)?;
        }

        self.validate()?;
        Ok(())
    }

    /// Append an element to a keyed list (only `identities` today). The slug
    /// becomes the element's `identity`; `fields` are `name=…`, `email=…`,
    /// `default=…` pairs coerced by afconfig.
    pub fn add_key(&mut self, key: &str, slug: &str, fields: &[(String, String)]) -> Result<()> {
        let mut config = self.clone();
        if key == "identities" && identity_fields_set_default(fields) {
            for identity in &mut config.identities {
                identity.default = false;
            }
        }
        let mut root = config.as_config_value()?;
        add_keyed(&mut root, key, slug, KEYED_LISTS, None, fields)
            .map_err(|e| config_key_error(key, e))?;
        self.replace_from_value(Value::from(root), key)?;
        self.validate()?;
        Ok(())
    }

    /// Remove the keyed-list element whose slug matches.
    pub fn remove_key(&mut self, key: &str, slug: &str) -> Result<()> {
        let mut root = self.as_config_value()?;
        match remove_keyed(&mut root, key, slug, KEYED_LISTS) {
            Ok(()) | Err(ConfigError::SlugNotFound { .. }) => {}
            Err(err) => return Err(config_key_error(key, err)),
        }
        self.replace_from_value(Value::from(root), key)?;
        self.validate()?;
        Ok(())
    }

    /// Handle keys whose behavior afconfig's value-shaped coercion can't capture.
    /// Returns `true` when the key was consumed here, `false` to fall through to
    /// the generic setter.
    fn apply_domain_override(&mut self, key: &str, values: &[String]) -> Result<bool> {
        match key {
            // Password source is one-of: setting either side clears the other so
            // validation can never see both populated.
            "imap.password_secret" => {
                self.imap.password_secret = Some(single(values, key)?);
                self.imap.password_secret_env = None;
            }
            "imap.password_secret_env" => {
                self.imap.password_secret_env = Some(single(values, key)?);
                self.imap.password_secret = None;
            }
            "smtp.password_secret" => {
                self.smtp.password_secret = Some(single(values, key)?);
                self.smtp.password_secret_env = None;
            }
            "smtp.password_secret_env" => {
                self.smtp.password_secret_env = Some(single(values, key)?);
                self.smtp.password_secret = None;
            }
            // These accept human inputs (`+0530`, `UTC`, `null`) that have to be
            // normalized/validated into canonical stored form.
            "workspace.language_bcp47" => {
                self.workspace.language_bcp47 =
                    parse_optional_language_bcp47(&single(values, key)?)?;
            }
            "workspace.timezone_utc_offset" => {
                self.workspace.timezone_utc_offset =
                    parse_optional_timezone_utc_offset(&single(values, key)?)?;
            }
            // Each id is validated and duplicates collapsed — afconfig would store
            // the raw multi-value array verbatim.
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
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Serialize the live config to afconfig's value IR. Optional fields carry no
    /// `skip_serializing_if`, so the value is dense (absent → `null`) and every
    /// key is traversable.
    fn as_config_value(&self) -> Result<AfValue> {
        let json = serde_json::to_value(self)
            .map_err(|e| AppError::new("internal", format!("serialize config: {e}")))?;
        Ok(AfValue::from(json))
    }

    /// Re-hydrate `self` from a mutated value, surfacing serde type/shape errors
    /// (e.g. a string coerced where a `u16` was expected) against the key.
    fn replace_from_value(&mut self, value: Value, key: &str) -> Result<()> {
        *self = serde_json::from_value(value).map_err(|e| {
            config_key_error(key, agent_first_config::ConfigError::from_serde(key, e))
        })?;
        Ok(())
    }
}

fn identity_fields_set_default(fields: &[(String, String)]) -> bool {
    fields.iter().any(|(field, value)| {
        field == "default" && matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes")
    })
}

fn config_key_error(key: &str, err: agent_first_config::ConfigError) -> AppError {
    AppError::new("invalid_request", format!("config key {key}: {err}"))
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
