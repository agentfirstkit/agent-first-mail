//! A person replacing afmail's Review UI, driven as a real process.
//!
//! The point of an AFUI frontend is which bytes reach a browser, so nothing
//! here is checked by calling a function: every case runs `afmail case review`
//! with a stub standing in for the browser, and asserts on the page the stub
//! actually fetched.
//!
//! `AFUI_BROWSER_BINARY` names the stub. It records the `--app=<url>` it was
//! launched with, curls it into a file, and exits — which is the person
//! closing the window, so the review ends and the command returns.
//!
//! `AFUI_CONFIG_DIR` moves AFUI's global directory into the test's temp tree,
//! so the trust store these tests write is theirs and not the developer's.

#![cfg(feature = "review")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_first_ui::test_support::FrontendOnDisk;
use serde_json::{Value, json};

const STUB_BROWSER: &str = r#"#!/bin/sh
set -eu
url=""
for arg in "$@"; do
  case "$arg" in
    --app=*) url="${arg#--app=}" ;;
  esac
done
printf '%s' "$url" > "$AFMAIL_STUB_DIR/url"
curl -sS -o "$AFMAIL_STUB_DIR/body" -w '%{http_code}' "$url" > "$AFMAIL_STUB_DIR/status"
"#;

/// A Review page nobody could mistake for afmail's own.
/// A person's own review page, as small as one is allowed to be.
///
/// The four `data-afmail-*` hooks are the contract afmail declares: the
/// runtime that ships with the page binds to them, and a page without them
/// opens and does nothing. Everything else here is this frontend's own.
const CUSTOM_PAGE: &str = "<!doctype html><html><body>\
<main data-my-review-ui data-afmail-review>MY OWN REVIEW UI\
<div data-afmail-review-content>\
<section data-afmail-agent-workbench><div data-afmail-agent-suggestion-content></div></section>\
</div></main>\
<!-- afmail:trusted-runtime --></body></html>";

/// The same page with one hook renamed — a frontend that looks finished and
/// cannot work.
const CUSTOM_PAGE_MISSING_HOOK: &str = "<!doctype html><html><body>\
<main data-my-review-ui data-afmail-review>MY OWN REVIEW UI\
<div data-my-own-content>\
<section data-afmail-agent-workbench><div data-afmail-agent-suggestion-content></div></section>\
</div></main>\
<!-- afmail:trusted-runtime --></body></html>";

struct Workspace {
    root: PathBuf,
    config_dir: PathBuf,
    stub_dir: PathBuf,
    stub: PathBuf,
    case_uid: String,
}

impl Workspace {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let base = std::env::temp_dir().join(format!(
            "afmail-frontend-{name}-{}-{stamp}",
            std::process::id()
        ));
        let root = base.join("workspace");
        let config_dir = base.join("afui-config");
        let stub_dir = base.join("stub");
        fs::create_dir_all(&root).expect("workspace directory");
        fs::create_dir_all(&config_dir).expect("afui config directory");
        fs::create_dir_all(&stub_dir).expect("stub directory");
        let stub = stub_dir.join("stub-browser.sh");
        fs::write(&stub, STUB_BROWSER).expect("write stub browser");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
        }
        let mut workspace = Self {
            root,
            config_dir,
            stub_dir,
            stub,
            case_uid: String::new(),
        };
        let (status, events) = workspace.afmail(&["init"], &[]);
        assert_eq!(status, 0, "{events:?}");
        let (status, events) = workspace.afmail(
            &["case", "create", "--name", "Review", "--group", "support"],
            &[],
        );
        assert_eq!(status, 0, "{events:?}");
        workspace.case_uid = result(&events)["case_uid"]
            .as_str()
            .expect("created case uid")
            .to_string();
        workspace
    }

    fn afmail(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, Vec<Value>) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_afmail"));
        command
            .current_dir(&self.root)
            .args(args)
            .env("AFUI_BROWSER_BINARY", &self.stub)
            .env("AFUI_CONFIG_DIR", &self.config_dir)
            .env("AFMAIL_STUB_DIR", &self.stub_dir)
            .env_remove("AFUI_SAFE_MODE");
        for (name, value) in env {
            command.env(name, value);
        }
        let output = command.output().expect("run afmail");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let events = format!("{stdout}{stderr}")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line:?}: {e}")))
            .collect();
        (output.status.code().unwrap_or(99), events)
    }

    /// Run one review to completion and return the page the stub fetched.
    fn review(&self, env: &[(&str, &str)]) -> (i32, Vec<Value>, String) {
        let _ = fs::remove_file(self.stub_dir.join("body"));
        let _ = fs::remove_file(self.stub_dir.join("url"));
        let (status, events) = self.afmail(&["case", "review", &self.case_uid], env);
        let body = fs::read_to_string(self.stub_dir.join("body")).unwrap_or_default();
        (status, events, body)
    }

    fn frontend_root(&self, ui_kind: &str) -> PathBuf {
        self.root.join(".afui/frontends/afmail").join(ui_kind)
    }

    /// What `afui frontend init` writes, plus a page of the person's own.
    fn install_frontend(&self, ui_kind: &str, ui_api_version: &str) -> PathBuf {
        let root = self.frontend_root(ui_kind);
        let review = root.join("templates/en-US/review");
        fs::create_dir_all(&review).expect("frontend templates directory");
        fs::create_dir_all(root.join("assets")).expect("frontend assets directory");
        fs::write(
            root.join("frontend.json"),
            serde_json::to_string_pretty(&json!({
                "frontend_id": "my_mail_review",
                "ui_api_version": ui_api_version,
            }))
            .expect("frontend manifest"),
        )
        .expect("write frontend manifest");
        fs::write(review.join("page.html.j2"), CUSTOM_PAGE).expect("write frontend page");
        root
    }

    /// Replace an installed frontend's page and re-trust it, so the change is
    /// what the test is about rather than the trust gate.
    fn replace_page(&self, ui_kind: &str, page: &str) {
        fs::write(
            self.frontend_root(ui_kind)
                .join("templates/en-US/review/page.html.j2"),
            page,
        )
        .expect("write frontend page");
        self.trust(ui_kind);
    }
    /// What `afui frontend enable` records, through AFUI's own code.
    ///
    /// The fingerprint used to be recomputed here so that a change to AFUI's
    /// algorithm showed up as this suite failing to serve a trusted frontend.
    /// Calling AFUI's own helper is the stronger version of that: there is no
    /// second implementation left to drift.
    fn trust(&self, ui_kind: &str) {
        FrontendOnDisk::at(self.frontend_root(ui_kind), "afmail", ui_kind)
            .trust_in(&self.config_dir)
            .expect("trust the frontend");
    }
}

fn event(events: &[Value], kind: &str) -> Value {
    events
        .iter()
        .find(|event| event["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} event in {events:?}"))
        .clone()
}

fn result(events: &[Value]) -> Value {
    event(events, "result")["result"].clone()
}

fn ready(events: &[Value]) -> Value {
    events
        .iter()
        .find(|event| event["progress"]["phase"] == "review_ready")
        .unwrap_or_else(|| panic!("no review_ready progress in {events:?}"))["progress"]
        .clone()
}

fn is_builtin_page(body: &str) -> bool {
    body.contains("data-afmail-review") && !body.contains("MY OWN REVIEW UI")
}

/// The whole lifecycle, in the order a person lives it.
///
/// One test rather than six, because each step is the previous step's
/// workspace one edit later: what makes the trust gate meaningful is that the
/// same directory served the built-in page a moment earlier.
#[test]
fn a_user_frontend_serves_the_review_only_once_it_is_installed_compatible_and_trusted() {
    let workspace = Workspace::new("lifecycle");

    // 1. Nothing installed: afmail's own Review UI, and nothing claimed
    //    otherwise.
    let (status, events, body) = workspace.review(&[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(is_builtin_page(&body), "{body}");
    assert!(ready(&events).get("review_frontend_id").is_none());
    assert_eq!(result(&events)["ended"], "closed");

    // 2. Installed but not trusted: still afmail's own UI. A workspace
    //    frontend is inert until someone says otherwise, and the ready event
    //    is where an Agent can see that it is not serving.
    workspace.install_frontend("case_review", "2");
    let (status, events, untrusted) = workspace.review(&[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(is_builtin_page(&untrusted), "{untrusted}");
    assert!(ready(&events).get("review_frontend_id").is_none());

    // 3. Trusted: the person's own markup is what a browser receives.
    workspace.trust("case_review");
    let (status, events, trusted) = workspace.review(&[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(trusted.contains("MY OWN REVIEW UI"), "{trusted}");
    assert!(trusted.contains("data-my-review-ui"), "{trusted}");
    assert_ne!(trusted, untrusted, "the override changed nothing");
    assert_eq!(ready(&events)["review_frontend_id"], "my_mail_review");
    // The trusted runtime is still afmail's, and still bound to the session's
    // own nonce: a custom UI gets behavior it did not write and cannot forge.
    assert!(trusted.contains("data-afmail-review") || trusted.contains("<script nonce="));

    // 4. Edited after being trusted: the fingerprint no longer matches, so the
    //    frontend is inert again and afmail's UI is back.
    fs::write(
        workspace
            .frontend_root("case_review")
            .join("templates/en-US/review/page.html.j2"),
        CUSTOM_PAGE.replace("MY OWN REVIEW UI", "EDITED AFTER TRUST"),
    )
    .expect("edit the trusted frontend");
    let (status, events, edited) = workspace.review(&[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(!edited.contains("EDITED AFTER TRUST"), "{edited}");
    assert!(is_builtin_page(&edited), "{edited}");
    assert!(ready(&events).get("review_frontend_id").is_none());

    // 5. Safe mode with a trusted frontend: afmail's UI, no questions asked.
    workspace.trust("case_review");
    let (status, events, safe) = workspace.review(&[("AFUI_SAFE_MODE", "1")]);
    assert_eq!(status, 0, "{events:?}");
    assert!(is_builtin_page(&safe), "{safe}");
    assert!(ready(&events).get("review_frontend_id").is_none());

    // …and the same frontend still serves when safe mode is not set, so step 5
    // proved safe mode rather than another revoked fingerprint.
    let (_, _, still_trusted) = workspace.review(&[]);
    assert!(
        still_trusted.contains("EDITED AFTER TRUST"),
        "{still_trusted}"
    );
}

/// The one behaviour a fallback would destroy.
///
/// A frontend afmail cannot use is an error naming safe mode. It is never a
/// quietly-substituted built-in page, because that is indistinguishable from
/// the override having worked.
#[test]
fn an_incompatible_frontend_is_an_error_naming_safe_mode_and_never_a_quiet_builtin_page() {
    let workspace = Workspace::new("incompatible");
    workspace.install_frontend("case_review", "99");
    workspace.trust("case_review");

    let (status, events, body) = workspace.review(&[]);
    assert_eq!(status, 1, "{events:?}");
    assert!(
        body.is_empty() && !workspace.stub_dir.join("url").exists(),
        "no window may open onto a Review UI afmail could not load"
    );
    let error = event(&events, "error")["error"].clone();
    assert_eq!(error["code"], "review_frontend_incompatible");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ui_api_version 99"),
        "{error}"
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("AFUI_SAFE_MODE=1"),
        "{error}"
    );

    // Safe mode is the documented way out, and it works on exactly this
    // workspace without touching the frontend.
    let (status, events, body) = workspace.review(&[("AFUI_SAFE_MODE", "1")]);
    assert_eq!(status, 0, "{events:?}");
    assert!(is_builtin_page(&body), "{body}");
}

/// A frontend that will not parse fails the same way, for the same reason.
#[test]
fn an_unreadable_frontend_manifest_is_an_error_rather_than_a_fallback() {
    let workspace = Workspace::new("unreadable");
    let root = workspace.install_frontend("case_review", "2");
    fs::write(root.join("frontend.json"), "{ not json").expect("break the manifest");
    workspace.trust("case_review");

    let (status, events, body) = workspace.review(&[]);
    assert_eq!(status, 1, "{events:?}");
    assert!(body.is_empty(), "{body}");
    let error = event(&events, "error")["error"].clone();
    assert_eq!(error["code"], "review_frontend_unreadable");
    assert!(
        error["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("AFUI_SAFE_MODE=1"),
        "{error}"
    );
}

/// A frontend that dropped one of afmail's hooks is refused before a window
/// opens, rather than serving a page whose runtime finds nothing.
///
/// This is the failure the contract exists for: the page parses, renders,
/// passes the markup guard, and carries the trusted runtime — everything
/// except the one hook the runtime binds every form through. Nobody looking
/// at it would see the problem; a person clicking Settle on it would.
#[test]
fn a_frontend_missing_one_of_afmails_hooks_is_refused_rather_than_served() {
    let workspace = Workspace::new("missing-hook");
    workspace.install_frontend("case_review", "2");
    workspace.replace_page("case_review", CUSTOM_PAGE_MISSING_HOOK);

    let (status, events, body) = workspace.review(&[]);
    assert_eq!(status, 1, "{events:?}");
    assert!(body.is_empty(), "{body}");
    let error = event(&events, "error")["error"].clone();
    assert_eq!(error["code"], "review_template_invalid");
    // The name of the hook, so the fix is the next thing the author reads.
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("data-afmail-review-content"),
        "{error}"
    );

    // And the same frontend with the hook restored serves.
    workspace.replace_page("case_review", CUSTOM_PAGE);
    let (status, events, served) = workspace.review(&[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(served.contains("MY OWN REVIEW UI"), "{served}");
}

/// `ui_kind` is the override key, so replacing one surface replaces one
/// surface. A case-review frontend must not turn up in a draft review.
#[test]
fn a_frontend_replaces_exactly_the_surface_it_was_installed_for() {
    let workspace = Workspace::new("per-surface");
    workspace.install_frontend("draft_review", "2");
    workspace.trust("draft_review");

    let (status, events, body) = workspace.review(&[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(is_builtin_page(&body), "{body}");
    assert!(ready(&events).get("review_frontend_id").is_none());
}

/// `afmail render templates` puts the Review UI where afmail reads it from,
/// and content where content is read from — never a copy in both places.
#[test]
fn exporting_built_in_templates_seeds_an_installed_frontend_and_not_the_content_tree() {
    let workspace = Workspace::new("export");

    let (status, events) = workspace.afmail(&["render", "templates"], &[]);
    assert_eq!(status, 0, "{events:?}");
    assert!(
        workspace
            .root
            .join("templates/en-US/case/case.md.j2")
            .is_file()
    );
    assert!(!workspace.root.join("templates/en-US/review").exists());
    assert!(!workspace.root.join("templates/shared/review").exists());
    assert_eq!(result(&events)["frontend_count"], 0);

    workspace.install_frontend("case_review", "2");
    let (status, events) = workspace.afmail(&["render", "templates"], &[]);
    assert_eq!(status, 0, "{events:?}");
    let frontend = workspace.frontend_root("case_review");
    assert!(
        frontend
            .join("templates/zh-CN/review/message.html.j2")
            .is_file()
    );
    assert!(
        frontend
            .join("templates/shared/review/style.css.j2")
            .is_file()
    );
    // The page the person already wrote is theirs; export never overwrites it.
    assert_eq!(
        fs::read_to_string(frontend.join("templates/en-US/review/page.html.j2"))
            .unwrap_or_default(),
        CUSTOM_PAGE
    );
    assert!(result(&events)["frontend_count"].as_u64().unwrap_or(0) > 0);
    // Still no Review UI in the content tree, from either run.
    assert!(!workspace.root.join("templates/en-US/review").exists());
}
