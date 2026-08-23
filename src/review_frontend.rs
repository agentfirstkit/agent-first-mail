//! Where a Review UI comes from when the person did not want afmail's.
//!
//! AFUI owns the location and the rules for reaching it:
//! `.afui/frontends/afmail/<ui_kind>/` in the workspace, the same path under
//! the user's config directory globally, workspace first, and a workspace
//! frontend inert until its exact contents have been trusted with
//! `afui frontend enable`. None of that is restated here — [`discover`] calls
//! `agent_first_ui::discover_frontend`, which is the one implementation of it.
//!
//! afmail owns what a frontend *is*: Review UI templates under `templates/`,
//! rendered against the same typed Review render document, escaped by the same
//! rules, and finished by the same trusted runtime as the built-in UI. That is
//! the whole of what an override supplies. It does not get to decide what a
//! save means, what a decision means, whether a base hash still matches, or
//! whether the workspace lock is held — those answers come from the server for
//! a custom UI exactly as they do for the built-in one, so a frontend can look
//! like anything and still cannot make a review say something untrue.

use std::path::{Path, PathBuf};

#[cfg(test)]
use agent_first_ui::FrontendLocation;
use agent_first_ui::{FrontendScope, UiAppIcon, UiErrorKind, UiFrontend};

use crate::error::{AppError, Result};

/// The provider id afmail's frontends are filed under.
pub(crate) const PROVIDER_ID: &str = "afmail";

/// The Review UI contract afmail implements.
///
/// A frontend declares the version it was written against and afmail refuses
/// every other value, because the contract is wider than any single part of
/// it: the typed Review render document, the template keys a frontend may
/// supply, the `<!-- afmail:trusted-runtime -->` marker the injected runtime
/// replaces, and the `data-afmail-*` hooks and `data-copy-*` attributes that
/// runtime reads. A change to any of those is a change to all of them from a
/// frontend author's point of view, so they share one number.
pub(crate) const UI_API_VERSION: &str = "2";

const BUILTIN_APP_ICON: &str = include_str!("review_app_icon.svg");

/// Every Review surface a person can replace, in `ui_kind` form.
///
/// One per surface, not one shared `review`, because `ui_kind` is the override
/// key: a single name would mean replacing the triage UI also replaced the
/// case, draft, and notification UIs. `review_ui_kind` in the review runner
/// maps a surface onto this list, and a test holds the two together.
pub(crate) const UI_KINDS: [&str; 4] = [
    "triage_review",
    "case_review",
    "notification_review",
    "draft_review",
];

/// The resolved overlay object used by AFUI's page assembler.
#[cfg(feature = "review")]
pub(crate) fn review_ui_frontend(workspace_root: &Path, ui_kind: &str) -> Result<UiFrontend> {
    UiFrontend::resolve(workspace_root, PROVIDER_ID, ui_kind, UI_API_VERSION)
        .map_err(|error| frontend_error(ui_kind, &error))
}

/// The icon resolved from the same frontend overlay as the Review page.
#[cfg(feature = "review")]
pub(crate) fn review_app_icon(frontend: &UiFrontend, ui_kind: &str) -> Result<UiAppIcon> {
    frontend
        .app_icon(BUILTIN_APP_ICON)
        .map_err(|error| frontend_error(ui_kind, &error))
}

/// The same lookup with safe mode named rather than read from the environment,
/// for the tests that need to drive both.
///
/// Safe mode is checked before any frontend file is opened, so a frontend that
/// breaks afmail cannot also be what afmail needs in order to recover. AFUI
/// does the checking: discovery, the trust gate, the `ui_api_version`
/// comparison and the "unloadable is an error, never a quiet built-in page"
/// rule are one implementation shared with every other Provider, and afmail's
/// only remaining job is to say those failures in its own error vocabulary.
#[cfg(test)]
pub(crate) fn discover(
    workspace_root: &Path,
    ui_kind: &str,
    safe_mode: bool,
) -> Result<Option<FrontendLocation>> {
    UiFrontend::resolve_with_safe_mode(
        workspace_root,
        PROVIDER_ID,
        ui_kind,
        UI_API_VERSION,
        safe_mode,
    )
    .map(|frontend| frontend.location().cloned())
    .map_err(|error| frontend_error(ui_kind, &error))
}

/// Where a workspace frontend for this surface belongs, whether or not one is
/// installed. AFUI owns the path rule; this only names the scope.
pub(crate) fn workspace_frontend_root(workspace_root: &Path, ui_kind: &str) -> Result<PathBuf> {
    agent_first_ui::frontend_root(
        FrontendScope::Workspace,
        workspace_root,
        PROVIDER_ID,
        ui_kind,
    )
    .map_err(|error| frontend_error(ui_kind, &error))
}

/// A frontend that will not load is an error, never a quiet built-in page.
///
/// Falling back would be the worst of both: the person's own UI is not what
/// they are looking at, and nothing said so. The way out comes from AFUI — in
/// AFUI's words, so it reads the same whichever Provider printed it, and only
/// when AFUI says there is one, since safe mode is no answer to a page afmail
/// itself could not render.
fn frontend_error(ui_kind: &str, error: &agent_first_ui::Error) -> AppError {
    let mut failure = AppError::new(review_error_code(error), error.to_string()).with_details(
        serde_json::json!({
            "provider_id": PROVIDER_ID,
            "ui_kind": ui_kind,
            "ui_api_version": UI_API_VERSION,
        }),
    );
    if let Some(hint) = error.hint() {
        failure = failure.with_hint(hint);
    }
    failure
}

/// `review_<afmail's word for what AFUI classified>`, interned so the code
/// stays `&'static str`.
///
/// One place, rather than the four matches over AFUI's variants this file and
/// `runner/review.rs` used to carry between them.
///
/// Exhaustive over AFUI's closed classification, with **no wildcard arm**. It
/// used to default to `review_frontend_unreadable`, so a closed session
/// runtime, a machine with no browser and an unreadable AFUI config all
/// reported as "the review frontend could not be read" — a sentence that sends
/// a person to a file with nothing wrong with it.
pub(crate) fn review_error_code(error: &agent_first_ui::Error) -> &'static str {
    match error.kind() {
        UiErrorKind::FrontendIncompatible => "review_frontend_incompatible",
        UiErrorKind::FrontendUnreadable => "review_frontend_unreadable",
        UiErrorKind::FrontendUnsafe => "review_template_unsafe",
        UiErrorKind::PageRender => "review_template_render_failed",
        UiErrorKind::PageIncomplete => "review_template_invalid",
        UiErrorKind::WindowUnavailable => "review_window_unavailable",
        UiErrorKind::WindowWaitFailed => "review_window_wait_failed",
        UiErrorKind::DeliveryModeInvalid | UiErrorKind::DeliveryModeNotOffered => {
            "review_delivery_mode_invalid"
        }
        UiErrorKind::LinkAddressUnavailable | UiErrorKind::UpstreamNotProxyable => {
            "review_delivery_unreachable"
        }
        UiErrorKind::RuntimeClosed => "review_runtime_closed",
        UiErrorKind::RuntimeBusy => "review_runtime_busy",
        UiErrorKind::RuntimeMisconfigured
        | UiErrorKind::RuntimeMessageTooLarge
        | UiErrorKind::RuntimeBlob
        | UiErrorKind::RuntimePayload => "review_runtime_failed",
        UiErrorKind::ConfigUnreadable => "review_config_unreadable",
        UiErrorKind::InvalidArgument | UiErrorKind::Io => "review_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("afmail-frontend-{name}-{stamp}"))
    }

    fn write_frontend(root: &Path, ui_kind: &str, manifest: &str) -> PathBuf {
        let frontend = root.join(".afui/frontends").join(PROVIDER_ID).join(ui_kind);
        fs::create_dir_all(&frontend).expect("frontend directory");
        fs::write(frontend.join("frontend.json"), manifest).expect("frontend manifest");
        frontend
    }

    /// Safe mode has to win before the manifest is even parsed, or a frontend
    /// bad enough to break parsing would also break the way out of it.
    #[test]
    fn safe_mode_ignores_a_frontend_that_would_otherwise_be_an_error() {
        let root = temp_root("safe-mode");
        write_frontend(&root, "case_review", "{ not json");
        assert!(
            discover(&root, "case_review", true)
                .expect("safe mode reads no frontend file")
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    /// An untrusted workspace frontend is not an error and not served: trust is
    /// the gate, and it is checked before the manifest is read.
    #[test]
    fn an_untrusted_workspace_frontend_is_inert() {
        let root = temp_root("untrusted");
        write_frontend(&root, "case_review", "{ not json");
        assert!(
            discover(&root, "case_review", false)
                .expect("an untrusted workspace frontend is skipped, not opened")
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn every_ui_kind_is_a_legal_frontend_directory_name() {
        for ui_kind in UI_KINDS {
            assert!(
                agent_first_ui::validate_frontend_identifier("ui_kind", ui_kind).is_ok(),
                "{ui_kind} cannot be a frontend directory"
            );
        }
    }
}
