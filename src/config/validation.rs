use super::*;
use agent_first_data::normalize_utc_offset;

pub(super) fn reject_legacy_config(raw: &Value) -> Result<()> {
    let Some(obj) = raw.as_object() else {
        return Err(AppError::new(
            "config_invalid",
            "config must be a JSON object",
        ));
    };
    for legacy_key in [
        "imap_host",
        "imap_port",
        "imap_tls",
        "imap_username",
        "imap_password_secret",
        "smtp_host",
        "smtp_port",
        "smtp_starttls",
        "smtp_tls_wrapper",
        "smtp_username",
        "smtp_password_secret",
        "from",
        "send",
        "folders",
        "special_use",
        "notes",
        "imap_mailboxes",
        "pull",
        "push",
        "ui",
        "timezone",
    ] {
        if obj.contains_key(legacy_key) {
            return Err(AppError::new(
                "config_invalid",
                format!("unsupported legacy config key: {legacy_key}; use mailboxes/actions"),
            ));
        }
    }
    Ok(())
}

pub(super) fn resolve_optional_password_secret(
    secret_label: &str,
    secret: Option<&str>,
    env_label: &str,
    env_name: Option<&str>,
) -> Result<Option<String>> {
    validate_password_secret_source(secret_label, secret, env_label, env_name)?;
    if let Some(secret) = secret {
        return Ok(Some(secret.to_string()));
    }
    env_name
        .map(|name| resolve_secret_env(env_label, name))
        .transpose()
}

pub(super) fn resolve_password_secret(
    secret_label: &str,
    secret: Option<&str>,
    env_label: &str,
    env_name: Option<&str>,
) -> Result<String> {
    resolve_optional_password_secret(secret_label, secret, env_label, env_name)?.ok_or_else(|| {
        AppError::new(
            "config_missing",
            format!("{secret_label} or {env_label} is required"),
        )
    })
}

pub(super) fn resolve_secret_env(label: &str, name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| {
        AppError::new(
            "config_missing",
            format!("{label} references unset environment variable: {name}"),
        )
    })
}

pub(super) fn validate_password_secret_source(
    secret_label: &str,
    secret: Option<&str>,
    env_label: &str,
    env_name: Option<&str>,
) -> Result<()> {
    if secret.is_some() && env_name.is_some() {
        return Err(AppError::new(
            "config_invalid",
            format!("{secret_label} and {env_label} cannot both be set"),
        ));
    }
    if let Some(secret) = secret {
        validate_inline_secret(secret_label, secret, env_label)?;
    }
    if let Some(name) = env_name {
        validate_secret_env_name(env_label, name)?;
    }
    Ok(())
}

pub(super) fn validate_inline_secret(label: &str, value: &str, env_label: &str) -> Result<()> {
    if value.starts_with("env:") || value.starts_with("literal:") {
        Err(AppError::new(
            "config_invalid",
            format!(
                "{label} stores the literal secret; remove env:/literal: prefixes or use {env_label}"
            ),
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_secret_env_name(label: &str, name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.ends_with("_SECRET")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && !name.starts_with('_')
        && !name.contains("__");
    if valid {
        Ok(())
    } else {
        Err(AppError::new(
            "config_invalid",
            format!("{label} env var must be UPPER_SNAKE_CASE and end with _SECRET"),
        ))
    }
}

pub(super) fn validate_steps(config: &MailConfig, label: &str, steps: &[ActionStep]) -> Result<()> {
    for (index, step) in steps.iter().enumerate() {
        let mut op_count = 0;
        if !step.add_flags.is_empty() {
            op_count += 1;
        }
        if step.move_to_mailbox_id.is_some() {
            op_count += 1;
        }
        if step.append_to_mailbox_id.is_some() {
            op_count += 1;
        }
        if step.smtp_send.is_some() {
            op_count += 1;
        }
        if op_count != 1 {
            return Err(AppError::new(
                "config_invalid",
                format!("{label}[{index}] must define exactly one operation"),
            ));
        }
        if let Some(params) = &step.smtp_send
            && !params.is_empty()
        {
            return Err(AppError::new(
                "config_invalid",
                format!("{label}[{index}].smtp_send must be an empty object"),
            ));
        }
        for flag in &step.add_flags {
            if flag.trim().is_empty() {
                return Err(AppError::new(
                    "config_invalid",
                    format!("{label}[{index}].add_flags contains an empty flag"),
                ));
            }
        }
        for target in step
            .move_to_mailbox_id
            .iter()
            .chain(step.append_to_mailbox_id.iter())
        {
            validate_config_id("action step mailbox id", target)?;
            let mailbox = config.mailboxes.get(target).ok_or_else(|| {
                AppError::new(
                    "config_invalid",
                    format!("{label}[{index}] references unknown mailbox id: {target}"),
                )
            })?;
            if let Some(kind) = mailbox.first_special_use_kind()
                && step.move_to_mailbox_id.is_some()
                && !kind.can_move_to()
            {
                return Err(AppError::new(
                    "config_invalid",
                    format!("{label}[{index}].move_to_mailbox_id cannot target {target}"),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn single(values: &[String], key: &str) -> Result<String> {
    if values.len() != 1 {
        return Err(AppError::new(
            "invalid_request",
            format!("config key {key} expects exactly one value"),
        ));
    }
    Ok(values[0].clone())
}

pub(super) fn validate_config_id(label: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: {value}"),
        ))
    }
}

pub(super) fn fixed_offset_from_utc_offset(value: &str) -> Option<FixedOffset> {
    let normalized = normalize_utc_offset(value)?;
    if normalized == "UTC" {
        return Some(chrono::Utc.fix());
    }
    let (sign, rest) = match normalized.as_bytes().first()? {
        b'+' => (1, &normalized[1..]),
        b'-' => (-1, &normalized[1..]),
        _ => return None,
    };
    let (hours, minutes) = rest.split_once(':')?;
    let hours = hours.parse::<i32>().ok()?;
    let minutes = minutes.parse::<i32>().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

pub(super) fn validate_language_bcp47(
    label: &str,
    value: &str,
    error_code: &'static str,
) -> Result<()> {
    if is_bcp47_like(value) {
        Ok(())
    } else {
        Err(AppError::new(
            error_code,
            format!("{label} expects a BCP-47-like language tag"),
        ))
    }
}

/// Whether `value` is a language tag afmail will store.
///
/// Well-formedness is afdata's `is_valid_bcp47`, not a second hand-written
/// grammar: the tag reaches template selection through the same conventions
/// everything else here follows, and afmail's own version diverged in both
/// directions — it accepted `chinese` and `abcdefgh` (stored, then silently
/// falling back to `en-US` at render time) while rejecting privateuse `x-…`.
///
/// The two surrounding rules stay afmail's, because they are about a config
/// *field* rather than the grammar: a value the user has to look at must not
/// carry invisible surrounding whitespace, and a config entry gets a length
/// bound so a pathological tag cannot ride along in every rendered path.
pub(super) fn is_bcp47_like(value: &str) -> bool {
    value.trim() == value && value.len() <= 64 && agent_first_data::is_valid_bcp47(value)
}
