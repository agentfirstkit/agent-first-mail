use super::*;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RemoteLocationUpdate {
    pub(super) flags_updated: bool,
    pub(super) location_restored: bool,
}

pub(super) fn update_remote_location_flags_from_envelope(
    root: &Path,
    message_id: &str,
    remote: &RemoteEnvelope,
) -> Result<RemoteLocationUpdate> {
    update_remote_location_flags_parts(
        root,
        message_id,
        &remote.mailbox,
        remote.uid_validity,
        remote.uid,
        &remote.flags,
    )
}

pub(super) fn update_remote_location_flags_parts(
    root: &Path,
    message_id: &str,
    mailbox: &str,
    uid_validity: u64,
    uid: u64,
    flags: &[String],
) -> Result<RemoteLocationUpdate> {
    let workspace = crate::store::Workspace::at(root);
    let mut message = workspace.read_message_by_id(message_id)?;
    let mut update = RemoteLocationUpdate::default();
    let mut changed = false;
    let state = message.remote.get_or_insert_with(|| RemoteState {
        locations: Vec::new(),
    });
    if let Some(location) = state.locations.iter_mut().find(|location| {
        location.mailbox_name == mailbox
            && location.uid_validity == Some(uid_validity)
            && location.uid == Some(uid)
    }) {
        if location.missing_rfc3339.take().is_some() {
            update.location_restored = true;
            changed = true;
        }
        update.flags_updated = set_location_flags(location, flags);
        changed |= update.flags_updated;
    } else {
        state.locations.push(RemoteLocation {
            mailbox_id: None,
            mailbox_name: mailbox.to_string(),
            uid_validity: Some(uid_validity),
            uid: Some(uid),
            flags: flags.to_vec(),
            observed_rfc3339: crate::store::now_rfc3339(),
            missing_rfc3339: None,
        });
        update.flags_updated = !flags.is_empty();
        changed = true;
    }
    if changed {
        workspace.persist_message_metadata(&message)?;
        workspace.write_message_materialized_cache(&message)?;
    }
    Ok(update)
}

pub(super) fn set_location_flags(location: &mut RemoteLocation, flags: &[String]) -> bool {
    let flags = canonical_flags(flags.iter().cloned());
    if location.flags == flags {
        return false;
    }
    location.flags = flags;
    true
}
