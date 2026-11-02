use super::*;
use crate::layout;
use ring::digest::{SHA256, digest};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ImapKey {
    pub(crate) mailbox: String,
    pub(crate) uid_validity: u64,
    pub(crate) uid: u64,
}

#[derive(Default)]
pub(crate) struct ExistingIndex {
    pub(crate) imap_keys: HashSet<ImapKey>,
    pub(crate) imap_locations: HashMap<ImapKey, String>,
    pub(crate) rfc822_cases: HashMap<String, BTreeSet<String>>,
    pub(crate) rfc822_messages: HashMap<String, BTreeSet<String>>,
    pub(crate) content_ids: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CaseSuggestion {
    pub(crate) case_uids: Vec<String>,
    pub(crate) evidence: Option<CaseRelationshipEvidence>,
}

pub(crate) struct IngestRenderContext {
    resolved_identities: Vec<ResolvedIdentity>,
}

impl IngestRenderContext {
    pub(crate) fn new(root: &Path, config: &MailConfig) -> Result<Self> {
        Ok(Self {
            resolved_identities: config.identity_profiles(root)?,
        })
    }

    pub(super) fn apply_identity(&self, message: &mut MessageFile) {
        let matched = match_message_identity_with_profiles(&self.resolved_identities, message);
        message.identity = matched.identity;
        message.identity_email = matched.identity_email;
        message.identity_match = Some(matched.identity_match);
        message.identity_candidates = matched.identity_candidates;
        message.observed_recipient_emails = matched.observed_recipient_emails;
    }
}

impl ExistingIndex {
    pub(crate) fn suggest_case(&self, raw_eml: &[u8]) -> CaseSuggestion {
        let mut case_uids = BTreeSet::new();
        for message_id in reply_header_message_ids(raw_eml) {
            if let Some(ids) = self.rfc822_cases.get(&message_id) {
                case_uids.extend(ids.iter().cloned());
            }
        }
        let case_uids = case_uids.into_iter().collect::<Vec<_>>();
        let evidence = if case_uids.is_empty() {
            None
        } else {
            Some(CaseRelationshipEvidence::ReplyHeadersMatchedLocalCase)
        };
        CaseSuggestion {
            case_uids,
            evidence,
        }
    }
}

pub(crate) fn load_existing_remote_index(root: &Path) -> Result<ExistingIndex> {
    let mut out = ExistingIndex::default();
    let cases_by_message_id = load_case_refs_by_message_id(root)?;
    let dir = root.join(".afmail/messages");
    if !dir.exists() {
        return Ok(out);
    }
    let workspace = crate::store::Workspace::at(root);
    let mut paths = fs::read_dir(dir)
        .map_err(|e| AppError::io("read messages", &e))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|e| AppError::io("read messages", &e))
        })
        .collect::<Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().and_then(|s| s.to_str()) == Some("eml"));
    paths.sort();
    let reader = workspace.message_reader()?;
    for path in paths {
        let Some(message_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let raw = fs::read(&path).map_err(|error| AppError::io("read message eml", &error))?;
        out.content_ids
            .entry(message_content_key(&raw))
            .or_insert_with(|| message_id.to_string());
        let message = reader.read(message_id)?;
        if let Some(rfc822_id) = message.rfc822_message_id.clone() {
            let key = normalize_message_id(&rfc822_id);
            if !key.is_empty() {
                out.rfc822_messages
                    .entry(key.clone())
                    .or_default()
                    .insert(message_id.to_string());
            }
            if let Some(case_uids) = cases_by_message_id.get(&message.message_id) {
                out.rfc822_cases
                    .entry(key)
                    .or_default()
                    .extend(case_uids.iter().cloned());
            }
        }
        if let Some(remote) = message.remote {
            for location in remote.locations {
                if location.missing_rfc3339.is_some() {
                    continue;
                }
                if let (Some(uid_validity), Some(uid)) = (location.uid_validity, location.uid) {
                    let key = ImapKey {
                        mailbox: location.mailbox_name,
                        uid_validity,
                        uid,
                    };
                    out.imap_keys.insert(key.clone());
                    out.imap_locations
                        .entry(key)
                        .or_insert_with(|| message.message_id.clone());
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn load_case_refs_by_message_id(
    root: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let cases_dir = root.join(layout::CASES);
    if !cases_dir.exists() {
        return Ok(out);
    }
    for group in fs::read_dir(&cases_dir).map_err(|e| AppError::io("read cases", &e))? {
        let group = group.map_err(|e| AppError::io("read cases", &e))?;
        if !group.path().is_dir() {
            continue;
        }
        for case in fs::read_dir(group.path()).map_err(|e| AppError::io("read case group", &e))? {
            let case = case.map_err(|e| AppError::io("read case group", &e))?;
            let case_path = case.path();
            if !case_path.is_dir() {
                continue;
            }
            let case_data_path = case_path.join("data").join("case.json");
            if !case_data_path.is_file() {
                continue;
            }
            let data = fs::read_to_string(&case_data_path)
                .map_err(|e| AppError::io("read case metadata", &e))?;
            let collection: crate::types::CaseMessages = serde_json::from_str(&data)
                .map_err(|e| AppError::json("parse case metadata", &e))?;
            if collection.schema_name != crate::types::CASE_SCHEMA_NAME
                || collection.schema_version != crate::types::SUGGESTIBLE_COLLECTION_SCHEMA_VERSION
            {
                return Err(AppError::new(
                    "case_metadata_invalid",
                    format!("invalid case metadata schema: {}", case_data_path.display()),
                ));
            }
            for item in collection.items {
                out.entry(item.message_id)
                    .or_default()
                    .insert(collection.collection_uid.clone());
            }
        }
    }
    Ok(out)
}

pub(crate) fn reply_header_message_ids(raw_eml: &[u8]) -> Vec<String> {
    let Some(message) = MessageParser::default().parse_headers(raw_eml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    out.extend(header_message_ids(message.in_reply_to()));
    out.extend(header_message_ids(message.references()));
    out.sort();
    out.dedup();
    out
}

pub(crate) fn header_message_ids(value: Option<TextList<'_>>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|list| list.iter())
        .flat_map(extract_message_ids)
        .collect()
}

pub(crate) fn extract_message_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        let id = normalize_message_id(&after_start[..end]);
        if !id.is_empty() {
            ids.push(id);
        }
        rest = &after_start[end + 1..];
    }
    if ids.is_empty() {
        ids.extend(
            text.split_whitespace()
                .map(normalize_message_id)
                .filter(|id| !id.is_empty()),
        );
    }
    ids.sort();
    ids.dedup();
    ids
}

pub(crate) fn normalize_message_id(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| matches!(ch, '<' | '>' | ',' | ';'))
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| AppError::io("create directory", &e))
}

pub(crate) fn message_content_key(raw_eml: &[u8]) -> String {
    digest(&SHA256, raw_eml)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn stable_message_id(raw_eml: &[u8], offset: FixedOffset) -> String {
    let parsed = MessageParser::default().parse_headers(raw_eml);
    // Prefer parser-normalized Date, accept common sender Date variants, then
    // fall back to the sender-side Received timestamp when Date is absent.
    let date_prefix = stable_message_datetime(parsed.as_ref(), raw_eml)
        .map(|date| date.with_timezone(&offset).format("%Y%m%d").to_string());
    let content_key = message_content_key(raw_eml);
    let content_id = &content_key[..32];
    match date_prefix {
        Some(date) => format!("message_{date}_{content_id}"),
        None => format!("message_{content_id}"),
    }
}

fn stable_message_datetime(
    parsed: Option<&mail_parser::Message<'_>>,
    raw_eml: &[u8],
) -> Option<DateTime<FixedOffset>> {
    parsed
        .and_then(|message| message.date())
        .map(|date| date.to_rfc3339())
        .and_then(|rfc3339| DateTime::parse_from_rfc3339(&rfc3339).ok())
        .or_else(|| {
            raw_header_values(raw_eml, "Date")
                .into_iter()
                .find_map(|value| parse_message_datetime(&value))
        })
        .or_else(|| sender_received_datetime(raw_eml))
}

fn sender_received_datetime(raw_eml: &[u8]) -> Option<DateTime<FixedOffset>> {
    raw_header_values(raw_eml, "Received")
        .into_iter()
        .rev()
        .filter_map(|value| {
            value
                .rsplit_once(';')
                .and_then(|(_, date)| parse_message_datetime(date))
        })
        .next()
}

fn parse_message_datetime(value: &str) -> Option<DateTime<FixedOffset>> {
    let trimmed = value.trim();
    let without_comment = trim_trailing_parenthetical(trimmed);
    for candidate in [trimmed, without_comment] {
        if let Ok(parsed) = DateTime::parse_from_rfc2822(candidate) {
            return Some(parsed);
        }
        for format in [
            "%a %b %d %Y %H:%M:%S GMT%z",
            "%a %b %e %Y %H:%M:%S GMT%z",
            "%a %b %d %Y %H:%M:%S %z",
            "%a %b %e %Y %H:%M:%S %z",
        ] {
            if let Ok(parsed) = DateTime::parse_from_str(candidate, format) {
                return Some(parsed);
            }
        }
    }
    None
}

fn trim_trailing_parenthetical(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.ends_with(')')
        && let Some(start) = trimmed.rfind(" (")
    {
        return trimmed[..start].trim_end();
    }
    trimmed
}

fn raw_header_values(raw_eml: &[u8], name: &str) -> Vec<String> {
    let header = raw_header_section(raw_eml);
    let text = String::from_utf8_lossy(header);
    let mut values = Vec::new();
    let mut current_matches = false;
    let mut current_value = String::new();
    let mut has_current = false;

    for line in text.lines().map(|line| line.trim_end_matches('\r')) {
        if line.starts_with(' ') || line.starts_with('\t') {
            if has_current && current_matches {
                if !current_value.is_empty() {
                    current_value.push(' ');
                }
                current_value.push_str(line.trim());
            }
            continue;
        }

        if has_current && current_matches {
            values.push(current_value.trim().to_string());
        }
        has_current = false;
        current_matches = false;
        current_value.clear();

        if let Some((header_name, value)) = line.split_once(':') {
            has_current = true;
            current_matches = header_name.eq_ignore_ascii_case(name);
            if current_matches {
                current_value.push_str(value.trim());
            }
        }
    }

    if has_current && current_matches {
        values.push(current_value.trim().to_string());
    }
    values
}

fn raw_header_section(raw_eml: &[u8]) -> &[u8] {
    if let Some(pos) = raw_eml.windows(4).position(|window| window == b"\r\n\r\n") {
        return &raw_eml[..pos];
    }
    if let Some(pos) = raw_eml.windows(2).position(|window| window == b"\n\n") {
        return &raw_eml[..pos];
    }
    raw_eml
}
