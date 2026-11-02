mod identity;
pub(crate) use identity::*;

use crate::config::{
    MailConfig, MailDirection, PullImportAs, ResolvedIdentity, match_message_identity_with_profiles,
};
use crate::error::{AppError, Result};
use crate::types::{
    CaseRelationshipEvidence, CaseRelationshipSuggestion, DeliveryEnvelope, MessageFile,
    MessageStatus, RemoteLocation, RemoteState, WorkspaceState,
};
use crate::util::{write_bytes_atomic, write_string_atomic};
use chrono::{DateTime, FixedOffset};
use mail_parser::{MessageParser, TextList};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::{fs, path::Path};

pub(crate) const MAX_MESSAGE_BYTES: u64 = 100 * 1024 * 1024;

pub(crate) struct IngestMessage {
    pub raw_eml: Vec<u8>,
    pub remote_location: Option<RemoteLocation>,
    pub delivery: Option<DeliveryEnvelope>,
    pub import_as: PullImportAs,
    pub direction: MailDirection,
}

pub(crate) enum IngestOutcome {
    Stored {
        message_id: String,
        duplicate_candidate_ids: Vec<String>,
    },
    Merged {
        message_id: String,
        flags_updated: bool,
    },
}
impl IngestOutcome {
    pub(crate) fn message_id(&self) -> &str {
        match self {
            Self::Stored { message_id, .. } | Self::Merged { message_id, .. } => message_id,
        }
    }

    pub(crate) fn duplicate_candidate_warning(&self) -> Option<serde_json::Value> {
        match self {
            Self::Stored {
                message_id,
                duplicate_candidate_ids,
            } if !duplicate_candidate_ids.is_empty() => Some(serde_json::json!({
                "code":"duplicate_candidate",
                "message_id":message_id,
                "candidate_message_ids":duplicate_candidate_ids,
                "evidence":"same_rfc822_message_id_distinct_raw"
            })),
            _ => None,
        }
    }
}

pub(crate) fn ingest_message(
    root: &Path,
    index: &mut ExistingIndex,
    context: &IngestRenderContext,
    config: &MailConfig,
    input: IngestMessage,
) -> Result<IngestOutcome> {
    if input.raw_eml.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(AppError::new(
            "message_too_large",
            "message exceeds the 100 MiB limit",
        ));
    }
    let content_key = message_content_key(&input.raw_eml);
    let workspace = crate::store::Workspace::at(root);
    if let Some(message_id) = index.content_ids.get(&content_key).cloned() {
        let mut message = workspace.read_message_by_id(&message_id)?;
        let mut flags_updated = false;
        if let Some(location) = input.remote_location {
            let state = message
                .remote
                .get_or_insert_with(|| RemoteState { locations: vec![] });
            if let Some(existing) = state.locations.iter_mut().find(|entry| {
                entry.mailbox_name == location.mailbox_name
                    && entry.uid_validity == location.uid_validity
                    && entry.uid == location.uid
            }) {
                flags_updated = existing.flags != location.flags;
                *existing = location.clone();
            } else {
                state.locations.push(location.clone());
            }
            index_location(index, &location, &message_id);
        }
        workspace.persist_message_metadata(&message)?;
        if let Some(delivery) = input.delivery {
            workspace.merge_message_delivery(&message_id, delivery)?;
        }
        let message = workspace.read_message_by_id(&message_id)?;
        workspace.write_message_materialized_cache(&message)?;
        return Ok(IngestOutcome::Merged {
            message_id,
            flags_updated,
        });
    }
    let message_id = stable_message_id(&input.raw_eml, config.resolved_timezone_offset());
    let suggestion = index.suggest_case(&input.raw_eml);
    let mut parsed = crate::mail::parse_message_with_options(
        message_id.clone(),
        &input.raw_eml,
        crate::mail::MessageParseOptions {
            direction: Some(input.direction.as_str().into()),
            workspace: WorkspaceState {
                status: input.import_as.as_str().into(),
                notification_uid: None,
                notification_rfc3339: None,
                origin: None,
                remote_sync: None,
                push: None,
            },
            remote: input.remote_location.clone().map(|location| RemoteState {
                locations: vec![location],
            }),
            received_rfc3339: input
                .delivery
                .as_ref()
                .and_then(|delivery| delivery.received_rfc3339.clone()),
            sent_rfc3339: None,
            attachments: vec![],
        },
    )?;
    if input.direction == MailDirection::Outbound {
        parsed.message.sent_rfc3339 = parsed.message.received_rfc3339.take();
    }
    let rfc822_id = parsed
        .message
        .rfc822_message_id
        .as_deref()
        .map(normalize_message_id)
        .filter(|value| !value.is_empty());
    // A shared Message-ID is weak evidence only. Distinct raw bytes remain
    // separate canonical messages even when all their visible content matches.
    let duplicate_candidate_ids = rfc822_id
        .as_ref()
        .and_then(|id| index.rfc822_messages.get(id))
        .map(|ids| ids.iter().cloned().collect())
        .unwrap_or_default();
    if let Some(delivery) = &input.delivery {
        merge_envelope_to(&mut parsed.message, delivery);
    }
    context.apply_identity(&mut parsed.message);
    parsed.message.case_relationship_suggestion =
        suggestion
            .evidence
            .map(|evidence| CaseRelationshipSuggestion {
                case_uids: suggestion.case_uids,
                evidence,
            });
    create_dir_all(&root.join(".afmail/messages"))?;
    let eml_path = root.join(format!(".afmail/messages/{message_id}.eml"));
    if eml_path.exists() {
        return Err(AppError::new(
            "message_id_conflict",
            format!("message id {message_id} already exists"),
        ));
    }
    workspace.persist_message_metadata(&parsed.message)?;
    if let Some(delivery) = input.delivery {
        workspace.merge_message_delivery(&message_id, delivery)?;
    }
    if let Ok(status) = MessageStatus::parse(input.import_as.as_str())
        && matches!(status, MessageStatus::Spam | MessageStatus::Trashed)
    {
        workspace.set_message_disposition(
            status,
            &message_id,
            None,
            &crate::store::now_rfc3339(),
        )?;
    }
    // Publish raw evidence only after its canonical metadata and disposition.
    // A crash before this point can be replayed from the source; a crash after
    // it leaves enough evidence for the index to rebuild a missing cache.
    write_bytes_atomic(&eml_path, &input.raw_eml, "write eml")?;
    workspace.write_message_materialized_cache(&parsed.message)?;
    if input.import_as == PullImportAs::Triage {
        create_dir_all(&root.join(crate::layout::TRIAGE))?;
        write_string_atomic(
            &root
                .join(crate::layout::TRIAGE)
                .join(format!("{message_id}.md")),
            &crate::store::render_triage_view(root, config, &parsed.message, vec![])?,
        )?;
    }
    if let Some(location) = input.remote_location {
        index_location(index, &location, &message_id);
    }
    index.content_ids.insert(content_key, message_id.clone());
    if let Some(id) = rfc822_id {
        index
            .rfc822_messages
            .entry(id)
            .or_default()
            .insert(message_id.clone());
    }
    Ok(IngestOutcome::Stored {
        message_id,
        duplicate_candidate_ids,
    })
}

fn index_location(index: &mut ExistingIndex, location: &RemoteLocation, message_id: &str) {
    if let (Some(uid_validity), Some(uid)) = (location.uid_validity, location.uid) {
        let key = ImapKey {
            mailbox: location.mailbox_name.clone(),
            uid_validity,
            uid,
        };
        index.imap_keys.insert(key.clone());
        index.imap_locations.insert(key, message_id.into());
    }
}

pub(crate) fn merge_envelope_to(message: &mut MessageFile, delivery: &DeliveryEnvelope) {
    for recipient in &delivery.envelope_to {
        if !message
            .envelope_to
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(recipient))
        {
            message.envelope_to.push(recipient.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DeliverySource;

    #[test]
    fn identical_bytes_merge_delivery_and_match_hidden_recipient() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::store::Workspace::init_command(root, None).unwrap();
        let mut config = MailConfig::load(root).unwrap();
        config.identities[0].email = "hidden@example.com".into();
        config.write(root).unwrap();
        let context = IngestRenderContext::new(root, &config).unwrap();
        let mut index = ExistingIndex::default();
        let raw = b"Message-ID: <hidden@example.net>\r\nFrom: sender@example.net\r\nTo: other@example.net\r\nSubject: hidden recipient\r\n\r\nhello";
        for (number, recipient) in ["hidden@example.com", "alias@example.com"]
            .into_iter()
            .enumerate()
        {
            let outcome = ingest_message(
                root,
                &mut index,
                &context,
                &config,
                IngestMessage {
                    raw_eml: raw.to_vec(),
                    remote_location: None,
                    import_as: PullImportAs::Triage,
                    direction: MailDirection::Inbound,
                    delivery: Some(DeliveryEnvelope {
                        envelope_from: Some("sender@example.net".into()),
                        envelope_to: vec![recipient.into()],
                        received_rfc3339: Some(format!("2026-10-21T00:00:0{}Z", 2 - number)),
                        sources: vec![DeliverySource::File {
                            path: format!("delivery-{number}.eml"),
                        }],
                    }),
                },
            )
            .unwrap();
            assert_eq!(matches!(outcome, IngestOutcome::Merged { .. }), number == 1);
            let message_id = outcome.message_id();
            assert_eq!(
                fs::read(root.join(format!(".afmail/messages/{message_id}.eml"))).unwrap(),
                raw
            );
            let message = crate::store::Workspace::at(root)
                .read_message_by_id(message_id)
                .unwrap();
            assert_eq!(
                message.identity_email.as_deref(),
                Some("hidden@example.com")
            );
            assert_eq!(message.envelope_to.len(), number + 1);
            let metadata: crate::types::MessageMetadata = serde_json::from_slice(
                &fs::read(root.join(format!(".afmail/messages/{message_id}.json"))).unwrap(),
            )
            .unwrap();
            let delivery = metadata.delivery.unwrap();
            assert_eq!(delivery.sources.len(), number + 1);
            assert_eq!(
                delivery.received_rfc3339.unwrap(),
                format!("2026-10-21T00:00:0{}Z", 2 - number)
            );
        }
    }
}
