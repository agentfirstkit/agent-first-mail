use crate::config::MailConfig;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Serialize)]
struct RouteReadiness {
    configured: bool,
    feature_enabled: bool,
    ready: bool,
    missing_config: Vec<&'static str>,
}

impl RouteReadiness {
    fn new(configured: bool, feature_enabled: bool, required: &[(&'static str, bool)]) -> Self {
        let missing_config: Vec<_> = required
            .iter()
            .filter_map(|(key, present)| (!present).then_some(*key))
            .collect();
        Self {
            configured,
            feature_enabled,
            ready: configured && feature_enabled && missing_config.is_empty(),
            missing_config,
        }
    }
}

/// Configuration readiness only: do not resolve secrets, connect to a provider,
/// or create channel directories as a side effect of inspecting status.
pub(super) fn readiness(config: &MailConfig) -> Value {
    let imap = RouteReadiness::new(
        config.imap.host.is_some(),
        true,
        &[
            ("imap.host", config.imap.host.is_some()),
            ("imap.username", config.imap.username.is_some()),
            (
                "imap.password_secret",
                config.imap.password_secret.is_some(),
            ),
        ],
    );
    let cloudflare = RouteReadiness::new(
        config.cloudflare.is_configured(),
        cfg!(feature = "cloudflare"),
        &[
            (
                "cloudflare.r2.bucket",
                config.cloudflare.r2.bucket.is_some(),
            ),
            (
                "cloudflare.r2.access_key_id",
                config.cloudflare.r2.access_key_id.is_some(),
            ),
            (
                "cloudflare.r2.secret_access_key_secret",
                config.cloudflare.r2.secret_access_key_secret.is_some(),
            ),
        ],
    );
    let files = RouteReadiness::new(
        !config.files.sources.is_empty(),
        cfg!(feature = "files"),
        &[("files.sources", !config.files.sources.is_empty())],
    );
    let mut smtp_required = vec![("smtp.host", config.smtp.host.is_some())];
    smtp_required.extend(
        crate::config::missing_smtp_auth_config(
            config.smtp.username.as_deref(),
            config.smtp.password_secret.as_deref(),
        )
        .into_iter()
        .map(|key| (key, false)),
    );
    let smtp = RouteReadiness::new(
        config.smtp.host.is_some(),
        cfg!(feature = "smtp"),
        &smtp_required,
    );
    let cloudflare_send = RouteReadiness::new(
        config.cloudflare.sending.api_token_secret.is_some(),
        cfg!(feature = "cloudflare"),
        &[
            (
                "cloudflare.account_id",
                config.cloudflare.account_id.is_some(),
            ),
            (
                "cloudflare.sending.api_token_secret",
                config.cloudflare.sending.api_token_secret.is_some(),
            ),
        ],
    );
    let placeholder_identity = config.identities.iter().any(|identity| {
        identity.email.eq_ignore_ascii_case("me@example.com")
            || (identity.identity == "me" && identity.name == "Me")
    });
    let mut imap_pull = imap.clone();
    if config.default_pull_ids().is_empty() {
        imap_pull.ready = false;
        imap_pull
            .missing_config
            .push("actions.pull.default_mailbox_ids");
    }
    let sources = [&imap_pull, &cloudflare, &files];
    let selected_sources: Vec<_> = sources
        .into_iter()
        .filter(|route| route.configured)
        .collect();
    let ready_for_pull =
        !selected_sources.is_empty() && selected_sources.iter().all(|route| route.ready);
    let mut missing_config: Vec<_> = selected_sources
        .iter()
        .flat_map(|route| route.missing_config.iter().copied())
        .collect();
    if selected_sources.is_empty() {
        missing_config.extend(imap.missing_config.iter().copied());
    }
    let email_selected = imap.configured
        || cloudflare.configured
        || smtp.configured
        || cloudflare_send.configured
        || !files.configured;
    let mut email_ready = true;
    if email_selected {
        for step in &config.actions.draft.send.steps {
            let route = if step.smtp_send.is_some() {
                Some(&smtp)
            } else if step.cloudflare_send.is_some() {
                Some(&cloudflare_send)
            } else if step.append_to_mailbox_id.is_some()
                || step.move_to_mailbox_id.is_some()
                || !step.add_flags.is_empty()
            {
                Some(&imap)
            } else {
                None
            };
            if let Some(route) = route {
                email_ready &= route.ready;
                missing_config.extend(route.missing_config.iter().copied());
            }
        }
    }
    if placeholder_identity {
        missing_config.push("identities (replace Me <me@example.com>)");
    }
    missing_config.sort_unstable();
    missing_config.dedup();
    let missing_special_use_mailbox_ids: Vec<_> = ["sent", "archive", "junk", "trash", "drafts"]
        .into_iter()
        .filter(|id| !config.mailboxes.contains_key(*id))
        .collect();
    json!({
        "ready_for_pull": ready_for_pull,
        "ready_for_send": !placeholder_identity && (files.ready || (email_selected && email_ready)),
        "imap_configured": imap.ready,
        "smtp_configured": smtp.ready,
        "placeholder_identity": placeholder_identity,
        "special_use_mapping_complete": missing_special_use_mailbox_ids.is_empty(),
        "missing_special_use_mailbox_ids": missing_special_use_mailbox_ids,
        "missing_config": missing_config,
        "sources": {"imap": imap_pull, "cloudflare": cloudflare, "files": files},
        "senders": {"smtp": smtp, "cloudflare": cloudflare_send, "files": files},
        "remote_validation_command": "afmail remote test"
    })
}
