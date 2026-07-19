use super::*;
use agent_first_data::document::{
    Document, DocumentError, KeyedList, Value as AfValue, ValueType, add_keyed,
    coerce_values_toward, get_path, remove_keyed, set_path, value_from_type,
};
use agent_first_data::normalize_utc_offset;
use std::path::Path;

/// Arrays addressed by a slug segment rather than a numeric index. Everything
/// else in the config is plain object/scalar traversal that afdata's document
/// layer handles generically — only these need a registration so
/// `identities.<slug>` routes to the element whose `identity` field matches.
const KEYED_LISTS: &[KeyedList] = &[KeyedList {
    prefix: "identities",
    slug_field: "identity",
}];

impl MailConfig {
    /// Read one config key via the document layer dot-path traversal over the serialized config.
    pub fn get_key(&self, key: &str) -> Result<Value> {
        let root = self.as_config_value()?;
        let value = get_path(&root, key, KEYED_LISTS).map_err(|e| config_key_error(key, e))?;
        Ok(Value::from(value))
    }

    /// Set one config key. Keys whose semantics the document layer cannot express — typed
    /// normalization, mutually-exclusive fields, the `move_to_mailbox_id` alias —
    /// are handled by `apply_domain_override`; the rest go through the document layer's
    /// generic coercing setter. Either way the result is re-validated.
    pub fn set_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        if values.is_empty() {
            return Err(AppError::new(
                "invalid_request",
                "config set requires at least one value",
            ));
        }

        // `none` is a CLI sentinel for "clear this step list"; the document layer has no
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
            // The document setter no longer coerces CLI strings; coerce them toward the
            // type already present at the leaf (falling back to String) before setting.
            let existing = get_path(&root, key, KEYED_LISTS).ok();
            let new_value = coerce_values_toward(values, existing.as_ref())
                .map_err(|e| config_key_error(key, e))?;
            set_path(&mut root, key, &new_value, KEYED_LISTS)
                .map_err(|e| config_key_error(key, e))?;
            self.replace_from_value(Value::from(root), key)?;
        }

        self.validate()?;
        Ok(())
    }

    /// Append an element to a keyed list (only `identities` today). The slug
    /// becomes the element's `identity`; `fields` are `name=…`, `email=…`,
    /// `default=…` pairs coerced by the document layer.
    pub fn add_key(&mut self, key: &str, slug: &str, fields: &[(String, String)]) -> Result<()> {
        let mut config = self.clone();
        if key == "identities" && identity_fields_set_default(fields) {
            for identity in &mut config.identities {
                identity.default = false;
            }
        }
        let mut root = config.as_config_value()?;
        // `add_keyed` takes typed field values. A freshly-appended element has no
        // existing leaf to type against, so the caller supplies the type it knows
        // from the identity schema: `default` is a bool, the rest are strings.
        // (afdata no longer shape-guesses — a bare value is a string.)
        let coerced_fields: Vec<(String, AfValue)> = fields
            .iter()
            .map(|(name, value)| {
                let coerced = match name.as_str() {
                    "default" => value_from_type(ValueType::Bool, Some(value))
                        .unwrap_or_else(|_| AfValue::String(value.clone())),
                    _ => AfValue::String(value.clone()),
                };
                (name.clone(), coerced)
            })
            .collect();
        add_keyed(&mut root, key, slug, KEYED_LISTS, None, &coerced_fields)
            .map_err(|e| config_key_error(key, e))?;
        self.replace_from_value(Value::from(root), key)?;
        self.validate()?;
        Ok(())
    }

    /// Remove the keyed-list element whose slug matches.
    pub fn remove_key(&mut self, key: &str, slug: &str) -> Result<()> {
        let mut root = self.as_config_value()?;
        match remove_keyed(&mut root, key, slug, KEYED_LISTS) {
            Ok(()) | Err(DocumentError::SlugNotFound { .. }) => {}
            Err(err) => return Err(config_key_error(key, err)),
        }
        self.replace_from_value(Value::from(root), key)?;
        self.validate()?;
        Ok(())
    }

    /// Stage a `config set <key> <values>` edit onto the sparse config document,
    /// re-expressing the domain rules — mutually-exclusive secrets, human-input
    /// normalization, id dedup, the `.steps` clear sentinel — as document edits.
    /// The caller validates via [`Self::validate_document`] and commits with
    /// [`agent_first_data::document::DocumentFile::save`].
    pub fn apply_set(doc: &mut Document, key: &str, values: &[String]) -> Result<()> {
        if values.is_empty() {
            return Err(AppError::new(
                "invalid_request",
                "config set requires at least one value",
            ));
        }
        // `none` is the CLI sentinel for "clear this step list".
        if key.ends_with(".steps") && values == ["none"] {
            return doc_set(doc, key, AfValue::Array(Vec::new()));
        }
        match key {
            "imap.password_secret" => {
                set_exclusive(doc, key, single(values, key)?, "imap.password_secret_env")
            }
            "imap.password_secret_env" => {
                set_exclusive(doc, key, single(values, key)?, "imap.password_secret")
            }
            "smtp.password_secret" => {
                set_exclusive(doc, key, single(values, key)?, "smtp.password_secret_env")
            }
            "smtp.password_secret_env" => {
                set_exclusive(doc, key, single(values, key)?, "smtp.password_secret")
            }
            "workspace.language_bcp47" => set_optional(
                doc,
                key,
                parse_optional_language_bcp47(&single(values, key)?)?,
            ),
            "workspace.timezone_utc_offset" => set_optional(
                doc,
                key,
                parse_optional_timezone_utc_offset(&single(values, key)?)?,
            ),
            "actions.pull.default_mailbox_ids" => {
                let mut ids: Vec<AfValue> = Vec::new();
                for value in values {
                    validate_config_id("actions.pull.default_mailbox_ids id", value)?;
                    if !ids
                        .iter()
                        .any(|existing| existing.as_str() == Some(value.as_str()))
                    {
                        ids.push(AfValue::String(value.clone()));
                    }
                }
                doc_set(doc, key, AfValue::Array(ids))
            }
            _ => {
                // Coerce toward the value already at the leaf, or the schema
                // default's type when the sparse file has none — so e.g.
                // `imap.port 993` stores a number even before `imap` exists.
                let existing = doc.value_at(key).ok().or(schema_default_at(key)?);
                let coerced = coerce_values_toward(values, existing.as_ref())
                    .map_err(|e| config_key_error(key, e))?;
                doc_set(doc, key, coerced)
            }
        }
    }

    /// Stage a `config add <key> <slug> <fields>` edit. Only the `identities`
    /// keyed list exists today; adding one that sets `default` first clears
    /// `default` on the existing elements.
    pub fn apply_add(
        doc: &mut Document,
        key: &str,
        slug: &str,
        fields: &[(String, String)],
    ) -> Result<()> {
        let slug_field = keyed_list_slug_field(key)?;
        if key == "identities"
            && identity_fields_set_default(fields)
            && let Ok(AfValue::Array(items)) = doc.value_at(key)
        {
            for index in 0..items.len() {
                doc_set(doc, &format!("{key}.{index}.default"), AfValue::Bool(false))?;
            }
        }
        let coerced_fields: Vec<(String, AfValue)> = fields
            .iter()
            .map(|(name, value)| {
                let coerced = match name.as_str() {
                    "default" => value_from_type(ValueType::Bool, Some(value))
                        .unwrap_or_else(|_| AfValue::String(value.clone())),
                    _ => AfValue::String(value.clone()),
                };
                (name.clone(), coerced)
            })
            .collect();
        doc.add(key, slug, slug_field, &coerced_fields)
            .map_err(|e| config_key_error(key, e))
    }

    /// Stage a `config remove <key> <slug>` edit. Idempotent: a missing slug is
    /// not an error.
    pub fn apply_remove(doc: &mut Document, key: &str, slug: &str) -> Result<()> {
        let slug_field = keyed_list_slug_field(key)?;
        match doc.remove(key, slug, slug_field) {
            Ok(()) | Err(DocumentError::SlugNotFound { .. }) => Ok(()),
            Err(err) => Err(config_key_error(key, err)),
        }
    }

    /// Validate a candidate config value edited in `doc` exactly as [`load`]
    /// validates a freshly-read one — schema, invariants, identity files —
    /// without persisting anything.
    pub fn validate_document(doc: &Document, workspace_root: &Path) -> Result<()> {
        let raw = Value::from(doc.value().clone());
        reject_legacy_config(&raw)?;
        let config: Self = serde_json::from_value(merge_with_defaults(raw)?)
            .map_err(|e| AppError::new("config_invalid", format!("invalid config schema: {e}")))?;
        config.validate()?;
        config.validate_identity_files(workspace_root)?;
        Ok(())
    }

    /// Handle keys whose behavior the document layer's value-shaped coercion can't capture.
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
            // Each id is validated and duplicates collapsed — the document layer would store
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

    /// Serialize the live config to the document layer's value IR. Optional fields carry no
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
            config_key_error(
                key,
                agent_first_data::document::DocumentError::from_serde(key, e),
            )
        })?;
        Ok(())
    }
}

fn identity_fields_set_default(fields: &[(String, String)]) -> bool {
    fields.iter().any(|(field, value)| {
        field == "default" && matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes")
    })
}

/// Stage a document set, keeping afmail's config sparse: a value equal to the
/// key's schema default is cleared instead of written, so defaults never
/// accumulate in the file (what `sparse_diff` did whole-config, done per key).
/// Errors map against `key`.
fn doc_set(doc: &mut Document, key: &str, value: AfValue) -> Result<()> {
    if value_is_schema_default(key, &value)? {
        doc.unset(key)
            .map(|_| ())
            .map_err(|e| config_key_error(key, e))
    } else {
        doc.set(key, value).map_err(|e| config_key_error(key, e))
    }
}

/// The schema default value at `key` (dense template), if any.
fn schema_default_at(key: &str) -> Result<Option<AfValue>> {
    let defaults = AfValue::from(MailConfig::default_template_value()?);
    Ok(get_path(&defaults, key, KEYED_LISTS).ok())
}

/// Whether `value` equals the schema default at `key`, so it need not be
/// persisted in the sparse config.
fn value_is_schema_default(key: &str, value: &AfValue) -> Result<bool> {
    Ok(schema_default_at(key)?.as_ref() == Some(value))
}

/// Set `key` and clear its mutually-exclusive `sibling` (an idempotent unset),
/// so validation never sees both populated.
fn set_exclusive(doc: &mut Document, key: &str, value: String, sibling: &str) -> Result<()> {
    doc_set(doc, key, AfValue::String(value))?;
    doc.unset(sibling)
        .map_err(|e| config_key_error(sibling, e))?;
    Ok(())
}

/// Set `key`, or clear it when the normalized `value` is `None`.
fn set_optional(doc: &mut Document, key: &str, value: Option<String>) -> Result<()> {
    match value {
        Some(value) => doc_set(doc, key, AfValue::String(value)),
        None => doc
            .unset(key)
            .map(|_| ())
            .map_err(|e| config_key_error(key, e)),
    }
}

/// The slug field of a registered keyed list, or an error for a non-keyed key.
fn keyed_list_slug_field(key: &str) -> Result<&'static str> {
    KEYED_LISTS
        .iter()
        .find(|list| list.prefix == key)
        .map(|list| list.slug_field)
        .ok_or_else(|| {
            AppError::new(
                "invalid_request",
                format!("config key {key} is not a keyed list"),
            )
        })
}

fn config_key_error(key: &str, err: agent_first_data::document::DocumentError) -> AppError {
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
