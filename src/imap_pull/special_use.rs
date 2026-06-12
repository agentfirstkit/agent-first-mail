use super::*;

pub fn resolve_special_use_from_mailboxes(
    config: &MailConfig,
    kind: SpecialUseKind,
    mailboxes: &[MailboxInfo],
) -> SpecialUseTarget {
    if let Some(id) = config.mailbox_id_for_special_use(kind) {
        if let Ok(mailbox_config) = config.mailbox(&id) {
            if let Some(folder) = &mailbox_config.mailbox_name {
                return special_use_target(
                    config,
                    kind,
                    folder.clone(),
                    SpecialUseSource::Mailboxes,
                );
            }
            if let Some(special_use) = &mailbox_config.special_use {
                if let Some(mailbox) = mailboxes.iter().find(|mailbox| {
                    mailbox
                        .attributes
                        .iter()
                        .any(|attribute| attribute.eq_ignore_ascii_case(special_use))
                }) {
                    return special_use_target(
                        config,
                        kind,
                        mailbox.name.clone(),
                        SpecialUseSource::Mailboxes,
                    );
                }
            }
        }
    }
    if let Some(mailbox) = mailboxes
        .iter()
        .find(|mailbox| mailbox.special_use == Some(kind))
    {
        return special_use_target(
            config,
            kind,
            mailbox.name.clone(),
            SpecialUseSource::Rfc6154Attribute,
        );
    }
    if let Some(mailbox) = mailboxes
        .iter()
        .find(|mailbox| mailbox_name_matches_fallback(mailbox, kind))
    {
        return special_use_target(
            config,
            kind,
            mailbox.name.clone(),
            SpecialUseSource::FallbackName,
        );
    }
    special_use_target(
        config,
        kind,
        fallback_names(kind)[0].to_string(),
        SpecialUseSource::FallbackName,
    )
}

pub(super) fn special_use_target(
    config: &MailConfig,
    kind: SpecialUseKind,
    folder: String,
    source: SpecialUseSource,
) -> SpecialUseTarget {
    SpecialUseTarget {
        kind,
        mailbox_name: folder,
        source,
        attribute: kind.attribute(),
        flag: config.special_use_flag(kind),
        can_move_to: kind.can_move_to(),
    }
}

pub(super) fn selected_targets_by_folder(
    config: &MailConfig,
    mailboxes: &[MailboxInfo],
) -> BTreeMap<String, Vec<Value>> {
    let mut selected: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for kind in special_use_kinds().iter().copied() {
        let target = resolve_special_use_from_mailboxes(config, kind, mailboxes);
        if mailboxes
            .iter()
            .any(|mailbox| mailbox.name == target.mailbox_name)
        {
            selected
                .entry(target.mailbox_name.clone())
                .or_default()
                .push(special_use_target_json(&target, true));
        }
    }
    selected
}

pub(super) fn special_use_matches_for_mailbox(
    config: &MailConfig,
    mailbox: &MailboxInfo,
) -> Vec<Value> {
    let mut matches = Vec::new();
    for kind in special_use_kinds().iter().copied() {
        let configured = config
            .mailbox_id_for_special_use(kind)
            .and_then(|id| config.mailbox(&id).ok().cloned());
        if configured.is_some_and(|configured| {
            configured.mailbox_name.as_deref() == Some(mailbox.name.as_str())
                || configured
                    .special_use
                    .as_deref()
                    .is_some_and(|special_use| {
                        mailbox
                            .attributes
                            .iter()
                            .any(|attribute| attribute.eq_ignore_ascii_case(special_use))
                    })
        }) {
            matches.push(special_use_match_json(
                config,
                kind,
                SpecialUseSource::Mailboxes,
            ));
        }
    }
    if let Some(kind) = mailbox.special_use {
        matches.push(special_use_match_json(
            config,
            kind,
            SpecialUseSource::Rfc6154Attribute,
        ));
    }
    for kind in special_use_kinds().iter().copied() {
        if mailbox_name_matches_fallback(mailbox, kind) {
            matches.push(special_use_match_json(
                config,
                kind,
                SpecialUseSource::FallbackName,
            ));
        }
    }
    matches
}

pub(super) fn special_use_match_json(
    config: &MailConfig,
    kind: SpecialUseKind,
    source: SpecialUseSource,
) -> Value {
    json!({
        "kind": kind.as_str(),
        "source": source.as_str(),
        "attribute": kind.attribute(),
        "flag": config.special_use_flag(kind),
        "can_move_to": kind.can_move_to()
    })
}

pub(super) fn special_use_target_json(target: &SpecialUseTarget, exists: bool) -> Value {
    json!({
        "kind": target.kind.as_str(),
        "mailbox_name": target.mailbox_name,
        "source": target.source.as_str(),
        "attribute": target.attribute,
        "flag": target.flag,
        "can_move_to": target.can_move_to,
        "exists": exists
    })
}

#[cfg(test)]
pub(super) fn special_use_from_attributes(attributes: &[String]) -> Option<SpecialUseKind> {
    special_use_kinds().iter().copied().find(|kind| {
        attributes
            .iter()
            .any(|attribute| attribute.eq_ignore_ascii_case(kind.attribute()))
    })
}

pub(super) fn special_use_kinds() -> &'static [SpecialUseKind] {
    &[
        SpecialUseKind::All,
        SpecialUseKind::Archive,
        SpecialUseKind::Drafts,
        SpecialUseKind::Flagged,
        SpecialUseKind::Junk,
        SpecialUseKind::Sent,
        SpecialUseKind::Trash,
    ]
}

pub(super) fn push_unique_folder(folders: &mut Vec<String>, folder: String) {
    if !folders.iter().any(|existing| existing == &folder) {
        folders.push(folder);
    }
}

pub(super) fn mailbox_name_matches_fallback(mailbox: &MailboxInfo, kind: SpecialUseKind) -> bool {
    fallback_names(kind).iter().any(|candidate| {
        mailbox.name.eq_ignore_ascii_case(candidate)
            || mailbox_leaf_name(mailbox).eq_ignore_ascii_case(candidate)
    })
}

pub(super) fn mailbox_leaf_name(mailbox: &MailboxInfo) -> &str {
    let Some(delimiter) = mailbox.delimiter.as_deref() else {
        return &mailbox.name;
    };
    mailbox
        .name
        .rsplit(delimiter)
        .next()
        .unwrap_or(mailbox.name.as_str())
}

pub(super) fn fallback_names(kind: SpecialUseKind) -> &'static [&'static str] {
    kind.fallback_names()
}
