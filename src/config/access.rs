use super::*;
use agent_first_data::document::{
    Addressing, Document, DocumentError, KeyedList, Value as AfValue, ValueType,
    coerce_values_toward, get_path, value_from_type,
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
        let value = get_path(&root, key, Addressing::keyed(KEYED_LISTS))
            .map_err(|e| config_key_error(key, e))?;
        Value::try_from(value).map_err(|e| config_key_error(key, e))
    }

    /// Set one config key over an in-memory config — test-only, see
    /// [`Self::mutate_via_document`].
    #[cfg(test)]
    pub(super) fn set_key(&mut self, key: &str, values: &[String]) -> Result<()> {
        self.mutate_via_document(|doc| Self::apply_set(doc, key, values))
    }

    /// Append a keyed-list element — test-only, see [`Self::mutate_via_document`].
    #[cfg(test)]
    pub(super) fn add_key(
        &mut self,
        key: &str,
        slug: &str,
        fields: &[(String, String)],
    ) -> Result<()> {
        self.mutate_via_document(|doc| Self::apply_add(doc, key, slug, fields))
    }

    /// Remove a keyed-list element — test-only, see [`Self::mutate_via_document`].
    #[cfg(test)]
    pub(super) fn remove_key(&mut self, key: &str, slug: &str) -> Result<()> {
        self.mutate_via_document(|doc| Self::apply_remove(doc, key, slug))
    }

    /// Serialize the config to its sparse document form, run `edit` through the
    /// shared `apply_*` rules, re-hydrate `self` from the result, and revalidate.
    ///
    /// Test-only. `config set`/`add`/`remove` do not come through here: they edit
    /// `.afmail/config.json` as an afdata `DocumentFile` (open, stage, validate,
    /// atomic save — see the `config_set`/`config_add`/`config_remove` store
    /// handlers), which preserves the file's own source and never leaves a
    /// rejected edit on disk. This is the file-less mirror of that sequence, for
    /// the unit tests that assert the `apply_*` domain rules directly. Keeping it
    /// out of the published API leaves exactly one way to edit a config: the one
    /// that writes atomically.
    #[cfg(test)]
    fn mutate_via_document<F>(&mut self, edit: F) -> Result<()>
    where
        F: FnOnce(&mut Document) -> Result<()>,
    {
        let sparse = serde_json::to_string(&self.sparse_value()?)
            .map_err(|e| AppError::json("serialize config", &e))?;
        let mut doc = Document::parse(&sparse, agent_first_data::document::Format::Json)
            .map_err(|e| config_key_error("config", e))?;
        edit(&mut doc)?;
        let edited =
            Value::try_from(doc.value().clone()).map_err(|e| config_key_error("config", e))?;
        let merged = merge_with_defaults(edited)?;
        // Build and validate a candidate before it becomes `self`, matching the
        // store handlers' validate-before-write. Assigning first left a rejected
        // edit in place: the caller saw the Err, and every later mutation then
        // failed on the invalid entry that the rejection had supposedly prevented.
        let candidate: Self = serde_json::from_value(merged)
            .map_err(|e| AppError::new("config_invalid", format!("invalid config schema: {e}")))?;
        candidate.validate()?;
        *self = candidate;
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
        materialize_keyed_list(doc, key)?;
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
        materialize_keyed_list(doc, key)?;
        match doc.remove(key, slug, slug_field) {
            Ok(()) | Err(DocumentError::SlugNotFound { .. }) => Ok(()),
            Err(err) => Err(config_key_error(key, err)),
        }
    }

    /// Validate a candidate config value edited in `doc` exactly as [`load`]
    /// validates a freshly-read one — schema, invariants, identity files —
    /// without persisting anything.
    pub fn validate_document(doc: &Document, workspace_root: &Path) -> Result<()> {
        let raw =
            Value::try_from(doc.value().clone()).map_err(|e| config_key_error("config", e))?;
        reject_legacy_config(&raw)?;
        let config: Self = serde_json::from_value(merge_with_defaults(raw)?)
            .map_err(|e| AppError::new("config_invalid", format!("invalid config schema: {e}")))?;
        config.validate()?;
        config.validate_identity_files(workspace_root)?;
        Ok(())
    }

    /// Serialize the live config to the document layer's value IR. Optional fields carry no
    /// `skip_serializing_if`, so the value is dense (absent → `null`) and every
    /// key is traversable.
    fn as_config_value(&self) -> Result<AfValue> {
        let json = serde_json::to_value(self)
            .map_err(|e| AppError::new("internal", format!("serialize config: {e}")))?;
        Ok(AfValue::from(json))
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
        doc_unset(doc, key)
    } else {
        doc.set(key, value).map_err(|e| config_key_error(key, e))
    }
}

/// Clear `key` if the sparse config carries it.
///
/// `Document::unset` is idempotent at any depth, so a key this sparse file
/// never wrote — at any level — is simply `Ok(false)`, not an error to absorb.
fn doc_unset(doc: &mut Document, key: &str) -> Result<()> {
    doc.unset(key)
        .map(|_removed| ())
        .map_err(|err| config_key_error(key, err))
}

/// The schema default value at `key` (dense template), if any.
fn schema_default_at(key: &str) -> Result<Option<AfValue>> {
    let defaults = AfValue::from(MailConfig::default_template_value()?);
    Ok(get_path(&defaults, key, Addressing::keyed(KEYED_LISTS)).ok())
}

/// Whether `value` equals the schema default at `key`, so it need not be
/// persisted in the sparse config.
fn value_is_schema_default(key: &str, value: &AfValue) -> Result<bool> {
    Ok(schema_default_at(key)?.as_ref() == Some(value))
}

/// Ensure the keyed list at `key` is present before an add/remove. A list equal
/// to its schema default is stripped from the sparse config, but these verbs
/// need the full current contents — so seed it from the default with a raw
/// `set` (bypassing `doc_set`'s unset-if-default collapse).
fn materialize_keyed_list(doc: &mut Document, key: &str) -> Result<()> {
    if doc.value_at(key).is_err()
        && let Some(default) = schema_default_at(key)?
    {
        doc.set(key, default)
            .map_err(|e| config_key_error(key, e))?;
    }
    Ok(())
}

/// Set `key` and clear its mutually-exclusive `sibling` (an idempotent unset),
/// so validation never sees both populated.
fn set_exclusive(doc: &mut Document, key: &str, value: String, sibling: &str) -> Result<()> {
    doc_set(doc, key, AfValue::String(value))?;
    doc_unset(doc, sibling)?;
    Ok(())
}

/// Set `key`, or clear it to `null` when the normalized `value` is `None`. Uses
/// `null` rather than unset so it overrides a non-`null` schema default (e.g.
/// the pinned `workspace.timezone_utc_offset`); `doc_set` still collapses it to
/// an unset when the default itself is `null`.
fn set_optional(doc: &mut Document, key: &str, value: Option<String>) -> Result<()> {
    match value {
        Some(value) => doc_set(doc, key, AfValue::String(value)),
        None => doc_set(doc, key, AfValue::Null),
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
