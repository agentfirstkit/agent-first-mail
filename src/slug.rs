use agent_first_slug::{validate_slug, SlugValidationPolicy};

use crate::error::{AppError, Result};

pub(crate) fn validate_path_segment(label: &str, value: &str) -> Result<()> {
    validate_slug(value, SlugValidationPolicy::LocalPathSegment).map_err(|_| {
        AppError::new(
            "invalid_request",
            format!("invalid {label}: must be a single path segment"),
        )
    })
}

pub(crate) fn validate_config_path_segment(label: &str, value: &str) -> Result<()> {
    validate_path_segment(label, value)?;
    if value.contains('.') {
        return Err(AppError::new(
            "invalid_request",
            format!("invalid {label}: must not contain '.'"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_config_path_segment, validate_path_segment};

    #[test]
    fn validate_path_segment_rejects_separators_whitespace_and_controls() {
        for value in ["", ".", "..", "a/b", "a\\b", "has space", "line\nbreak"] {
            assert!(
                validate_path_segment("test segment", value).is_err(),
                "expected {value:?} to be rejected"
            );
        }
    }

    #[test]
    fn validate_path_segment_accepts_safe_unicode_segments() {
        for value in ["case-123", "draft.md", "現在的nobody-未來的somebody"] {
            assert!(
                validate_path_segment("test segment", value).is_ok(),
                "expected {value:?} to be accepted"
            );
        }
    }

    #[test]
    fn validate_config_path_segment_rejects_dots_only() {
        assert!(validate_config_path_segment("test segment", "support@flfgsc.cn").is_err());
        assert!(validate_config_path_segment("test segment", "法律法规速查小客服").is_ok());
        assert!(validate_config_path_segment("test segment", "support@flfgsc").is_ok());
    }
}
