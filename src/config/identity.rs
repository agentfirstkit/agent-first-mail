use crate::error::{AppError, Result};
use crate::frontmatter::IdentityFileFrontmatter;
use crate::markdown::read_doc;
use crate::types::MessageFile;
use lettre::address::Address;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    pub identity: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResolvedIdentity {
    pub identity: String,
    pub name: String,
    pub email: String,
    pub default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageIdentityMatch {
    pub identity: Option<String>,
    pub identity_email: Option<String>,
    pub identity_match: String,
    pub identity_candidates: Vec<String>,
    pub observed_recipient_emails: Vec<String>,
}

#[derive(Clone, Debug)]
struct RecipientObservation {
    email: String,
    display_name: Option<String>,
}

#[derive(Clone, Debug)]
struct IdentityFile {
    identity: String,
    name: Option<String>,
    footer: Option<String>,
    notes: Option<String>,
}

impl IdentityConfig {
    pub(super) fn validate(&self) -> Result<()> {
        crate::slug::validate_config_path_segment("identity slug", &self.identity)
            .map_err(|err| AppError::new("config_invalid", err.message))?;
        if self.name.trim().is_empty() {
            return Err(AppError::new(
                "config_invalid",
                format!("identities.{} name is required", self.identity),
            ));
        }
        validate_identity_email(&self.identity, &self.email)
    }
}

impl super::MailConfig {
    pub(super) fn validate_identities(&self) -> Result<()> {
        if self.identities.is_empty() {
            return Err(AppError::new(
                "config_invalid",
                "identities must contain at least one workspace identity",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut default_count = 0usize;
        for identity in &self.identities {
            identity.validate()?;
            if !seen.insert(identity.identity.clone()) {
                return Err(AppError::new(
                    "config_invalid",
                    format!("duplicate identity slug: {}", identity.identity),
                ));
            }
            if identity.default {
                default_count += 1;
            }
        }
        if default_count != 1 {
            return Err(AppError::new(
                "config_invalid",
                "identities must contain exactly one default identity",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_identity_files(&self, workspace_root: &Path) -> Result<()> {
        self.load_identity_files(workspace_root).map(|_| ())
    }

    pub fn default_identity(&self) -> Result<&IdentityConfig> {
        self.identities
            .iter()
            .find(|identity| identity.default)
            .ok_or_else(|| AppError::new("config_invalid", "no default identity configured"))
    }

    pub fn identity_emails(&self) -> Vec<String> {
        let mut out = Vec::new();
        for identity in &self.identities {
            let email = normalize_email(&identity.email);
            if !email.is_empty() && !out.iter().any(|existing| existing == &email) {
                out.push(email);
            }
        }
        out
    }

    pub fn resolve_identity(
        &self,
        workspace_root: &Path,
        identity: Option<&str>,
    ) -> Result<ResolvedIdentity> {
        let slug = identity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                self.default_identity()
                    .map(|identity| identity.identity.clone())
                    .unwrap_or_default()
            });
        let config = self
            .identities
            .iter()
            .find(|candidate| candidate.identity == slug)
            .ok_or_else(|| {
                AppError::new("unknown_identity", format!("unknown identity: {slug}"))
            })?;
        let files = self.load_identity_files(workspace_root)?;
        Ok(resolve_identity_from_file(
            config,
            files.get(&config.identity),
        ))
    }

    pub fn identity_profiles(&self, workspace_root: &Path) -> Result<Vec<ResolvedIdentity>> {
        let files = self.load_identity_files(workspace_root)?;
        Ok(self
            .identities
            .iter()
            .map(|identity| resolve_identity_from_file(identity, files.get(&identity.identity)))
            .collect())
    }

    pub fn match_message_identity(
        &self,
        workspace_root: &Path,
        message: &MessageFile,
    ) -> Result<MessageIdentityMatch> {
        Ok(match_message_identity(
            &self.identity_profiles(workspace_root)?,
            message,
        ))
    }

    fn load_identity_files(&self, workspace_root: &Path) -> Result<BTreeMap<String, IdentityFile>> {
        let dir = workspace_root.join("identities");
        if !dir.exists() {
            return Ok(BTreeMap::new());
        }
        if !dir.is_dir() {
            return Err(AppError::new(
                "config_invalid",
                "identities path must be a directory",
            ));
        }
        let configured = self
            .identities
            .iter()
            .map(|identity| identity.identity.as_str())
            .collect::<BTreeSet<_>>();
        let mut paths = fs::read_dir(&dir)
            .map_err(|e| AppError::io("read identities", &e))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| AppError::io("read identities", &e))?;
        paths.sort();
        let mut files = BTreeMap::new();
        for path in paths {
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            let file = read_identity_file(&path, &configured)?;
            if files.insert(file.identity.clone(), file).is_some() {
                return Err(AppError::new(
                    "config_invalid",
                    format!("duplicate identity file for {}", path.display()),
                ));
            }
        }
        Ok(files)
    }
}

fn validate_identity_email(identity: &str, email: &str) -> Result<()> {
    email.parse::<Address>().map(|_| ()).map_err(|e| {
        AppError::new(
            "config_invalid",
            format!("invalid email for identity {identity}: {e}"),
        )
    })
}

fn read_identity_file(path: &Path, configured: &BTreeSet<&str>) -> Result<IdentityFile> {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            AppError::new(
                "config_invalid",
                format!("invalid identity file name: {}", path.display()),
            )
        })?;
    let text = fs::read_to_string(path).map_err(|e| AppError::io("read identity", &e))?;
    let (frontmatter, body) = read_doc::<IdentityFileFrontmatter>(&text).map_err(|e| {
        AppError::new(
            "config_invalid",
            format!("invalid identity file {}: {}", path.display(), e.message),
        )
    })?;
    if frontmatter.kind != "identity" {
        return Err(AppError::new(
            "config_invalid",
            format!("identity file {} kind must be identity", path.display()),
        ));
    }
    if frontmatter.identity != stem {
        return Err(AppError::new(
            "config_invalid",
            format!(
                "identity file {} identity must match file stem {stem}",
                path.display()
            ),
        ));
    }
    if !configured.contains(frontmatter.identity.as_str()) {
        return Err(AppError::new(
            "config_invalid",
            format!(
                "identity file {} references unknown identity {}",
                path.display(),
                frontmatter.identity
            ),
        ));
    }
    Ok(IdentityFile {
        identity: frontmatter.identity,
        name: frontmatter
            .name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        footer: normalize_optional_block(frontmatter.footer.as_deref()),
        notes: normalize_optional_block(Some(&body)),
    })
}

fn resolve_identity_from_file(
    identity: &IdentityConfig,
    file: Option<&IdentityFile>,
) -> ResolvedIdentity {
    ResolvedIdentity {
        identity: identity.identity.clone(),
        name: file
            .and_then(|file| file.name.clone())
            .unwrap_or_else(|| identity.name.clone()),
        email: identity.email.clone(),
        default: identity.default,
        footer: file.and_then(|file| file.footer.clone()),
        notes: file.and_then(|file| file.notes.clone()),
    }
}

fn match_message_identity(
    profiles: &[ResolvedIdentity],
    message: &MessageFile,
) -> MessageIdentityMatch {
    let observations = recipient_observations(message);
    let observed_recipient_emails =
        unique_emails(observations.iter().map(|obs| obs.email.as_str()));
    let by_email = profiles_by_email(profiles);
    for email in &observed_recipient_emails {
        let Some(candidates) = by_email.get(email) else {
            continue;
        };
        if candidates.len() == 1 {
            let identity = candidates[0];
            return MessageIdentityMatch {
                identity: Some(identity.identity.clone()),
                identity_email: Some(identity.email.clone()),
                identity_match: "email".to_string(),
                identity_candidates: Vec::new(),
                observed_recipient_emails: Vec::new(),
            };
        }
        let names = observations
            .iter()
            .filter(|obs| &obs.email == email)
            .filter_map(|obs| obs.display_name.as_deref())
            .map(normalize_display_name)
            .filter(|name| !name.is_empty())
            .collect::<BTreeSet<_>>();
        let name_matches = candidates
            .iter()
            .filter(|identity| names.contains(&normalize_display_name(&identity.name)))
            .map(|identity| (*identity).clone())
            .collect::<Vec<_>>();
        if name_matches.len() == 1 {
            let identity = name_matches[0].clone();
            return MessageIdentityMatch {
                identity: Some(identity.identity),
                identity_email: Some(identity.email),
                identity_match: "name".to_string(),
                identity_candidates: Vec::new(),
                observed_recipient_emails: Vec::new(),
            };
        }
        return MessageIdentityMatch {
            identity: None,
            identity_email: Some(candidates[0].email.clone()),
            identity_match: "multiple".to_string(),
            identity_candidates: candidates
                .iter()
                .map(|identity| identity.identity.clone())
                .collect(),
            observed_recipient_emails: Vec::new(),
        };
    }
    MessageIdentityMatch {
        identity: None,
        identity_email: None,
        identity_match: "unmatched".to_string(),
        identity_candidates: Vec::new(),
        observed_recipient_emails,
    }
}

fn recipient_observations(message: &MessageFile) -> Vec<RecipientObservation> {
    let mut out = Vec::new();
    for value in message
        .delivered_to
        .iter()
        .chain(message.x_original_to.iter())
        .chain(message.envelope_to.iter())
        .chain(message.to.iter())
        .chain(message.cc.iter())
    {
        if let Some(obs) = parse_recipient_observation(value) {
            out.push(obs);
        }
    }
    out
}

fn parse_recipient_observation(value: &str) -> Option<RecipientObservation> {
    let email = extract_email(value);
    if email.is_empty() {
        return None;
    }
    Some(RecipientObservation {
        email,
        display_name: extract_display_name(value),
    })
}

fn extract_email(value: &str) -> String {
    let trimmed = value.trim();
    if let (Some(start), Some(end)) = (trimmed.rfind('<'), trimmed.rfind('>')) {
        if start < end {
            return normalize_email(&trimmed[start + 1..end]);
        }
    }
    normalize_email(trimmed)
}

fn extract_display_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let start = trimmed.rfind('<')?;
    let name = trimmed[..start].trim().trim_matches('"').trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn normalize_email(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_display_name(value: &str) -> String {
    value.trim().trim_matches('"').trim().to_ascii_lowercase()
}

fn normalize_optional_block(value: Option<&str>) -> Option<String> {
    value
        .map(|value| value.trim_matches('\n').trim_end().to_string())
        .filter(|value| !value.trim().is_empty())
}

fn unique_emails<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let normalized = normalize_email(value);
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }
    out
}

fn profiles_by_email(profiles: &[ResolvedIdentity]) -> BTreeMap<String, Vec<&ResolvedIdentity>> {
    let mut by_email: BTreeMap<String, Vec<&ResolvedIdentity>> = BTreeMap::new();
    for identity in profiles {
        by_email
            .entry(normalize_email(&identity.email))
            .or_default()
            .push(identity);
    }
    by_email
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MessageAuthentication, RemoteState, WorkspaceState};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("afmail-identity-{name}-{stamp}"))
    }

    fn message(to: Vec<String>, delivered_to: Vec<String>) -> MessageFile {
        MessageFile {
            schema_name: "message".to_string(),
            schema_version: 1,
            message_id: "message_id".to_string(),
            rfc822_message_id: None,
            in_reply_to: None,
            references: Vec::new(),
            remote: None::<RemoteState>,
            direction: Some("inbound".to_string()),
            subject: None,
            from: Some("sender@example.com".to_string()),
            to,
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            sender: None,
            delivered_to,
            x_original_to: Vec::new(),
            envelope_to: Vec::new(),
            list_id: None,
            mailing_list_headers: Vec::new(),
            authentication: MessageAuthentication::default(),
            received_rfc3339: None,
            sent_rfc3339: None,
            body_text: String::new(),
            eml_path: None,
            attachments: Vec::new(),
            contact: None,
            identity: None,
            identity_email: None,
            identity_match: None,
            identity_candidates: Vec::new(),
            observed_recipient_emails: Vec::new(),
            workspace: WorkspaceState {
                status: "triage".to_string(),
                archive_uid: None,
                archived_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
        }
    }

    fn profiles() -> Vec<ResolvedIdentity> {
        vec![
            ResolvedIdentity {
                identity: "support".to_string(),
                name: "Support Team".to_string(),
                email: "hello@example.com".to_string(),
                default: true,
                footer: None,
                notes: None,
            },
            ResolvedIdentity {
                identity: "sales".to_string(),
                name: "Sales Team".to_string(),
                email: "hello@example.com".to_string(),
                default: false,
                footer: None,
                notes: None,
            },
        ]
    }

    #[test]
    fn same_email_matches_display_name() {
        let result = match_message_identity(
            &profiles(),
            &message(
                vec!["Support Team <hello@example.com>".to_string()],
                Vec::new(),
            ),
        );
        assert_eq!(result.identity.as_deref(), Some("support"));
        assert_eq!(result.identity_match, "name");
    }

    #[test]
    fn same_email_without_name_is_multiple() {
        let result = match_message_identity(
            &profiles(),
            &message(vec!["hello@example.com".to_string()], Vec::new()),
        );
        assert_eq!(result.identity_match, "multiple");
        assert_eq!(result.identity_candidates, vec!["support", "sales"]);
    }

    #[test]
    fn unknown_email_is_unmatched_with_observed_recipients() {
        let result = match_message_identity(
            &profiles(),
            &message(Vec::new(), vec!["other@example.com".to_string()]),
        );
        assert_eq!(result.identity_match, "unmatched");
        assert_eq!(result.observed_recipient_emails, vec!["other@example.com"]);
    }

    #[test]
    fn identity_file_enriches_without_overriding_address() {
        let root = temp_root("file");
        let _ = fs::create_dir_all(root.join("identities"));
        let _ = fs::write(
            root.join("identities/support.md"),
            "---\nkind: identity\nidentity: support\nname: Help Desk\nfooter: |\n  --\n  Help Desk\n---\nUse for support mail.\n",
        );
        let config = super::super::MailConfig {
            identities: vec![IdentityConfig {
                identity: "support".to_string(),
                name: "Support Team".to_string(),
                email: "hello@example.com".to_string(),
                default: true,
            }],
            ..super::super::MailConfig::default()
        };
        let resolved = config.resolve_identity(&root, Some("support"));
        assert!(resolved.is_ok());
        if let Ok(identity) = resolved {
            assert_eq!(identity.name, "Help Desk");
            assert_eq!(identity.email, "hello@example.com");
            assert_eq!(identity.footer.as_deref(), Some("--\nHelp Desk"));
            assert_eq!(identity.notes.as_deref(), Some("Use for support mail."));
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unicode_identity_file_enriches_without_overriding_address() {
        let root = temp_root("unicode-file");
        let _ = fs::create_dir_all(root.join("identities"));
        let _ = fs::write(
            root.join("identities/身份一.md"),
            "---\nkind: identity\nidentity: 身份一\nname: Unicode Identity\n---\nUnicode identity notes.\n",
        );
        let config = super::super::MailConfig {
            identities: vec![IdentityConfig {
                identity: "身份一".to_string(),
                name: "Support".to_string(),
                email: "unicode@example.com".to_string(),
                default: true,
            }],
            ..super::super::MailConfig::default()
        };
        let resolved = config.resolve_identity(&root, Some("身份一"));
        assert!(resolved.is_ok());
        if let Ok(identity) = resolved {
            assert_eq!(identity.name, "Unicode Identity");
            assert_eq!(identity.email, "unicode@example.com");
            assert_eq!(identity.notes.as_deref(), Some("Unicode identity notes."));
        }
        let _ = fs::remove_dir_all(root);
    }
}
