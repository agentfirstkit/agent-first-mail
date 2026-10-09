//! Local, blocking review sessions. The page is deliberately a thin renderer:
//! it receives a fixed typed Review document and can only save notes, save a
//! draft, or settle one item — locally.
//!
//! This module owns the review domain: the typed documents a surface shows,
//! the hash-checked saves and settlements it allows, and the summary it
//! reads back from the workspace when the session ends. AFUI owns the
//! delivery — the listener, the credential, expiry, the isolated window —
//! and [`host`] is the only place the two meet.
//!
//! There is no batch. Each item a review shows — a triage message, or the
//! one case/notification a case/notification review is about — settles on
//! its own the moment a person decides it, written straight to the
//! workspace. Closing the window ends the session; whatever was settled by
//! then stays settled, and whatever was not stays unsettled, available for
//! another review later. The terminal result is not carried out of the
//! session — it is read back from the workspace after the session ends,
//! because the workspace is what a crash or a `kill -9` cannot erase.

mod host;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::time::Instant;

use agent_first_data::{CliEmitter, CliEmitterError};
use agent_first_ui::{UiAttentionPolicy, UiDeliveryMode, UiFrontend, UiPage};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::host::run_session;
use super::output::progress_event;
use crate::cli::{CaseCommand, CaseDraftAction, Command, NotificationCommand, TriageAction};
use crate::config::MailConfig;
use crate::error::{AppError, Result};
use crate::render_document::{
    CollectionRenderDocument, DraftRenderDocument, NotesRenderDocument,
    RENDER_DOCUMENT_SCHEMA_VERSION, TriageRenderDocument,
};
use crate::review_frontend::review_ui_frontend;
use crate::store::{
    CaseHumanSuggestionInput, DraftChange, MessageHumanSuggestionInput,
    NotificationHumanSuggestionInput, Workspace, case_review_base_hash_from_documents,
    notification_review_base_hash_from_documents,
};
use crate::templates::{
    TemplateKey, TemplateRenderOptions, add_review_page_filters, language_template_path,
};
use crate::types::{
    CaseAgentSuggestion, CaseHumanSuggestion, CaseSuggestedAction, CollectionAgentSuggestion,
    CommandResult, HumanSuggestionOutcome, MessageAgentSuggestion, MessageHumanSuggestion,
    MessageSuggestedAction, NotificationAgentSuggestion, NotificationHumanSuggestion,
    NotificationSuggestedAction,
};
use crate::workspace_lock::{LockMode, WorkspaceLock};

pub(super) fn is_review_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Triage {
            action: TriageAction::Review { .. },
        } | Command::Case {
            action: CaseCommand::Review { .. }
                | CaseCommand::Draft {
                    action: CaseDraftAction::Review { .. },
                },
        } | Command::Notification {
            action: NotificationCommand::Review { .. },
        }
    )
}

pub(super) fn run_review_command<W: Write>(
    command: &Command,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<CommandResult> {
    let result = match command {
        Command::Triage {
            action: TriageAction::Review { message_id, mode },
        } => run_triage_review(message_id, *mode, emitter, started),
        Command::Case {
            action: CaseCommand::Review { case_ref, mode },
        } => run_case_review(case_ref, *mode, emitter, started),
        Command::Case {
            action:
                CaseCommand::Draft {
                    action:
                        CaseDraftAction::Review {
                            case_ref,
                            draft_name,
                            mode,
                        },
                },
        } => run_draft_review(case_ref, draft_name, *mode, emitter, started),
        Command::Notification {
            action:
                NotificationCommand::Review {
                    notification_ref,
                    mode,
                },
        } => run_notification_review(notification_ref, *mode, emitter, started),
        _ => Err(AppError::new(
            "review_dispatch_error",
            "run_review_command requires a review command",
        )
        .into()),
    };
    result.map(CommandResult::succeeded)
}

pub(super) enum ReviewRunError {
    App(AppError),
    Output(CliEmitterError),
}

impl From<AppError> for ReviewRunError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<CliEmitterError> for ReviewRunError {
    fn from(error: CliEmitterError) -> Self {
        Self::Output(error)
    }
}

type ReviewRunResult<T> = std::result::Result<T, ReviewRunError>;

const TRUSTED_RUNTIME_MARKER: &str = "<!-- afmail:trusted-runtime -->";

/// A review window is one sitting, not a queue browser. `afmail triage
/// review` with no message ids opens the current triage queue up to this
/// many messages, newest first, and says in the result how many were left
/// out — the same shape `triage list --limit` already reports, not a new
/// one. Someone triaging a bigger backlog picks message ids from `triage
/// list` instead of asking review to open all of it at once.
const TRIAGE_REVIEW_DEFAULT_LIMIT: usize = 25;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReviewSurfaceKind {
    #[serde(rename = "triage_review")]
    Triage,
    #[serde(rename = "case_review")]
    Case,
    #[serde(rename = "notification_review")]
    Notification,
    #[serde(rename = "draft_review")]
    Draft,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewRenderDocument {
    schema_name: String,
    schema_version: u64,
    #[serde(skip)]
    interaction_id: String,
    #[serde(skip)]
    workspace_uid: String,
    surface_kind: ReviewSurfaceKind,
    #[serde(skip)]
    subject_refs: Vec<String>,
    #[serde(skip)]
    base_hash: String,
    contents: ReviewContents,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum ReviewContents {
    Triage(TriageReviewContents),
    Case(CaseReviewContents),
    Notification(NotificationReviewContents),
    Draft(DraftReviewContents),
}

/// One triage message as a review page shows it: the typed document, the
/// message's own current review hash (what a settle route checks against,
/// whether or not the message has an `agent_suggestion`), and whether its
/// `agent_suggestion` — if it has one — is stale.
#[derive(Clone, Debug, Serialize)]
struct TriageReviewItem {
    document: TriageRenderDocument,
    message_review_base_hash: String,
    /// The suggestion round the person actually saw.  Kept server-side: a
    /// settle request cannot nominate a newer round that arrived afterward.
    #[serde(skip)]
    expected_agent_suggestion_uid: Option<String>,
    /// The message changed after its `agent_suggestion` was proposed. The
    /// item still opens and can still be settled — rejected, or decided
    /// fresh — but accepting or changing the stale suggestion is refused at
    /// settle time, the same as any other hash mismatch.
    stale: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct TriageTruncation {
    /// How many messages were in triage when this review opened.
    count: usize,
    /// How many of them this review opened.
    returned_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct TriageReviewContents {
    items: Vec<TriageReviewItem>,
    /// Present only when the caller passed no message ids and the triage
    /// queue was larger than [`TRIAGE_REVIEW_DEFAULT_LIMIT`].
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<TriageTruncation>,
}

#[derive(Clone, Debug, Serialize)]
struct CaseReviewContents {
    collection: CollectionRenderDocument,
    /// The case's own current review hash — what a settle route checks
    /// against, whether or not the case has an `agent_suggestion`. Read by
    /// the settle handler from this trusted server-side snapshot, the same
    /// way a triage settle reads [`TriageReviewItem::message_review_base_hash`];
    /// neither is a secret, so the template is free to print it too.
    case_review_base_hash: String,
    #[serde(skip)]
    expected_agent_suggestion_uid: Option<String>,
    /// The case's `agent_suggestion` — if it has one — no longer matches the
    /// case's current review hash. See [`TriageReviewItem::stale`].
    stale: bool,
    notes: EditableReviewDocument<NotesRenderDocument>,
    drafts: Vec<EditableReviewDocument<DraftRenderDocument>>,
}

#[derive(Clone, Debug, Serialize)]
struct NotificationReviewContents {
    collection: CollectionRenderDocument,
    /// See [`CaseReviewContents::case_review_base_hash`].
    notification_review_base_hash: String,
    #[serde(skip)]
    expected_agent_suggestion_uid: Option<String>,
    /// See [`CaseReviewContents::stale`].
    stale: bool,
    notes: EditableReviewDocument<NotesRenderDocument>,
}

#[derive(Clone, Debug, Serialize)]
struct DraftReviewContents {
    draft: EditableReviewDocument<DraftRenderDocument>,
}

#[derive(Clone, Debug, Serialize)]
struct EditableReviewDocument<T> {
    document: T,
    base_hash: String,
}

fn editable_review_document<T: Serialize>(document: T) -> Result<EditableReviewDocument<T>> {
    Ok(EditableReviewDocument {
        base_hash: value_hash(&document)?,
        document,
    })
}

fn build_triage_review_contents(
    workspace: &Workspace,
    message_ids: &[String],
    truncated: Option<TriageTruncation>,
) -> Result<TriageReviewContents> {
    let mut items = Vec::with_capacity(message_ids.len());
    for message_id in message_ids {
        let document = workspace.triage_template_document(message_id)?;
        let message_review_base_hash = workspace.message_review_base_hash(message_id)?;
        let suggestion = document.context.message.message.agent_suggestion.as_ref();
        let stale = suggestion.is_some_and(|suggestion| {
            suggestion.message_review_base_hash != message_review_base_hash
        });
        let expected_agent_suggestion_uid =
            suggestion.map(|suggestion| suggestion.agent_suggestion_uid.clone());
        items.push(TriageReviewItem {
            document,
            message_review_base_hash,
            expected_agent_suggestion_uid,
            stale,
        });
    }
    Ok(TriageReviewContents { items, truncated })
}

fn build_case_review_contents(
    workspace: &Workspace,
    case_ref: &str,
) -> Result<(String, CaseReviewContents)> {
    let collection = workspace.case_render_document(case_ref)?;
    let case_review_base_hash = workspace.case_review_base_hash(case_ref)?;
    let stale = matches!(
        collection.collection.agent_suggestion.as_ref(),
        Some(CollectionAgentSuggestion::Case(suggestion))
            if suggestion.case_review_base_hash != case_review_base_hash
    );
    let expected_agent_suggestion_uid = match collection.collection.agent_suggestion.as_ref() {
        Some(CollectionAgentSuggestion::Case(suggestion)) => {
            Some(suggestion.agent_suggestion_uid.clone())
        }
        _ => None,
    };
    let case_uid = collection.collection.collection_uid.clone();
    let notes = editable_review_document(workspace.case_notes_render_document(&case_uid)?)?;
    let mut drafts = Vec::new();
    if collection.collection.status.is_empty() {
        for document in workspace.draft_render_documents(&case_uid)? {
            drafts.push(editable_review_document(document)?);
        }
    }
    Ok((
        case_uid,
        CaseReviewContents {
            collection,
            case_review_base_hash,
            expected_agent_suggestion_uid,
            stale,
            notes,
            drafts,
        },
    ))
}

fn build_notification_review_contents(
    workspace: &Workspace,
    notification_ref: &str,
) -> Result<(String, NotificationReviewContents)> {
    let collection = workspace.notification_render_document(notification_ref)?;
    let notification_review_base_hash =
        workspace.notification_review_base_hash(notification_ref)?;
    let stale = matches!(
        collection.collection.agent_suggestion.as_ref(),
        Some(CollectionAgentSuggestion::Notification(suggestion))
            if suggestion.notification_review_base_hash != notification_review_base_hash
    );
    let expected_agent_suggestion_uid = match collection.collection.agent_suggestion.as_ref() {
        Some(CollectionAgentSuggestion::Notification(suggestion)) => {
            Some(suggestion.agent_suggestion_uid.clone())
        }
        _ => None,
    };
    let notification_uid = collection.collection.collection_uid.clone();
    let notes =
        editable_review_document(workspace.notification_notes_render_document(&notification_uid)?)?;
    Ok((
        notification_uid,
        NotificationReviewContents {
            collection,
            notification_review_base_hash,
            expected_agent_suggestion_uid,
            stale,
            notes,
        },
    ))
}

fn build_draft_review_contents(
    workspace: &Workspace,
    case_ref: &str,
    draft_name: &str,
) -> Result<(String, String, DraftReviewContents)> {
    let document = workspace.draft_render_document(case_ref, draft_name)?;
    let case_uid = document.draft.frontmatter.case_uid.clone();
    let draft_name = document.locator.draft_name.clone();
    Ok((
        case_uid,
        draft_name,
        DraftReviewContents {
            draft: editable_review_document(document)?,
        },
    ))
}

fn run_triage_review<W: Write>(
    message_ids: &[String],
    mode: Option<UiDeliveryMode>,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let (selected, truncated) = if message_ids.is_empty() {
        let (ids, total_count) =
            workspace.triage_review_message_ids(TRIAGE_REVIEW_DEFAULT_LIMIT)?;
        let truncated = (total_count > ids.len()).then_some(TriageTruncation {
            count: total_count,
            returned_count: ids.len(),
        });
        (ids, truncated)
    } else {
        let mut unique = HashSet::new();
        for message_id in message_ids {
            if !unique.insert(message_id.as_str()) {
                return Err(AppError::new(
                    "invalid_review_request",
                    "each message_id may appear only once",
                )
                .into());
            }
        }
        (message_ids.to_vec(), None)
    };
    if selected.is_empty() {
        return Err(
            AppError::new("nothing_to_review", "no messages are currently in triage")
                .with_hint("Pull mail, or check `afmail triage list` for the current triage queue.")
                .into(),
        );
    }
    let contents = build_triage_review_contents(&workspace, &selected, truncated)?;
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Triage,
        ReviewContents::Triage(contents),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Triage,
        model,
        mode,
        emitter,
        started,
    )
}

fn run_case_review<W: Write>(
    case_ref: &str,
    mode: Option<UiDeliveryMode>,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let (case_uid, contents) = build_case_review_contents(&workspace, case_ref)?;
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Case,
        ReviewContents::Case(contents),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Case { case_ref: case_uid },
        model,
        mode,
        emitter,
        started,
    )
}

fn run_notification_review<W: Write>(
    notification_ref: &str,
    mode: Option<UiDeliveryMode>,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let (notification_uid, contents) =
        build_notification_review_contents(&workspace, notification_ref)?;
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Notification,
        ReviewContents::Notification(contents),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Notification {
            notification_ref: notification_uid,
        },
        model,
        mode,
        emitter,
        started,
    )
}

fn run_draft_review<W: Write>(
    case_ref: &str,
    draft_name: &str,
    mode: Option<UiDeliveryMode>,
    emitter: &mut CliEmitter<W>,
    started: Instant,
) -> ReviewRunResult<Value> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    let workspace = Workspace::discover(&cwd)?;
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let (case_uid, draft_name, contents) =
        build_draft_review_contents(&workspace, case_ref, draft_name)?;
    drop(lock);
    let model = review_render_document(
        &workspace,
        ReviewSurfaceKind::Draft,
        ReviewContents::Draft(contents),
    )?;
    run_session(
        &workspace,
        ReviewSurface::Draft {
            case_ref: case_uid,
            draft_name,
        },
        model,
        mode,
        emitter,
        started,
    )
}

enum ReviewSurface {
    Triage,
    Case {
        case_ref: String,
    },
    Notification {
        notification_ref: String,
    },
    Draft {
        case_ref: String,
        draft_name: String,
    },
}

/// One notes-editor save, exactly as the review page sends it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotesEdit {
    #[serde(default)]
    base_hash: String,
    #[serde(default)]
    text: String,
}

/// One draft-editor save.
///
/// `draft_name` names which of a case review's listed drafts this is; the draft
/// surface shows exactly one draft and ignores it. `subject` and `body` are
/// optional because an absent field leaves that part of the draft alone, while
/// an empty `cc`/`bcc` clears that recipient list.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftEdit {
    #[serde(default)]
    base_hash: String,
    #[serde(default)]
    draft_name: String,
    subject: Option<String>,
    #[serde(default)]
    to: String,
    #[serde(default)]
    cc: String,
    #[serde(default)]
    bcc: String,
    body: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SavedFile {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    case_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notification_ref: Option<String>,
    draft_name: Option<String>,
    path: String,
    base_hash: String,
}

#[derive(Clone, Debug)]
enum SavedDocument {
    Notes(NotesRenderDocument),
    Draft(Box<DraftRenderDocument>),
}

#[derive(Clone, Debug)]
struct SavedEdit {
    file: SavedFile,
    document: SavedDocument,
}

impl std::ops::Deref for SavedEdit {
    type Target = SavedFile;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

#[derive(Default)]
struct ReviewSessionState {
    saved_files: Vec<SavedFile>,
}

/// One settled item, read back from the workspace rather than carried out of
/// the session — see the module doc.
#[derive(Clone, Debug, Serialize)]
struct SettledItem {
    #[serde(rename = "ref")]
    subject_ref: String,
    outcome: HumanSuggestionOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_action: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// One item this review showed that nobody settled. Silence is not a decision.
#[derive(Clone, Debug, Serialize)]
struct UnsettledItem {
    #[serde(rename = "ref")]
    subject_ref: String,
}

#[derive(Serialize)]
struct ReviewEnvelope<'a> {
    interaction_id: &'a str,
    workspace_uid: &'a str,
    surface_kind: ReviewSurfaceKind,
    subject_refs: &'a [String],
    base_hash: &'a str,
    /// How the session ended: `"closed"` (the window closed — a person
    /// closing it is the normal, unremarkable end of every review, not a
    /// decision) or `"expired"` (AFUI's remote attention clock reached the
    /// end of its warning grace). There is no `"submitted"` — settling
    /// happens per item, not once for the whole session.
    ended: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<TriageTruncation>,
    saved_count: usize,
    saved_files: &'a [SavedFile],
    settled_count: usize,
    settled: Vec<SettledItem>,
    unsettled_count: usize,
    unsettled: Vec<UnsettledItem>,
}

/// One human_suggestion's disposition-relevant fields, independent of which
/// of the three (message/case/notification) structs it came from.
struct HumanDisposition {
    outcome: HumanSuggestionOutcome,
    suggested_action: Option<Value>,
    reason: Option<String>,
}

impl HumanDisposition {
    fn from_message(human: MessageHumanSuggestion) -> Self {
        Self {
            outcome: human.outcome,
            suggested_action: human.suggested_action.map(|action| json!(action)),
            reason: human.reason,
        }
    }

    fn from_case(human: CaseHumanSuggestion) -> Self {
        Self {
            outcome: human.outcome,
            suggested_action: human.suggested_action.map(|action| json!(action)),
            reason: human.reason,
        }
    }

    fn from_notification(human: NotificationHumanSuggestion) -> Self {
        Self {
            outcome: human.outcome,
            suggested_action: human.suggested_action.map(|action| json!(action)),
            reason: human.reason,
        }
    }
}

/// Reread exactly the items this review's `subject_refs` name, bucketed into
/// settled and unsettled. This is the one place the terminal result is
/// built, and it always rereads — even when the page reported nothing,
/// because the page cannot report a settlement made moments before a `kill
/// -9`.
fn review_summary_for_subjects(
    workspace: &Workspace,
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
) -> Result<(Vec<SettledItem>, Vec<UnsettledItem>)> {
    let mut settled = Vec::new();
    let mut unsettled = Vec::new();
    match surface {
        ReviewSurface::Triage => {
            let ReviewContents::Triage(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            for item in &contents.items {
                let message_id = &item.document.context.message.message.message_id;
                let current = workspace.read_message_by_id(message_id)?;
                match current.human_suggestion {
                    Some(human) => settled.push(SettledItem {
                        subject_ref: message_id.clone(),
                        ..settled_from(HumanDisposition::from_message(human))
                    }),
                    None => unsettled.push(UnsettledItem {
                        subject_ref: message_id.clone(),
                    }),
                }
            }
        }
        ReviewSurface::Case { case_ref } => {
            let collection = workspace.case_render_document(case_ref)?;
            match collection.collection.human_suggestion {
                Some(crate::types::CollectionHumanSuggestion::Case(human)) => {
                    settled.push(SettledItem {
                        subject_ref: case_ref.clone(),
                        ..settled_from(HumanDisposition::from_case(human))
                    });
                }
                _ => unsettled.push(UnsettledItem {
                    subject_ref: case_ref.clone(),
                }),
            }
        }
        ReviewSurface::Notification { notification_ref } => {
            let collection = workspace.notification_render_document(notification_ref)?;
            match collection.collection.human_suggestion {
                Some(crate::types::CollectionHumanSuggestion::Notification(human)) => {
                    settled.push(SettledItem {
                        subject_ref: notification_ref.clone(),
                        ..settled_from(HumanDisposition::from_notification(human))
                    });
                }
                _ => unsettled.push(UnsettledItem {
                    subject_ref: notification_ref.clone(),
                }),
            }
        }
        // A draft review has no `human_suggestion` of its own to reread —
        // there is nothing to settle, only the draft text, which autosaves.
        ReviewSurface::Draft { .. } => {}
    }
    Ok((settled, unsettled))
}

fn settled_from(human: HumanDisposition) -> SettledItem {
    SettledItem {
        subject_ref: String::new(),
        outcome: human.outcome,
        suggested_action: human.suggested_action,
        reason: human.reason,
    }
}

/// Build the terminal result envelope by rereading the workspace.
///
/// `ended` names how the session stopped; `state` supplies the notes/draft
/// autosave report, which — unlike settlement — is not reread here because
/// the file itself is not self-describing about "was this written during
/// this session", so the autosave routes keep their own running list.
fn review_envelope(
    workspace: &Workspace,
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    ended: &'static str,
    state: &ReviewSessionState,
) -> Result<Value> {
    let (settled, unsettled) = review_summary_for_subjects(workspace, surface, model)?;
    let truncated = match &model.contents {
        ReviewContents::Triage(contents) => contents.truncated,
        _ => None,
    };
    serde_json::to_value(ReviewEnvelope {
        interaction_id: &model.interaction_id,
        workspace_uid: &model.workspace_uid,
        surface_kind: model.surface_kind,
        subject_refs: &model.subject_refs,
        base_hash: &model.base_hash,
        ended,
        truncated,
        saved_count: state.saved_files.len(),
        saved_files: &state.saved_files,
        settled_count: settled.len(),
        settled,
        unsettled_count: unsettled.len(),
        unsettled,
    })
    .map_err(|error| AppError::json("serialize review result", &error))
}

/// Announce the open review, and say whose UI is serving it.
///
/// `review_frontend_id` is absent when the built-in UI is serving. An Agent
/// that installed a frontend can tell from this alone whether the page it is
/// pointing a person at is the one they wrote or the one afmail ships — which
/// is the only way an untrusted frontend, the one case that is deliberately
/// silent, becomes visible without opening the window.
fn emit_review_ready<W: Write>(
    emitter: &mut CliEmitter<W>,
    model: &ReviewRenderDocument,
    mode: UiDeliveryMode,
    review_url: Option<&str>,
    attention_policy: Option<UiAttentionPolicy>,
    frontend_id: Option<&str>,
    started: Instant,
) -> std::result::Result<(), CliEmitterError> {
    let mut progress = serde_json::Map::new();
    progress.insert("phase".to_string(), json!("review_ready"));
    let mut message = match (mode, review_url) {
        (UiDeliveryMode::Link, Some(review_url)) => {
            progress.insert("review_url".to_string(), json!(review_url));
            if let Some(policy) = attention_policy {
                progress.insert(
                    "idle_timeout_s".to_string(),
                    json!(policy.idle_timeout().map(|duration| duration.as_secs())),
                );
                progress.insert(
                    "grace_period_s".to_string(),
                    json!(policy.grace_period().as_secs()),
                );
            }
            "Open this review URL from a device on the trusted local network. AFUI keeps it \
             open while attended and warns before it lapses."
                .to_string()
        }
        (UiDeliveryMode::Session, _) => "The review is published as a session. Reach it with \
                                          `afui session serve` or `afui session open`."
            .to_string(),
        // `(Link, None)` cannot happen — the caller always has a URL in hand
        // by the time it names the mode `Link` — but the match stays
        // exhaustive rather than assuming it away.
        (UiDeliveryMode::Window, _) | (UiDeliveryMode::Link, None) => {
            "The review window is open.".to_string()
        }
    };
    progress.insert("review_mode".to_string(), json!(mode.as_str()));
    progress.insert("surface_kind".to_string(), json!(model.surface_kind));
    progress.insert("subject_refs".to_string(), json!(&model.subject_refs));
    if let Some(frontend_id) = frontend_id {
        progress.insert("review_frontend_id".to_string(), json!(frontend_id));
    }
    if let ReviewContents::Triage(TriageReviewContents {
        truncated: Some(truncated),
        ..
    }) = &model.contents
    {
        message.push_str(&format!(
            " Showing {} of {} current triage messages.",
            truncated.returned_count, truncated.count
        ));
        progress.insert("triage_truncated".to_string(), json!(truncated));
    }
    progress.insert("message".to_string(), json!(message));
    let progress = Value::Object(progress);
    emitter.emit(
        agent_first_data::json_progress(progress_event(progress))
            .trace(json!({"duration_ms": started.elapsed().as_millis() as u64}))
            .build(),
    )
}

fn save_notes(
    surface: &ReviewSurface,
    edit: &NotesEdit,
) -> Result<std::result::Result<SavedEdit, String>> {
    let workspace = current_workspace()?;
    match surface {
        ReviewSurface::Case { case_ref } => {
            save_notes_in_workspace(&workspace, case_ref, &edit.text, &edit.base_hash)
        }
        ReviewSurface::Notification { notification_ref } => save_notification_notes_in_workspace(
            &workspace,
            notification_ref,
            &edit.text,
            &edit.base_hash,
        ),
        ReviewSurface::Triage | ReviewSurface::Draft { .. } => Err(AppError::new(
            "review_save_invalid",
            "this review surface does not allow notes saves",
        )),
    }
}

fn save_notes_in_workspace(
    workspace: &Workspace,
    case_ref: &str,
    text: &str,
    base_hash: &str,
) -> Result<std::result::Result<SavedEdit, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.case_notes_render_document(case_ref)?;
    let current_hash = value_hash(&current)?;
    if current_hash != base_hash {
        return Ok(Err(current_hash));
    }
    let changed = workspace.case_notes_replace(case_ref, text)?;
    let saved = workspace.case_notes_render_document(case_ref)?;
    let base_hash = value_hash(&saved)?;
    Ok(Ok(SavedEdit {
        file: SavedFile {
            kind: "notes",
            case_ref: Some(case_ref.to_string()),
            notification_ref: None,
            draft_name: None,
            path: saved_path(&changed, "notes_path")?,
            base_hash,
        },
        document: SavedDocument::Notes(saved),
    }))
}

fn save_notification_notes_in_workspace(
    workspace: &Workspace,
    notification_ref: &str,
    text: &str,
    base_hash: &str,
) -> Result<std::result::Result<SavedEdit, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.notification_notes_render_document(notification_ref)?;
    let current_hash = value_hash(&current)?;
    if current_hash != base_hash {
        return Ok(Err(current_hash));
    }
    let changed = workspace.notification_notes_replace(notification_ref, text)?;
    let saved = workspace.notification_notes_render_document(notification_ref)?;
    let base_hash = value_hash(&saved)?;
    Ok(Ok(SavedEdit {
        file: SavedFile {
            kind: "notes",
            case_ref: None,
            notification_ref: Some(notification_ref.to_string()),
            draft_name: None,
            path: saved_path(&changed, "notes_path")?,
            base_hash,
        },
        document: SavedDocument::Notes(saved),
    }))
}

fn save_draft(
    case_ref: &str,
    draft_name: &str,
    edit: &DraftEdit,
) -> Result<std::result::Result<SavedEdit, String>> {
    let workspace = current_workspace()?;
    save_draft_in_workspace(&workspace, case_ref, draft_name, edit)
}

fn save_draft_in_workspace(
    workspace: &Workspace,
    case_ref: &str,
    draft_name: &str,
    edit: &DraftEdit,
) -> Result<std::result::Result<SavedEdit, String>> {
    let _lock = acquire_review_write_lock(workspace)?;
    let current = workspace.draft_render_document(case_ref, draft_name)?;
    let current_hash = value_hash(&current)?;
    if current_hash != edit.base_hash {
        return Ok(Err(current_hash));
    }
    if edit.to.trim().is_empty() && edit.bcc.trim().is_empty() {
        return Err(AppError::new(
            "draft_invalid",
            "draft requires at least one to or bcc recipient",
        ));
    }
    let to = crate::smtp_send::normalize_recipient_list_text(&edit.to, "to")?;
    let cc = crate::smtp_send::normalize_recipient_list_text(&edit.cc, "cc")?;
    let bcc = crate::smtp_send::normalize_recipient_list_text(&edit.bcc, "bcc")?;
    let changed = workspace.change_draft(
        case_ref,
        draft_name,
        DraftChange {
            subject: edit.subject.as_deref(),
            to: &to,
            cc: &cc,
            clear_cc: edit.cc.trim().is_empty(),
            bcc: &bcc,
            clear_bcc: edit.bcc.trim().is_empty(),
            identity: None,
            body: edit.body.as_deref(),
            body_file: None,
        },
    )?;
    let saved = workspace.draft_render_document(case_ref, draft_name)?;
    let base_hash = value_hash(&saved)?;
    Ok(Ok(SavedEdit {
        file: SavedFile {
            kind: "draft",
            case_ref: Some(case_ref.to_string()),
            notification_ref: None,
            draft_name: Some(draft_name.to_string()),
            path: saved_path(&changed, "draft_path")?,
            base_hash,
        },
        document: SavedDocument::Draft(Box::new(saved)),
    }))
}

/// Advance only the typed document this session just wrote.
///
/// Rereading the entire workspace here would also adopt unrelated changes the
/// page has not seen, weakening the settle hash during the short interval
/// before the browser refreshes. Updating the successful write in the trusted
/// model keeps that interval safe: an unrelated change still makes settlement
/// stale, while a fresh human decision after this save uses the new exact
/// notes/draft document rather than the pre-save hash.
fn apply_saved_edit_to_model(
    surface: &ReviewSurface,
    model: &mut ReviewRenderDocument,
    saved: &SavedEdit,
) -> Result<()> {
    match (surface, &mut model.contents, &saved.document) {
        (
            ReviewSurface::Case { .. },
            ReviewContents::Case(contents),
            SavedDocument::Notes(document),
        ) => {
            contents.notes.document = document.clone();
            contents.notes.base_hash.clone_from(&saved.file.base_hash);
            let drafts = contents
                .drafts
                .iter()
                .map(|draft| draft.document.clone())
                .collect::<Vec<_>>();
            let current_hash = case_review_base_hash_from_documents(
                &contents.collection,
                &contents.notes.document,
                &drafts,
            )?;
            contents.stale = matches!(
                contents.collection.collection.agent_suggestion.as_ref(),
                Some(CollectionAgentSuggestion::Case(suggestion))
                    if suggestion.case_review_base_hash != current_hash
            );
            contents.case_review_base_hash = current_hash;
        }
        (
            ReviewSurface::Notification { .. },
            ReviewContents::Notification(contents),
            SavedDocument::Notes(document),
        ) => {
            contents.notes.document = document.clone();
            contents.notes.base_hash.clone_from(&saved.file.base_hash);
            let current_hash = notification_review_base_hash_from_documents(
                &contents.collection,
                &contents.notes.document,
            )?;
            contents.stale = matches!(
                contents.collection.collection.agent_suggestion.as_ref(),
                Some(CollectionAgentSuggestion::Notification(suggestion))
                    if suggestion.notification_review_base_hash != current_hash
            );
            contents.notification_review_base_hash = current_hash;
        }
        (
            ReviewSurface::Case { .. },
            ReviewContents::Case(contents),
            SavedDocument::Draft(document),
        ) => {
            let draft_name = saved.file.draft_name.as_deref().ok_or_else(|| {
                AppError::new("review_saved_document_mismatch", "saved draft has no name")
            })?;
            let draft = contents
                .drafts
                .iter_mut()
                .find(|draft| draft.document.locator.draft_name == draft_name)
                .ok_or_else(|| {
                    AppError::new(
                        "review_saved_document_mismatch",
                        "saved draft is not part of this review",
                    )
                })?;
            draft.document = document.as_ref().clone();
            draft.base_hash.clone_from(&saved.file.base_hash);
            let drafts = contents
                .drafts
                .iter()
                .map(|draft| draft.document.clone())
                .collect::<Vec<_>>();
            let current_hash = case_review_base_hash_from_documents(
                &contents.collection,
                &contents.notes.document,
                &drafts,
            )?;
            contents.stale = matches!(
                contents.collection.collection.agent_suggestion.as_ref(),
                Some(CollectionAgentSuggestion::Case(suggestion))
                    if suggestion.case_review_base_hash != current_hash
            );
            contents.case_review_base_hash = current_hash;
        }
        (
            ReviewSurface::Draft { draft_name, .. },
            ReviewContents::Draft(contents),
            SavedDocument::Draft(document),
        ) if saved.file.draft_name.as_deref() == Some(draft_name.as_str()) => {
            contents.draft.document = document.as_ref().clone();
            contents.draft.base_hash.clone_from(&saved.file.base_hash);
        }
        _ => {
            return Err(AppError::new(
                "review_saved_document_mismatch",
                "saved document does not belong to this review surface",
            ));
        }
    }
    model.base_hash = value_hash(&model.contents)?;
    Ok(())
}

fn current_workspace() -> Result<Workspace> {
    let cwd = std::env::current_dir().map_err(|error| AppError::io("current dir", &error))?;
    Workspace::discover(&cwd)
}

fn saved_path(value: &Value, field: &str) -> Result<String> {
    value[field].as_str().map(str::to_string).ok_or_else(|| {
        AppError::new(
            "review_save_invalid",
            format!("local save result omitted {field}"),
        )
    })
}

fn saved_draft_target(
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    requested: &str,
) -> Result<(String, String)> {
    match surface {
        ReviewSurface::Draft {
            case_ref,
            draft_name,
        } => Ok((case_ref.clone(), draft_name.clone())),
        ReviewSurface::Case { case_ref } => {
            let is_listed = match &model.contents {
                ReviewContents::Case(contents) => contents
                    .drafts
                    .iter()
                    .any(|draft| draft.document.locator.draft_name == requested),
                _ => false,
            };
            if !is_listed {
                return Err(AppError::new(
                    "review_draft_not_allowed",
                    "only drafts listed in this case review can be saved",
                ));
            }
            Ok((case_ref.clone(), requested.to_string()))
        }
        ReviewSurface::Triage | ReviewSurface::Notification { .. } => Err(AppError::new(
            "review_draft_not_allowed",
            "this review surface does not allow draft saves",
        )),
    }
}

fn content_hash(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}
fn value_hash<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map(|value| content_hash(&value))
        .map_err(|error| AppError::json("serialize review hash", &error))
}

/// The fields one item's settle route receives from its own inline form.
///
/// One struct serves every review surface: a field a given surface's
/// template never renders simply arrives empty, and the settle handler for
/// that surface never looks at it. `message_id` is empty (and ignored) for
/// case/notification/draft settlement, which have exactly one settleable
/// item and so need no id to disambiguate it.
type SettleFields = HashMap<String, String>;

fn field<'a>(fields: &'a SettleFields, name: &str) -> &'a str {
    fields.get(name).map(String::as_str).unwrap_or_default()
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn required_field(fields: &SettleFields, name: &str) -> Result<String> {
    non_empty(field(fields, name)).ok_or_else(|| {
        AppError::new(
            "invalid_settle_decision",
            format!("{name} is required for this decision"),
        )
    })
}

fn invalid_settle_decision(message: impl Into<String>) -> AppError {
    AppError::new("invalid_settle_decision", message)
}

/// Settle exactly the one item `fields` names, on whichever surface this
/// session is: one message from a triage review, or the one case/notification
/// a case/notification review is about. Returns the fields the page's
/// runtime needs to show the item as settled without a page reload.
///
/// The review hash each write is checked against comes from `model` — the
/// trusted snapshot built when the page was rendered — never from the form.
/// A settle form carries no `base_hash` field: there is nothing for a person
/// to tamper with, and nothing for the page to leak, because the hash was
/// never sent to the browser as data in the first place (`model.base_hash`
/// is `#[serde(skip)]`; a triage item's or a case's own review hash is a
/// server-side fact, not page state).
fn settle_from_fields(
    surface: &ReviewSurface,
    model: &ReviewRenderDocument,
    fields: &SettleFields,
) -> Result<Value> {
    let workspace = current_workspace()?;
    match surface {
        ReviewSurface::Triage => {
            let message_id = required_field(fields, "message_id")?;
            let ReviewContents::Triage(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            let item = contents
                .items
                .iter()
                .find(|item| item.document.context.message.message.message_id == message_id)
                .ok_or_else(|| {
                    AppError::new(
                        "review_settle_invalid",
                        "message_id is not part of this review",
                    )
                })?;
            let human = settle_message_in_workspace(
                &workspace,
                &message_id,
                &item.message_review_base_hash,
                item.expected_agent_suggestion_uid.as_deref(),
                fields,
            )?;
            Ok(json!({
                "settled": true,
                "message_id": message_id,
                "outcome": human.outcome,
                "suggested_action": human.suggested_action,
                "reason": human.reason,
            }))
        }
        ReviewSurface::Case { case_ref } => {
            let ReviewContents::Case(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            let human = settle_case_in_workspace(
                &workspace,
                case_ref,
                &contents.case_review_base_hash,
                contents.expected_agent_suggestion_uid.as_deref(),
                fields,
            )?;
            Ok(json!({
                "settled": true,
                "outcome": human.outcome,
                "suggested_action": human.suggested_action,
                "reason": human.reason,
            }))
        }
        ReviewSurface::Notification { notification_ref } => {
            let ReviewContents::Notification(contents) = &model.contents else {
                return Err(review_contents_mismatch());
            };
            let human = settle_notification_in_workspace(
                &workspace,
                notification_ref,
                &contents.notification_review_base_hash,
                contents.expected_agent_suggestion_uid.as_deref(),
                fields,
            )?;
            Ok(json!({
                "settled": true,
                "outcome": human.outcome,
                "suggested_action": human.suggested_action,
                "reason": human.reason,
            }))
        }
        ReviewSurface::Draft { .. } => Err(AppError::new(
            "review_settle_invalid",
            "this review surface has nothing to settle",
        )),
    }
}

fn current_message_agent_suggestion(
    workspace: &Workspace,
    message_id: &str,
) -> Result<Option<MessageAgentSuggestion>> {
    match workspace.message_agent_suggestion(message_id) {
        Ok(agent) => Ok(Some(agent)),
        Err(error) if error.error_code == "suggestion_not_found" => Ok(None),
        Err(error) => Err(error),
    }
}

fn settle_message_in_workspace(
    workspace: &Workspace,
    message_id: &str,
    review_base_hash: &str,
    expected_agent_suggestion_uid: Option<&str>,
    fields: &SettleFields,
) -> Result<MessageHumanSuggestion> {
    let _lock = acquire_review_write_lock(workspace)?;
    let agent = current_message_agent_suggestion(workspace, message_id)?;
    ensure_suggestion_round(
        expected_agent_suggestion_uid,
        agent
            .as_ref()
            .map(|suggestion| suggestion.agent_suggestion_uid.as_str()),
    )?;
    let reason = non_empty(field(fields, "reason"));
    let (outcome, based_on_agent_suggestion_uid, suggested_action) = match field(fields, "decision")
    {
        "accept" => {
            let agent = agent.as_ref().ok_or_else(|| {
                invalid_settle_decision("accept requires a current agent suggestion")
            })?;
            (
                HumanSuggestionOutcome::Accepted,
                Some(agent.agent_suggestion_uid.clone()),
                Some(agent.suggested_action.clone()),
            )
        }
        "reject" => {
            if reason.is_none() {
                return Err(invalid_settle_decision("reject requires a reason"));
            }
            (
                HumanSuggestionOutcome::Rejected,
                agent
                    .as_ref()
                    .map(|agent| agent.agent_suggestion_uid.clone()),
                None,
            )
        }
        "override_spam" => {
            let agent = agent.as_ref().ok_or_else(|| {
                invalid_settle_decision("overriding to spam requires a current agent suggestion")
            })?;
            if reason.is_none() {
                return Err(invalid_settle_decision(
                    "overriding a suggestion requires a reason",
                ));
            }
            (
                HumanSuggestionOutcome::Changed,
                Some(agent.agent_suggestion_uid.clone()),
                Some(MessageSuggestedAction::Spam),
            )
        }
        "override_trash" => {
            let agent = agent.as_ref().ok_or_else(|| {
                invalid_settle_decision("overriding to trash requires a current agent suggestion")
            })?;
            if reason.is_none() {
                return Err(invalid_settle_decision(
                    "overriding a suggestion requires a reason",
                ));
            }
            (
                HumanSuggestionOutcome::Changed,
                Some(agent.agent_suggestion_uid.clone()),
                Some(MessageSuggestedAction::Trash),
            )
        }
        "decide_case_create" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(MessageSuggestedAction::CaseCreate {
                case_name: required_field(fields, "case_name")?,
                group: required_field(fields, "group")?,
            }),
        ),
        "decide_case_add" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(MessageSuggestedAction::CaseAdd {
                case_ref: required_field(fields, "case_ref")?,
            }),
        ),
        "decide_notification_create" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(MessageSuggestedAction::NotificationCreate {
                notification_name: required_field(fields, "notification_name")?,
                summary: required_field(fields, "summary")?,
            }),
        ),
        "decide_notification_add" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(MessageSuggestedAction::NotificationAdd {
                notification_ref: required_field(fields, "notification_ref")?,
                summary: required_field(fields, "summary")?,
            }),
        ),
        "decide_spam" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(MessageSuggestedAction::Spam),
        ),
        "decide_trash" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(MessageSuggestedAction::Trash),
        ),
        _ => return Err(invalid_settle_decision("choose a decision")),
    };
    workspace.set_message_human_suggestion(&MessageHumanSuggestionInput {
        message_id: message_id.to_string(),
        based_on_agent_suggestion_uid,
        message_review_base_hash: review_base_hash.to_string(),
        outcome,
        suggested_action,
        reason,
    })
}

fn current_case_agent_suggestion(
    workspace: &Workspace,
    case_ref: &str,
) -> Result<Option<CaseAgentSuggestion>> {
    let collection = workspace.case_render_document(case_ref)?;
    match collection.collection.agent_suggestion {
        Some(CollectionAgentSuggestion::Case(agent)) => Ok(Some(agent)),
        _ => Ok(None),
    }
}

fn settle_case_in_workspace(
    workspace: &Workspace,
    case_ref: &str,
    review_base_hash: &str,
    expected_agent_suggestion_uid: Option<&str>,
    fields: &SettleFields,
) -> Result<CaseHumanSuggestion> {
    let _lock = acquire_review_write_lock(workspace)?;
    let agent = current_case_agent_suggestion(workspace, case_ref)?;
    ensure_suggestion_round(
        expected_agent_suggestion_uid,
        agent
            .as_ref()
            .map(|suggestion| suggestion.agent_suggestion_uid.as_str()),
    )?;
    let reason = non_empty(field(fields, "reason"));
    let (outcome, based_on_agent_suggestion_uid, suggested_action) = match field(fields, "decision")
    {
        "accept" => {
            let agent = agent.as_ref().ok_or_else(|| {
                invalid_settle_decision("accept requires a current agent suggestion")
            })?;
            if matches!(
                &agent.suggested_action,
                CaseSuggestedAction::CaseDelete { allow_notes: true }
            ) && field(fields, "allow_notes_confirmed") != "true"
            {
                return Err(AppError::new(
                    "invalid_case_decision",
                    "accepting case deletion with allow_notes requires explicit notes deletion confirmation",
                ));
            }
            (
                HumanSuggestionOutcome::Accepted,
                Some(agent.agent_suggestion_uid.clone()),
                Some(agent.suggested_action.clone()),
            )
        }
        "reject" => {
            if reason.is_none() {
                return Err(invalid_settle_decision("reject requires a reason"));
            }
            (
                HumanSuggestionOutcome::Rejected,
                agent
                    .as_ref()
                    .map(|agent| agent.agent_suggestion_uid.clone()),
                None,
            )
        }
        "decide_archive" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseArchive),
        ),
        "decide_unarchive" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseUnarchive {
                group: required_field(fields, "group")?,
            }),
        ),
        "decide_move" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseMove {
                group: required_field(fields, "group")?,
            }),
        ),
        "decide_rename" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseRename {
                case_name: required_field(fields, "case_name")?,
            }),
        ),
        "decide_tag" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseTag {
                tag: required_field(fields, "tag")?,
            }),
        ),
        "decide_untag" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseUntag {
                tag: required_field(fields, "tag")?,
            }),
        ),
        "decide_trash" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(CaseSuggestedAction::CaseTrash),
        ),
        "decide_delete" => {
            let allow_notes = field(fields, "decide_allow_notes") == "true";
            (
                HumanSuggestionOutcome::Decided,
                None,
                Some(CaseSuggestedAction::CaseDelete { allow_notes }),
            )
        }
        _ => return Err(invalid_settle_decision("choose a decision")),
    };
    workspace.set_case_human_suggestion(&CaseHumanSuggestionInput {
        case_ref: case_ref.to_string(),
        based_on_agent_suggestion_uid,
        case_review_base_hash: review_base_hash.to_string(),
        outcome,
        suggested_action,
        reason,
    })
}

fn current_notification_agent_suggestion(
    workspace: &Workspace,
    notification_ref: &str,
) -> Result<Option<NotificationAgentSuggestion>> {
    let collection = workspace.notification_render_document(notification_ref)?;
    match collection.collection.agent_suggestion {
        Some(CollectionAgentSuggestion::Notification(agent)) => Ok(Some(agent)),
        _ => Ok(None),
    }
}

fn settle_notification_in_workspace(
    workspace: &Workspace,
    notification_ref: &str,
    review_base_hash: &str,
    expected_agent_suggestion_uid: Option<&str>,
    fields: &SettleFields,
) -> Result<NotificationHumanSuggestion> {
    let _lock = acquire_review_write_lock(workspace)?;
    let agent = current_notification_agent_suggestion(workspace, notification_ref)?;
    ensure_suggestion_round(
        expected_agent_suggestion_uid,
        agent
            .as_ref()
            .map(|suggestion| suggestion.agent_suggestion_uid.as_str()),
    )?;
    let reason = non_empty(field(fields, "reason"));
    let (outcome, based_on_agent_suggestion_uid, suggested_action) = match field(fields, "decision")
    {
        "accept" => {
            let agent = agent.as_ref().ok_or_else(|| {
                invalid_settle_decision("accept requires a current agent suggestion")
            })?;
            if matches!(
                &agent.suggested_action,
                NotificationSuggestedAction::NotificationDelete { allow_notes: true }
            ) && field(fields, "allow_notes_confirmed") != "true"
            {
                return Err(AppError::new(
                    "invalid_notification_decision",
                    "accepting notification deletion with allow_notes requires explicit notes deletion confirmation",
                ));
            }
            (
                HumanSuggestionOutcome::Accepted,
                Some(agent.agent_suggestion_uid.clone()),
                Some(agent.suggested_action.clone()),
            )
        }
        "reject" => {
            if reason.is_none() {
                return Err(invalid_settle_decision("reject requires a reason"));
            }
            (
                HumanSuggestionOutcome::Rejected,
                agent
                    .as_ref()
                    .map(|agent| agent.agent_suggestion_uid.clone()),
                None,
            )
        }
        "decide_rename" => (
            HumanSuggestionOutcome::Decided,
            None,
            Some(NotificationSuggestedAction::NotificationRename {
                notification_name: required_field(fields, "notification_name")?,
            }),
        ),
        "decide_delete" => {
            let allow_notes = field(fields, "decide_allow_notes") == "true";
            (
                HumanSuggestionOutcome::Decided,
                None,
                Some(NotificationSuggestedAction::NotificationDelete { allow_notes }),
            )
        }
        _ => return Err(invalid_settle_decision("choose a decision")),
    };
    workspace.set_notification_human_suggestion(&NotificationHumanSuggestionInput {
        notification_ref: notification_ref.to_string(),
        based_on_agent_suggestion_uid,
        notification_review_base_hash: review_base_hash.to_string(),
        outcome,
        suggested_action,
        reason,
    })
}

fn ensure_suggestion_round(expected: Option<&str>, current: Option<&str>) -> Result<()> {
    if expected == current {
        return Ok(());
    }
    Err(AppError::new(
        "review_stale",
        "the Agent suggestion changed after this review page was rendered",
    )
    .with_hint("Reload the review and decide the suggestion round now shown."))
}

fn review_contents_mismatch() -> AppError {
    AppError::new(
        "review_document_invalid",
        "review surface and typed contents do not match",
    )
}

fn acquire_review_write_lock(workspace: &Workspace) -> Result<WorkspaceLock> {
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Exclusive)?;
    workspace.ensure_no_incomplete_transactions()?;
    Ok(lock)
}

fn review_render_document(
    workspace: &Workspace,
    surface_kind: ReviewSurfaceKind,
    contents: ReviewContents,
) -> Result<ReviewRenderDocument> {
    let identity = crate::workspace_identity::load_workspace_identity(workspace.root())?;
    let base_hash = value_hash(&contents)?;
    let subject_refs = subject_refs(&contents);
    Ok(ReviewRenderDocument {
        schema_name: "afmail_review_render_document".to_string(),
        schema_version: RENDER_DOCUMENT_SCHEMA_VERSION,
        interaction_id: interaction_id()?,
        workspace_uid: identity.workspace_uid,
        surface_kind,
        subject_refs,
        base_hash,
        contents,
    })
}

/// Reread the same review surface after its controller changed workspace
/// artifacts, while keeping this session's identity stable.
///
/// AFUI retained state carries only the controller's instruction to refresh.
/// The page is rebuilt from canonical workspace files, so a response cannot
/// smuggle a suggestion, draft, or settlement into the UI without the
/// corresponding afmail write having succeeded first.
fn refreshed_review_render_document(
    workspace: &Workspace,
    surface: &ReviewSurface,
    current: &ReviewRenderDocument,
) -> Result<ReviewRenderDocument> {
    let lock = WorkspaceLock::acquire(workspace.root(), LockMode::Shared)?;
    let contents = match surface {
        ReviewSurface::Triage => {
            let ReviewContents::Triage(current_contents) = &current.contents else {
                return Err(review_contents_mismatch());
            };
            let message_ids = current_contents
                .items
                .iter()
                .map(|item| item.document.context.message.message.message_id.clone())
                .collect::<Vec<_>>();
            ReviewContents::Triage(build_triage_review_contents(
                workspace,
                &message_ids,
                current_contents.truncated,
            )?)
        }
        ReviewSurface::Case { case_ref } => {
            let (_, contents) = build_case_review_contents(workspace, case_ref)?;
            ReviewContents::Case(contents)
        }
        ReviewSurface::Notification { notification_ref } => {
            let (_, contents) = build_notification_review_contents(workspace, notification_ref)?;
            ReviewContents::Notification(contents)
        }
        ReviewSurface::Draft {
            case_ref,
            draft_name,
        } => {
            let (_, _, contents) = build_draft_review_contents(workspace, case_ref, draft_name)?;
            ReviewContents::Draft(contents)
        }
    };
    drop(lock);
    let mut refreshed = review_render_document(workspace, current.surface_kind, contents)?;
    refreshed.interaction_id.clone_from(&current.interaction_id);
    refreshed.workspace_uid.clone_from(&current.workspace_uid);
    Ok(refreshed)
}

fn subject_refs(contents: &ReviewContents) -> Vec<String> {
    match contents {
        ReviewContents::Triage(contents) => contents
            .items
            .iter()
            .map(|item| item.document.context.message.message.message_id.clone())
            .collect(),
        ReviewContents::Case(contents) => {
            std::iter::once(contents.collection.collection.collection_uid.clone())
                .chain(
                    contents
                        .drafts
                        .iter()
                        .map(|draft| draft.document.locator.draft_name.clone()),
                )
                .collect()
        }
        ReviewContents::Notification(contents) => {
            vec![contents.collection.collection.collection_uid.clone()]
        }
        ReviewContents::Draft(contents) => vec![
            contents.draft.document.draft.frontmatter.case_uid.clone(),
            contents.draft.document.locator.draft_name.clone(),
        ],
    }
}

/// Mint the identifier the Agent correlates this review's result with.
///
/// This is a domain fact carried in the result envelope, not a credential:
/// AFUI mints the session's access credential and its CSP nonce, and neither
/// is ever this value.
fn interaction_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        AppError::new(
            "review_random_failed",
            "could not generate a review interaction id",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Render the review page, binding the trusted runtime to the session's nonce.
///
/// The nonce comes from the host because the header that authorizes it does:
/// a page rendered against any other nonce would have its own script refused.
///
/// The templates come from the person's own frontend when they installed one
/// for this surface, and from afmail's built-ins otherwise. The choice is made
/// per request rather than once per session, because trust is a fact about the
/// exact bytes on disk: editing a trusted frontend mid-review revokes it, and
/// the next page must already be the built-in one.
fn render_page(workspace: &Workspace, model: &ReviewRenderDocument, nonce: &str) -> Result<String> {
    let frontend = review_ui_frontend(workspace.root(), host::review_ui_kind(model.surface_kind))?;
    render_page_from(workspace, model, nonce, &frontend)
}

/// The hooks the review runtime cannot work without.
///
/// Not every `data-afmail-*` name the runtime knows. Two kinds are missing on
/// purpose, and finding out which was the whole job here — the list only had
/// meaning once it was checked against what a page actually renders:
///
/// - **Conditional on data.** The settle form and its controls
///   (`data-afmail-settle`, `-settle-button`, `-decision`) exist only where
///   there is an agent suggestion to settle. A triage page for a message
///   nobody proposed anything about has none, and is correct.
/// - **Optional by surface, or merely cosmetic.** Autosave and its base hash
///   belong to editors, which triage has none of; the status lines, connection
///   state and composer are all read through guards and a page missing one
///   still works.
///
/// What is left is what every review page has and no review page works
/// without:
///
/// - `data-afmail-review` — the runtime returns immediately without it, so
///   the page is inert.
/// - `data-afmail-review-content` — every form binding hangs off this node.
///   No hook, no autosave and no settling, on a page that still looks whole.
/// - `data-afmail-agent-workbench` and `data-afmail-agent-suggestion-content`
///   — a pair, and the reason this list is not just the first two. The
///   refresh path replaces the suggestion inside a surviving workbench node
///   and throws if it finds one without the other, so a frontend that kept
///   the workbench and renamed what is inside it breaks every refresh rather
///   than failing to open.
///
/// AFUI checks that they are there, on an element a browser would render.
/// What each one means stays here.
const REQUIRED_REVIEW_HOOKS: [&str; 4] = [
    "data-afmail-review",
    "data-afmail-review-content",
    "data-afmail-agent-workbench",
    "data-afmail-agent-suggestion-content",
];

/// Render one review page from a Review UI that has already been resolved.
///
/// `model` is `impl Serialize` rather than the typed document because `doctor`
/// renders through here too, with a smoke document of its own. That is the
/// point: the engine that validates a frontend has to be the engine that
/// serves it, or a template can pass one and fail the other.
pub(crate) fn render_page_from(
    workspace: &Workspace,
    model: &impl Serialize,
    nonce: &str,
    frontend: &UiFrontend,
) -> Result<String> {
    let config = MailConfig::load(workspace.root())?;
    let language = config.template_language();
    // AFUI's page kernel, then afmail's behavior inside the same scope — the
    // typed runtime, reconnection, correlation, blob transport, and unload
    // guard are shared infrastructure rather than Review-specific protocol.
    let runtime = format!(
        "<script nonce=\"{nonce}\">(function(){{\n{}\n{}\n}})();</script>",
        agent_first_ui::page_kernel_source(),
        include_str!("review_runtime.js")
    );

    let entry = TemplateKey::ReviewPage;
    let entry_path = frontend_template_path(language, entry);
    let mut page = UiPage::builder(frontend)
        .entry_at(entry.as_str(), entry_path)
        .fallback(entry.builtin_text(language));
    for key in entry.required_templates() {
        if key == entry {
            continue;
        }
        page = page.template_at(
            key.as_str(),
            frontend_template_path(language, key),
            key.builtin_text(language),
        );
    }
    for (logical_name, frontend_path) in review_partial_templates(frontend, language)? {
        page = page.override_template_at(logical_name, frontend_path);
    }

    let timezone_offset = config.resolved_timezone_offset();
    let frontend_root = frontend
        .location()
        .map(|location| location.root.to_path_buf());
    page.document_at_root()
        .requires_attributes(REQUIRED_REVIEW_HOOKS)
        .extend_environment(move |environment| {
            add_review_page_filters(
                environment,
                frontend_root.as_deref(),
                TemplateRenderOptions {
                    timezone_offset: Some(timezone_offset),
                    output_dir: None,
                },
            )
        })
        .runtime_marker(TRUSTED_RUNTIME_MARKER)
        .runtime(Some(runtime))
        .render(model)
        .map_err(review_page_error)
}

fn frontend_template_path(language: crate::config::TemplateLanguage, key: TemplateKey) -> String {
    std::path::Path::new("templates")
        .join(language_template_path(language, key))
        .to_string_lossy()
        .replace('\\', "/")
}

/// Optional, open-ended partials keep the established loader's logical names
/// while the frontend keeps locale/shared files in separate physical trees.
fn review_partial_templates(
    frontend: &UiFrontend,
    language: crate::config::TemplateLanguage,
) -> Result<Vec<(String, String)>> {
    let shared_root = "templates/shared";
    let locale_root = format!("templates/{}", language.as_str());
    let contract_names = TemplateKey::ReviewPage
        .required_templates()
        .into_iter()
        .map(TemplateKey::as_str)
        .collect::<HashSet<_>>();
    let mut templates = BTreeMap::new();

    // A shared file is reachable both as the fallback for `name` and
    // explicitly as `shared/name`. Contract templates already have their
    // built-in fallback registered, so only their explicit shared alias is
    // additional here.
    for frontend_path in frontend
        .files_under(shared_root)
        .map_err(review_page_error)?
    {
        let relative = partial_relative_path(shared_root, &frontend_path)?;
        if !contract_names.contains(relative.as_str()) && !relative.starts_with("shared/") {
            templates.insert(relative.clone(), frontend_path.clone());
        }
        templates.insert(format!("shared/{relative}"), frontend_path);
    }

    // Locale files win for an unprefixed include. A locale directory actually
    // named `shared` is intentionally skipped: the established loader reserves
    // that first component for an explicit lookup in the shared tree.
    for frontend_path in frontend
        .files_under(&locale_root)
        .map_err(review_page_error)?
    {
        let relative = partial_relative_path(&locale_root, &frontend_path)?;
        if !contract_names.contains(relative.as_str()) && !relative.starts_with("shared/") {
            templates.insert(relative, frontend_path);
        }
    }
    Ok(templates.into_iter().collect())
}

fn partial_relative_path(root: &str, frontend_path: &str) -> Result<String> {
    frontend_path
        .strip_prefix(root)
        .and_then(|name| name.strip_prefix('/'))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            AppError::new(
                "review_frontend_unreadable",
                format!("frontend partial {frontend_path} is outside {root}"),
            )
        })
}

/// One AFUI failure, in afmail's vocabulary.
///
/// The code is AFUI's classification with afmail's prefix; the recovery action
/// is AFUI's, except for the one case where afmail knows something more useful
/// — an incomplete page is a contract a template author can actually fix, and
/// saying which part beats saying "turn overrides off".
fn review_page_error(error: agent_first_ui::Error) -> AppError {
    if error.kind() == agent_first_ui::UiErrorKind::FrontendUnsafe {
        return crate::templates::unsafe_review_template_error(error);
    }
    let failure = AppError::new(
        crate::review_frontend::review_error_code(&error),
        error.to_string(),
    );
    if error.kind() == agent_first_ui::UiErrorKind::PageIncomplete {
        return failure.with_hint(
            "Keep every required element and place the trusted runtime marker once, normally immediately before </body>.",
        );
    }
    match error.hint() {
        Some(hint) => failure.with_hint(hint),
        None => failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewDraft;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Stands in for the per-session nonce AFUI mints for the live host.
    const TEST_NONCE: &str = "0123456789abcdef0123456789abcdef";
    static FRONTEND_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_workspace(name: &str) -> (PathBuf, Workspace, String, String) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("afmail-review-{name}-{stamp}"));
        let workspace = Workspace::at(&root);
        workspace.init().expect("test workspace must initialize");
        let case = workspace
            .create_case("Review", Some("support"), None, None, None)
            .expect("test case must create");
        let case_ref = case["case_uid"]
            .as_str()
            .expect("created case must have a uid")
            .to_string();
        let draft = workspace
            .create_draft(
                &case_ref,
                NewDraft {
                    to: &["customer@example.com".to_string()],
                    cc: &[],
                    bcc: &[],
                    subject: "Review draft",
                    identity: None,
                    body: Some("Initial authored text"),
                    body_file: None,
                },
            )
            .expect("test draft must create");
        let draft_name = draft["draft_name"]
            .as_str()
            .expect("created draft must have a name")
            .to_string();
        (root, workspace, case_ref, draft_name)
    }

    fn render_builtin_page(workspace: &Workspace, model: &ReviewRenderDocument) -> Result<String> {
        let frontend = UiFrontend::builtin(
            crate::review_frontend::PROVIDER_ID,
            host::review_ui_kind(model.surface_kind),
            crate::review_frontend::UI_API_VERSION,
        );
        render_page_from(workspace, model, TEST_NONCE, &frontend)
    }

    fn case_review_document(workspace: &Workspace, case_ref: &str) -> ReviewRenderDocument {
        let collection = workspace
            .case_render_document(case_ref)
            .expect("case render document");
        let expected_agent_suggestion_uid = match collection.collection.agent_suggestion.as_ref() {
            Some(CollectionAgentSuggestion::Case(suggestion)) => {
                Some(suggestion.agent_suggestion_uid.clone())
            }
            _ => None,
        };
        let case_review_base_hash = workspace
            .case_review_base_hash(case_ref)
            .expect("case review hash");
        let notes_document = workspace
            .case_notes_render_document(case_ref)
            .expect("case notes render document");
        let notes = EditableReviewDocument {
            base_hash: value_hash(&notes_document).expect("notes document hash"),
            document: notes_document,
        };
        let drafts = workspace
            .draft_render_documents(case_ref)
            .expect("draft render documents")
            .into_iter()
            .map(|document| EditableReviewDocument {
                base_hash: value_hash(&document).expect("draft document hash"),
                document,
            })
            .collect();
        review_render_document(
            workspace,
            ReviewSurfaceKind::Case,
            ReviewContents::Case(CaseReviewContents {
                collection,
                case_review_base_hash,
                expected_agent_suggestion_uid,
                stale: false,
                notes,
                drafts,
            }),
        )
        .expect("case Review document")
    }

    fn write_test_message(workspace: &Workspace, message_id: &str, subject: &str) {
        let raw = format!(
            "From: Alice <alice@example.com>\r\nTo: me@example.com\r\nSubject: {subject}\r\nDate: Mon, 20 Jul 2026 08:00:00 +0000\r\nMessage-ID: <{message_id}@example.com>\r\n\r\nBody for {subject}\r\n"
        );
        let parsed = crate::mail::parse_inbound_message(
            message_id.to_string(),
            raw.as_bytes(),
            crate::types::ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 1,
            },
        )
        .expect("test message parses");
        workspace
            .write_message_artifacts(&parsed.message)
            .expect("test message artifacts write");
    }

    #[test]
    fn review_envelope_reports_settled_and_unsettled_from_the_workspace() {
        let (root, workspace, case_ref, _) = temp_workspace("envelope-reread");
        let model = case_review_document(&workspace, &case_ref);
        let state = ReviewSessionState {
            saved_files: vec![SavedFile {
                kind: "notes",
                case_ref: Some("c1".to_string()),
                notification_ref: None,
                draft_name: None,
                path: "cases/c1/notes.md".to_string(),
                base_hash: "n1".to_string(),
            }],
        };
        // Nobody settled the case yet: the envelope reads that from the
        // workspace, not from anything the session remembers in memory.
        let surface = ReviewSurface::Case {
            case_ref: case_ref.clone(),
        };
        let unsettled_envelope = review_envelope(&workspace, &surface, &model, "closed", &state)
            .expect("envelope must serialize");
        assert_eq!(unsettled_envelope["ended"], "closed");
        assert_eq!(unsettled_envelope["saved_count"], 1);
        assert_eq!(unsettled_envelope["settled_count"], 0);
        assert_eq!(unsettled_envelope["unsettled_count"], 1);
        assert_eq!(unsettled_envelope["unsettled"][0]["ref"], case_ref);
        assert!(unsettled_envelope.get("outcome").is_none());

        // Settle it directly against the workspace, the way the settle route
        // would, then rebuild the envelope: it must now report settled.
        let base_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case review hash");
        settle_case_in_workspace(
            &workspace,
            &case_ref,
            &base_hash,
            None,
            &HashMap::from([("decision".to_string(), "decide_archive".to_string())]),
        )
        .expect("case settles");
        let settled_envelope = review_envelope(&workspace, &surface, &model, "closed", &state)
            .expect("envelope must serialize");
        assert_eq!(settled_envelope["settled_count"], 1);
        assert_eq!(settled_envelope["unsettled_count"], 0);
        assert_eq!(settled_envelope["settled"][0]["outcome"], "decided");
        assert_eq!(
            settled_envelope["settled"][0]["suggested_action"]["kind"],
            "case_archive"
        );
        assert!(
            settled_envelope["settled"][0]
                .get("based_on_agent_suggestion_uid")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_closed_window_and_a_lapsed_url_do_not_report_the_same_ended_reason() {
        let (root, workspace, case_ref, _) = temp_workspace("ended-outcomes");
        let model = case_review_document(&workspace, &case_ref);
        let surface = ReviewSurface::Case {
            case_ref: case_ref.clone(),
        };
        let state = ReviewSessionState::default();
        let closed = host::ended_envelope(
            &workspace,
            &surface,
            &model,
            &state,
            agent_first_ui::Outcome::Closed,
        )
        .expect("closed envelope must serialize");
        assert_eq!(closed["ended"], "closed");
        let expired = host::ended_envelope(
            &workspace,
            &surface,
            &model,
            &state,
            agent_first_ui::Outcome::Expired,
        )
        .expect("expired envelope must serialize");
        assert_eq!(expired["ended"], "expired");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn case_review_contains_the_exact_markdown_collection_document() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("shared-document");
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case review hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                crate::types::CaseSuggestedAction::CaseArchive,
                "This case is complete",
                None,
            )
            .expect("case suggestion");
        let markdown_document = workspace
            .case_render_document(&case_ref)
            .expect("Markdown collection render document");
        let notes_document = workspace
            .case_notes_render_document(&case_ref)
            .expect("notes render document");
        let draft_document = workspace
            .draft_render_document(&case_ref, &draft_name)
            .expect("draft render document");
        let model = case_review_document(&workspace, &case_ref);
        let render_value = serde_json::to_value(&model).expect("Review render document serializes");
        assert!(render_value.get("interaction_id").is_none());
        assert!(render_value.get("workspace_uid").is_none());
        assert!(render_value.get("subject_refs").is_none());
        assert!(render_value.get("base_hash").is_none());
        let page = render_builtin_page(&workspace, &model).expect("case Review page renders");
        assert!(page.contains("This case is complete"));
        let ReviewContents::Case(contents) = model.contents else {
            panic!("case Review must contain case contents");
        };
        assert_eq!(contents.collection, markdown_document);
        assert!(!contents.stale);
        assert_eq!(contents.notes.document, notes_document);
        assert_eq!(
            contents
                .drafts
                .iter()
                .map(|draft| &draft.document)
                .collect::<Vec<_>>(),
            vec![&draft_document]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refreshing_a_review_rereads_canonical_state_without_changing_session_identity() {
        let (root, workspace, case_ref, _) = temp_workspace("canonical-refresh");
        let current = case_review_document(&workspace, &case_ref);
        let interaction_id = current.interaction_id.clone();
        let workspace_uid = current.workspace_uid.clone();
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case review hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                crate::types::CaseSuggestedAction::CaseArchive,
                "The work is complete",
                None,
            )
            .expect("canonical suggestion writes");

        let refreshed = refreshed_review_render_document(
            &workspace,
            &ReviewSurface::Case {
                case_ref: case_ref.clone(),
            },
            &current,
        )
        .expect("Review rereads canonical state");
        assert_eq!(refreshed.interaction_id, interaction_id);
        assert_eq!(refreshed.workspace_uid, workspace_uid);
        let ReviewContents::Case(contents) = refreshed.contents else {
            panic!("case Review must contain case contents");
        };
        assert!(matches!(
            contents.collection.collection.agent_suggestion,
            Some(CollectionAgentSuggestion::Case(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_case_without_an_agent_suggestion_has_no_speculative_action_menu() {
        let (root, workspace, case_ref, _) = temp_workspace("case-status-decisions");
        let mut model = case_review_document(&workspace, &case_ref);

        let active =
            render_builtin_page(&workspace, &model).expect("active case Review page renders");
        assert!(active.contains("No current agent suggestion."), "{active}");
        assert!(active.contains("data-afmail-agent-form"), "{active}");
        assert!(!active.contains("data-afmail-settle=\"case\""), "{active}");
        assert!(!active.contains(r#"value="decide_archive""#), "{active}");
        assert!(!active.contains(r#"value="decide_move""#), "{active}");

        let ReviewContents::Case(contents) = &mut model.contents else {
            panic!("case Review must contain case contents");
        };
        contents.collection.collection.status = "archived".to_string();
        contents.drafts.clear();
        let archived =
            render_builtin_page(&workspace, &model).expect("archived case Review page renders");
        assert!(
            archived.contains("No current agent suggestion."),
            "{archived}"
        );
        assert!(
            !archived.contains("data-afmail-settle=\"case\""),
            "{archived}"
        );
        assert!(
            !archived.contains(r#"value="decide_unarchive""#),
            "{archived}"
        );
        assert!(
            !archived.contains(r#"value="decide_archive""#),
            "{archived}"
        );
        assert!(!archived.contains(r#"value="decide_move""#), "{archived}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn draft_review_contains_the_exact_draft_render_document() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("shared-draft-document");
        let document = workspace
            .draft_render_document(&case_ref, &draft_name)
            .expect("draft render document");
        let review_document = review_render_document(
            &workspace,
            ReviewSurfaceKind::Draft,
            ReviewContents::Draft(DraftReviewContents {
                draft: EditableReviewDocument {
                    base_hash: value_hash(&document).expect("draft document hash"),
                    document: document.clone(),
                },
            }),
        )
        .expect("draft Review document");
        let page =
            render_builtin_page(&workspace, &review_document).expect("draft Review page renders");
        assert!(page.contains("Review draft"));
        assert!(page.contains("Autosaves to this workspace."));
        assert!(page.contains("Quoted history and attachments remain unchanged."));
        assert!(!page.contains("<!-- afmail:compose:start -->"));
        let ReviewContents::Draft(contents) = review_document.contents else {
            panic!("draft Review must contain draft contents");
        };
        assert_eq!(contents.draft.document, document);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn triage_review_opens_a_message_with_no_agent_suggestion_and_lets_a_person_decide() {
        let (root, workspace, _, _) = temp_workspace("zero-suggestion-decides");
        write_test_message(&workspace, "message_no_suggestion", "No recommendation yet");
        let message_review_base_hash = workspace
            .message_review_base_hash("message_no_suggestion")
            .expect("message review hash");
        let document = workspace
            .triage_template_document("message_no_suggestion")
            .expect("triage document opens with no agent_suggestion");
        assert!(document.context.message.message.agent_suggestion.is_none());
        let item = TriageReviewItem {
            document,
            message_review_base_hash: message_review_base_hash.clone(),
            expected_agent_suggestion_uid: None,
            stale: false,
        };
        let model = review_render_document(
            &workspace,
            ReviewSurfaceKind::Triage,
            ReviewContents::Triage(TriageReviewContents {
                items: vec![item],
                truncated: None,
            }),
        )
        .expect("triage Review document with no suggestion");
        let page = render_builtin_page(&workspace, &model).expect("triage Review page renders");
        assert!(page.contains("No recommendation yet"));
        assert!(page.contains("data-afmail-agent-form"), "{page}");
        assert!(!page.contains("data-afmail-settle=\"message\""), "{page}");
        assert!(!page.contains(r#"value="decide_trash""#), "{page}");

        // The typed domain operation remains available to a controller after
        // it has understood the person's instruction; only the speculative
        // no-suggestion action menu is absent from the Review presentation.
        let human = settle_message_in_workspace(
            &workspace,
            "message_no_suggestion",
            &message_review_base_hash,
            None,
            &HashMap::from([("decision".to_string(), "decide_trash".to_string())]),
        )
        .expect("a person can decide a message with no agent_suggestion");
        assert_eq!(human.outcome, HumanSuggestionOutcome::Decided);
        assert!(human.based_on_agent_suggestion_uid.is_none());
        assert_eq!(human.suggested_action, Some(MessageSuggestedAction::Trash));
        let _ = std::fs::remove_dir_all(root);
    }

    /// An HTML mail is read through `body_markdown`: the page writes real
    /// anchors whose text is the sender's link text, the tracking URL lives
    /// only in `href`, and markup inside the mail never becomes markup on the
    /// page — the block tree carries text, the template writes the tags.
    #[test]
    fn triage_review_renders_an_html_body_as_links_and_paragraphs() {
        let (root, workspace, _, _) = temp_workspace("html-body-links");
        let html = concat!(
            "<html><head><style>p{color:red}</style></head><body>",
            "<div>\u{034F}\u{2800}\u{2800}</div>",
            "<p>前往<a href=\"https://c.apple.com/r?v=2&amp;a=abc\">教育商店选购</a>。</p>",
            "<p><img src=\"https://x/pixel.gif\" width=\"1\"><b>Bold</b> &lt;script&gt;alert(1)&lt;/script&gt;</p>",
            "<p><a href=\"javascript:alert(2)\">unsafe</a></p>",
            "</body></html>"
        );
        let raw = format!(
            "From: Apple <news@example.com>\r\nTo: me@example.com\r\nSubject: Offer\r\nDate: Mon, 20 Jul 2026 08:00:00 +0000\r\nMessage-ID: <html-body@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n教育商店选购\r\nhttps://c.apple.com/r?v=2&a=abc\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n--b--\r\n"
        );
        let parsed = crate::mail::parse_inbound_message(
            "message_html_body".to_string(),
            raw.as_bytes(),
            crate::types::ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 7,
            },
        )
        .expect("html message parses");
        assert!(
            parsed.message.body_markdown.as_deref().is_some_and(
                |markdown| markdown.contains("[教育商店选购](https://c.apple.com/r?v=2&a=abc)")
            ),
            "{:?}",
            parsed.message.body_markdown
        );
        workspace
            .write_message_artifacts(&parsed.message)
            .expect("html message artifacts write");
        let message_review_base_hash = workspace
            .message_review_base_hash("message_html_body")
            .expect("message review hash");
        let document = workspace
            .triage_template_document("message_html_body")
            .expect("triage document");
        let item = TriageReviewItem {
            document,
            message_review_base_hash,
            expected_agent_suggestion_uid: None,
            stale: false,
        };
        let model = review_render_document(
            &workspace,
            ReviewSurfaceKind::Triage,
            ReviewContents::Triage(TriageReviewContents {
                items: vec![item],
                truncated: None,
            }),
        )
        .expect("triage Review document");
        let page = render_builtin_page(&workspace, &model).expect("triage Review page renders");
        // A link is text that knows where it goes, never navigation: no `<a href`
        // reaches the page, the destination is a data attribute the trusted
        // runtime reads, and the registrable domain is shown beside the text,
        // marked when it is not the sender's. (MiniJinja's HTML autoescape also
        // encodes `/` in attributes.)
        assert!(
            page.contains(
                r#"<span class="md-link" role="link" tabindex="0" data-afmail-href="https:&#x2f;&#x2f;c.apple.com&#x2f;r?v=2&amp;a=abc" data-afmail-host="apple.com">教育商店选购</span>"#
            ),
            "{page}"
        );
        assert!(
            page.contains(
                r#"<span class="link-host foreign" title="Not the sender&#x27;s domain">↗ apple.com</span>"#
            ),
            "{page}"
        );
        assert!(!page.contains("<a href="), "{page}");
        assert!(page.contains("data-afmail-link-panel"), "{page}");
        assert!(page.contains("<strong>Bold</strong>"), "{page}");
        assert!(
            page.contains("&lt;script&gt;alert(1)&lt;&#x2f;script&gt;"),
            "{page}"
        );
        assert!(!page.contains("<script>alert(1)"), "{page}");
        assert!(!page.contains("javascript:"), "{page}");
        assert!(page.contains(">unsafe</p>"), "{page}");
        assert!(page.contains("data-afmail-link-panel"), "{page}");
        assert!(!page.contains("pixel.gif"), "{page}");
        // The plain-text alternative, with its bare URL line, is not what the
        // page shows once a Markdown body exists.
        assert!(!page.contains(">https:&#x2f;&#x2f;c.apple.com"), "{page}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn every_surface_refuses_a_suggestion_round_that_changed_after_render() {
        let (root, workspace, case_ref, _) = temp_workspace("suggestion-round-races");

        // Message: A -> B. The review saw A, and must not accept replacement B
        // merely because the content hash deliberately excludes suggestions.
        write_test_message(&workspace, "message_round", "Suggestion round");
        let message_hash = workspace
            .message_review_base_hash("message_round")
            .expect("message review hash");
        workspace
            .set_message_agent_suggestion(
                "message_round",
                &message_hash,
                MessageSuggestedAction::Spam,
                "round A",
                None,
            )
            .expect("message round A");
        let message_a = current_message_agent_suggestion(&workspace, "message_round")
            .expect("read message round A")
            .expect("message has round A");
        workspace
            .set_message_agent_suggestion(
                "message_round",
                &message_hash,
                MessageSuggestedAction::Trash,
                "round B",
                None,
            )
            .expect("message round B");
        let error = settle_message_in_workspace(
            &workspace,
            "message_round",
            &message_hash,
            Some(&message_a.agent_suggestion_uid),
            &HashMap::from([("decision".to_string(), "accept".to_string())]),
        )
        .expect_err("a page showing message round A cannot settle B");
        assert_eq!(error.error_code, "review_stale");

        // Case: None -> B. A fresh decision page is equally stale once a new
        // proposal arrives; it may not silently overwrite a round it never saw.
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case review hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                CaseSuggestedAction::CaseArchive,
                "new proposal",
                None,
            )
            .expect("case proposal arrives");
        let error = settle_case_in_workspace(
            &workspace,
            &case_ref,
            &case_hash,
            None,
            &HashMap::from([("decision".to_string(), "decide_archive".to_string())]),
        )
        .expect_err("a case page showing no proposal cannot settle a new one");
        assert_eq!(error.error_code, "review_stale");

        // Notification: A -> None. Clearing what the page showed also makes
        // that page stale, instead of turning accept into a generic 400.
        let notification = workspace
            .create_notification("Release", None, None, None)
            .expect("notification creates");
        let notification_uid = notification["notification_uid"]
            .as_str()
            .expect("notification uid");
        let notification_hash = workspace
            .notification_review_base_hash(notification_uid)
            .expect("notification review hash");
        workspace
            .set_notification_agent_suggestion(
                notification_uid,
                &notification_hash,
                NotificationSuggestedAction::NotificationRename {
                    notification_name: "Launch".to_string(),
                },
                "round A",
                None,
            )
            .expect("notification round A");
        let notification_a = current_notification_agent_suggestion(&workspace, notification_uid)
            .expect("read notification round A")
            .expect("notification has round A");
        workspace
            .clear_notification_suggestion(notification_uid, &notification_a.agent_suggestion_uid)
            .expect("notification round clears");
        let error = settle_notification_in_workspace(
            &workspace,
            notification_uid,
            &notification_hash,
            Some(&notification_a.agent_suggestion_uid),
            &HashMap::from([("decision".to_string(), "accept".to_string())]),
        )
        .expect_err("a page showing notification round A cannot settle None");
        assert_eq!(error.error_code, "review_stale");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn settling_two_of_three_and_closing_leaves_the_third_untouched() {
        let (root, workspace, _, _) = temp_workspace("partial-settle");
        for (id, subject) in [
            ("message_a", "First"),
            ("message_b", "Second"),
            ("message_c", "Third"),
        ] {
            write_test_message(&workspace, id, subject);
        }
        for message_id in ["message_a", "message_b"] {
            let base_hash = workspace
                .message_review_base_hash(message_id)
                .expect("message review hash");
            settle_message_in_workspace(
                &workspace,
                message_id,
                &base_hash,
                None,
                &HashMap::from([("decision".to_string(), "decide_trash".to_string())]),
            )
            .expect("message settles");
        }
        let mut items = Vec::new();
        for message_id in ["message_a", "message_b", "message_c"] {
            let document = workspace
                .triage_template_document(message_id)
                .expect("triage document");
            let message_review_base_hash = workspace
                .message_review_base_hash(message_id)
                .expect("message review hash");
            items.push(TriageReviewItem {
                document,
                message_review_base_hash,
                expected_agent_suggestion_uid: None,
                stale: false,
            });
        }
        let model = review_render_document(
            &workspace,
            ReviewSurfaceKind::Triage,
            ReviewContents::Triage(TriageReviewContents {
                items,
                truncated: None,
            }),
        )
        .expect("triage Review document");
        let surface = ReviewSurface::Triage;
        let envelope = review_envelope(
            &workspace,
            &surface,
            &model,
            "closed",
            &ReviewSessionState::default(),
        )
        .expect("envelope must serialize");
        assert_eq!(envelope["settled_count"], 2);
        assert_eq!(envelope["unsettled_count"], 1);
        assert_eq!(envelope["unsettled"][0]["ref"], "message_c");
        let settled_refs: Vec<&str> = envelope["settled"]
            .as_array()
            .expect("settled array")
            .iter()
            .map(|item| item["ref"].as_str().expect("ref is a string"))
            .collect();
        assert!(settled_refs.contains(&"message_a"));
        assert!(settled_refs.contains(&"message_b"));

        // message_c has no metadata.json written by a human decision: kill -9
        // the process here and it is exactly as absent on disk as it is now.
        let untouched = workspace
            .read_message_by_id("message_c")
            .expect("message_c is still readable");
        assert!(untouched.human_suggestion.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resettling_an_item_replaces_its_record_rather_than_appending() {
        let (root, workspace, _, _) = temp_workspace("resettle-replaces");
        write_test_message(&workspace, "message_resettle", "Changeable");
        let first_hash = workspace
            .message_review_base_hash("message_resettle")
            .expect("message review hash");
        let first = settle_message_in_workspace(
            &workspace,
            "message_resettle",
            &first_hash,
            None,
            &HashMap::from([("decision".to_string(), "decide_trash".to_string())]),
        )
        .expect("first settle");
        assert_eq!(first.suggested_action, Some(MessageSuggestedAction::Trash));

        let second_hash = workspace
            .message_review_base_hash("message_resettle")
            .expect("message review hash unchanged by a human_suggestion write");
        let second = settle_message_in_workspace(
            &workspace,
            "message_resettle",
            &second_hash,
            None,
            &HashMap::from([("decision".to_string(), "decide_spam".to_string())]),
        )
        .expect("resettle replaces the prior decision");
        assert_ne!(second.human_suggestion_uid, first.human_suggestion_uid);
        assert_eq!(second.suggested_action, Some(MessageSuggestedAction::Spam));
        let current = workspace
            .read_message_by_id("message_resettle")
            .expect("message readable");
        assert_eq!(
            current
                .human_suggestion
                .map(|human| human.human_suggestion_uid),
            Some(second.human_suggestion_uid)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_followup_failure_reports_that_the_settlement_was_already_persisted() {
        let (root, workspace, _, _) = temp_workspace("settlement-followup-failure");
        write_test_message(&workspace, "message_committed", "Committed decision");
        let base_hash = workspace
            .message_review_base_hash("message_committed")
            .expect("message review hash");

        // Make the audit target unopenable as a file after the canonical and
        // projection writes succeed.
        let audit_path = root.join(".afmail/logs/events.jsonl");
        if audit_path.is_file() {
            std::fs::remove_file(&audit_path).expect("remove test audit log");
        }
        std::fs::create_dir_all(&audit_path).expect("directory blocks audit file open");

        let error = settle_message_in_workspace(
            &workspace,
            "message_committed",
            &base_hash,
            None,
            &HashMap::from([("decision".to_string(), "decide_trash".to_string())]),
        )
        .expect_err("audit follow-up must be reported");
        assert_eq!(error.error_code, "settlement_followup_failed");
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|details| details["settlement_persisted"].as_bool()),
            Some(true)
        );
        assert!(
            workspace
                .read_message_by_id("message_committed")
                .expect("canonical message remains readable")
                .human_suggestion
                .is_some(),
            "the response contract must reflect the canonical write"
        );
        assert!(host::settlement_was_persisted(&error));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_stale_case_suggestion_is_marked_in_place_rather_than_blocking_the_review() {
        let (root, workspace, case_ref, _) = temp_workspace("stale-in-place");
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                crate::types::CaseSuggestedAction::CaseArchive,
                "Looks complete",
                None,
            )
            .expect("case suggestion sets");
        // Rename the case without replacing its suggestion: the case's notes
        // path, and so its review hash, moves out from under the standing
        // suggestion — the same kind of drift `run_case_review` must
        // tolerate rather than refuse to open.
        workspace
            .rename_case(
                &case_ref,
                "Renamed while a suggestion is pending",
                Some("drift the review hash"),
            )
            .expect("case rename");
        let collection = workspace
            .case_render_document(&case_ref)
            .expect("case document still opens");
        let current_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("current case hash");
        let stale = matches!(
            collection.collection.agent_suggestion.as_ref(),
            Some(CollectionAgentSuggestion::Case(suggestion))
                if suggestion.case_review_base_hash != current_hash
        );
        assert!(
            stale,
            "a case whose review hash moved under a standing suggestion must be detected as stale"
        );

        // The review still opens and shows it — no `review_stale` error at
        // open time — and a person can still settle it (reject, or decide
        // fresh); only accepting the drifted suggestion fails, at settle time.
        let notes = workspace
            .case_notes_render_document(&case_ref)
            .expect("notes document");
        let model = review_render_document(
            &workspace,
            ReviewSurfaceKind::Case,
            ReviewContents::Case(CaseReviewContents {
                collection,
                case_review_base_hash: current_hash,
                expected_agent_suggestion_uid: current_case_agent_suggestion(&workspace, &case_ref)
                    .expect("current case suggestion")
                    .map(|suggestion| suggestion.agent_suggestion_uid),
                stale,
                notes: EditableReviewDocument {
                    base_hash: value_hash(&notes).expect("notes hash"),
                    document: notes,
                },
                drafts: Vec::new(),
            }),
        )
        .expect("case Review document opens even though the suggestion is stale");
        let page =
            render_builtin_page(&workspace, &model).expect("stale case Review page still renders");
        assert!(page.contains("Renamed while a suggestion is pending"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn window_review_ready_progress_is_emitted_without_exposing_the_session_url() {
        let (root, workspace, case_ref, _) = temp_workspace("window-ready");
        let model = case_review_document(&workspace, &case_ref);
        let mut output = Vec::new();
        {
            let mut emitter = CliEmitter::new(&mut output, agent_first_data::OutputFormat::Json)
                .with_strict_protocol();
            emit_review_ready(
                &mut emitter,
                &model,
                UiDeliveryMode::Window,
                None,
                None,
                None,
                Instant::now(),
            )
            .expect("window ready progress emits");
        }
        let event: Value = serde_json::from_slice(&output).expect("progress is JSON");
        assert!(agent_first_data::validate_protocol_event(&event, false).is_ok());
        assert_eq!(event["kind"], "progress");
        assert_eq!(event["progress"]["phase"], "review_ready");
        assert_eq!(event["progress"]["review_mode"], "window");
        assert_eq!(event["progress"]["surface_kind"], "case_review");
        assert!(event["progress"].get("review_url").is_none());
        // Nothing to say when the built-in UI is serving, so nothing is said.
        assert!(event["progress"].get("review_frontend_id").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A surface's `ui_kind` is the override key, and the list of kinds a
    /// person can install a frontend for is written down separately from the
    /// mapping. Neither is allowed to grow a name the other does not have.
    #[test]
    fn every_review_surface_has_exactly_one_installable_ui_kind() {
        let mapped = [
            ReviewSurfaceKind::Triage,
            ReviewSurfaceKind::Case,
            ReviewSurfaceKind::Notification,
            ReviewSurfaceKind::Draft,
        ]
        .map(host::review_ui_kind);
        let mut mapped = mapped.to_vec();
        mapped.sort_unstable();
        let mut declared = crate::review_frontend::UI_KINDS.to_vec();
        declared.sort_unstable();
        assert_eq!(mapped, declared);
    }

    #[test]
    fn notes_save_writes_continuations_and_stale_save_does_not_overwrite() {
        let (root, workspace, case_ref, _) = temp_workspace("notes-save");
        let first_hash = value_hash(
            &workspace
                .case_notes_render_document(&case_ref)
                .expect("notes render document"),
        )
        .expect("notes document hash");
        let first = save_notes_in_workspace(&workspace, &case_ref, "First note", &first_hash)
            .expect("notes save")
            .expect("notes must not be stale");
        assert_eq!(
            std::fs::read_to_string(root.join(&first.path)).expect("read notes"),
            "First note\n"
        );
        let second =
            save_notes_in_workspace(&workspace, &case_ref, "Second note", &first.base_hash)
                .expect("second notes save")
                .expect("updated hash must save");
        let before_stale = std::fs::read_to_string(root.join(&second.path)).expect("read notes");
        assert!(
            save_notes_in_workspace(&workspace, &case_ref, "Stale", &first_hash)
                .expect("stale notes result")
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.join(&second.path)).expect("read notes"),
            before_stale
        );
    }

    #[test]
    fn a_successful_notes_save_advances_the_open_review_for_a_fresh_decision() {
        let (root, workspace, case_ref, _) = temp_workspace("notes-save-review-hash");
        let surface = ReviewSurface::Case {
            case_ref: case_ref.clone(),
        };
        let mut model = case_review_document(&workspace, &case_ref);
        let ReviewContents::Case(contents) = &model.contents else {
            panic!("case review contents");
        };
        let prior_hash = contents.case_review_base_hash.clone();
        let saved = save_notes_in_workspace(
            &workspace,
            &case_ref,
            "Decision context recorded by the person",
            &contents.notes.base_hash,
        )
        .expect("notes save")
        .expect("current notes save");

        apply_saved_edit_to_model(&surface, &mut model, &saved)
            .expect("advance the trusted review snapshot");
        let ReviewContents::Case(contents) = &model.contents else {
            panic!("case review contents");
        };
        assert_ne!(contents.case_review_base_hash, prior_hash);
        assert_eq!(
            contents.case_review_base_hash,
            workspace
                .case_review_base_hash(&case_ref)
                .expect("canonical review hash"),
            "the session must advance by exactly its own successful write"
        );
        settle_case_in_workspace(
            &workspace,
            &case_ref,
            &contents.case_review_base_hash,
            contents.expected_agent_suggestion_uid.as_deref(),
            &HashMap::from([("decision".to_string(), "decide_archive".to_string())]),
        )
        .expect("a fresh decision after the person's own save must not be self-stale");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn advancing_after_a_save_does_not_adopt_an_unseen_external_edit() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("notes-save-external-edit");
        let surface = ReviewSurface::Case {
            case_ref: case_ref.clone(),
        };
        let mut model = case_review_document(&workspace, &case_ref);
        let ReviewContents::Case(contents) = &model.contents else {
            panic!("case review contents");
        };
        let notes_hash = contents.notes.base_hash.clone();

        workspace
            .change_draft(
                &case_ref,
                &draft_name,
                DraftChange {
                    subject: Some("Changed outside the open page"),
                    to: &["customer@example.com".to_string()],
                    cc: &[],
                    clear_cc: true,
                    bcc: &[],
                    clear_bcc: true,
                    identity: None,
                    body: Some("Unseen external body"),
                    body_file: None,
                },
            )
            .expect("external draft edit");
        let saved = save_notes_in_workspace(
            &workspace,
            &case_ref,
            "The notes editor itself is still current",
            &notes_hash,
        )
        .expect("notes save")
        .expect("notes target remained current");
        apply_saved_edit_to_model(&surface, &mut model, &saved)
            .expect("advance only the notes document");
        let ReviewContents::Case(contents) = &model.contents else {
            panic!("case review contents");
        };
        assert_ne!(
            contents.case_review_base_hash,
            workspace
                .case_review_base_hash(&case_ref)
                .expect("canonical review hash"),
            "an unrelated edit the page never saw must remain a stale boundary"
        );
        let error = settle_case_in_workspace(
            &workspace,
            &case_ref,
            &contents.case_review_base_hash,
            contents.expected_agent_suggestion_uid.as_deref(),
            &HashMap::from([("decision".to_string(), "decide_archive".to_string())]),
        )
        .expect_err("unseen external edit must still stale settlement");
        assert_eq!(error.error_code, "review_stale");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn notes_save_becomes_stale_when_the_exact_document_path_changes() {
        let (root, workspace, case_ref, _) = temp_workspace("notes-path-stale");
        let initial = workspace
            .case_notes_render_document(&case_ref)
            .expect("initial notes render document");
        let initial_hash = value_hash(&initial).expect("initial notes document hash");

        workspace
            .rename_case(
                &case_ref,
                "Renamed review",
                Some("exercise review concurrency"),
            )
            .expect("case rename");
        let moved = workspace
            .case_notes_render_document(&case_ref)
            .expect("moved notes render document");
        assert_eq!(moved.text, initial.text);
        assert_ne!(moved.notes_path, initial.notes_path);

        let stale = save_notes_in_workspace(&workspace, &case_ref, "must not save", &initial_hash)
            .expect("stale notes result");
        assert_eq!(
            stale.expect_err("moved notes document must be stale"),
            value_hash(&moved).expect("moved notes document hash")
        );
        assert_eq!(
            workspace
                .case_notes_render_document(&case_ref)
                .expect("unchanged notes render document")
                .text,
            moved.text
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn draft_save_preserves_template_sections_and_stale_save_does_not_overwrite() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("draft-save");
        let initial = workspace
            .draft_render_document(&case_ref, &draft_name)
            .expect("draft render document");
        let initial_hash = value_hash(&initial).expect("draft hash");
        let initial_text =
            std::fs::read_to_string(root.join(&initial.locator.draft_path)).expect("read draft");
        let mut edit = DraftEdit {
            base_hash: initial_hash.clone(),
            draft_name: draft_name.clone(),
            subject: Some("Updated subject".to_string()),
            to: "\"Doe, Jane\" <jane@example.com>, customer@example.com".to_string(),
            cc: String::new(),
            bcc: "Hidden Reviewer <hidden@example.com>".to_string(),
            body: Some("Updated authored text".to_string()),
        };
        let saved = save_draft_in_workspace(&workspace, &case_ref, &draft_name, &edit)
            .expect("draft save")
            .expect("draft must not be stale");
        let updated_text =
            std::fs::read_to_string(root.join(&saved.path)).expect("read updated draft");
        assert!(updated_text.contains("Updated authored text"));
        assert!(updated_text.contains("<!-- afmail:compose:start -->"));
        assert!(updated_text.contains("<!-- afmail:compose:end -->"));
        assert!(initial_text.contains("<!-- afmail:compose:start -->"));
        let shown = workspace
            .show_draft(&case_ref, &draft_name)
            .expect("show parsed recipients");
        assert_eq!(
            shown["draft"]["frontmatter"]["to"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            shown["draft"]["frontmatter"]["to"][0],
            "\"Doe, Jane\" <jane@example.com>"
        );
        assert_eq!(
            shown["draft"]["frontmatter"]["bcc"][0],
            "Hidden Reviewer <hidden@example.com>"
        );

        edit.base_hash = saved.base_hash.clone();
        edit.to = String::new();
        edit.bcc = String::new();
        let error = save_draft_in_workspace(&workspace, &case_ref, &draft_name, &edit)
            .expect_err("empty recipient set must be rejected");
        assert_eq!(error.error_code, "draft_invalid");
        let unchanged_draft = workspace
            .show_draft(&case_ref, &draft_name)
            .expect("show unchanged recipients");
        assert_eq!(
            unchanged_draft["draft"]["frontmatter"]["to"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        let updated_text =
            std::fs::read_to_string(root.join(&saved.path)).expect("read unchanged draft");
        edit.base_hash = initial_hash;
        edit.to = "customer@example.com".to_string();
        edit.body = Some("Stale authored text".to_string());
        assert!(
            save_draft_in_workspace(&workspace, &case_ref, &draft_name, &edit)
                .expect("stale draft result")
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.join(&saved.path)).expect("read draft"),
            updated_text
        );
    }

    #[test]
    fn review_writes_refuse_an_incomplete_workspace_transaction() {
        let (root, workspace, case_ref, _) = temp_workspace("transaction-guard");
        let transaction_path = root.join(".afmail/transactions/transaction_review_test.json");
        let transaction = json!({
            "schema_name": "local_transaction",
            "schema_version": 1,
            "transaction_id": "transaction_review_test",
            "kind": "review_test",
            "created_rfc3339": "2026-07-21T00:00:00Z",
            "paths": ["cases/test/notes.md"]
        });
        std::fs::write(
            transaction_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&transaction).expect("serialize transaction")
            ),
        )
        .expect("write incomplete transaction");
        let base_hash = value_hash(
            &workspace
                .case_notes_render_document(&case_ref)
                .expect("notes render document"),
        )
        .expect("notes document hash");
        let error = save_notes_in_workspace(&workspace, &case_ref, "blocked", &base_hash)
            .expect_err("review write must be blocked");
        assert_eq!(error.error_code, "transaction_incomplete");
    }

    #[test]
    fn destructive_case_suggestion_requires_explicit_notes_confirmation() {
        let (root, workspace, case_ref, _) = temp_workspace("destructive-confirmation");
        let case_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case hash");
        workspace
            .set_case_agent_suggestion(
                &case_ref,
                &case_hash,
                crate::types::CaseSuggestedAction::CaseDelete { allow_notes: true },
                "Remove the obsolete collection",
                None,
            )
            .expect("case suggestion");
        let model = case_review_document(&workspace, &case_ref);
        let base_hash = workspace
            .case_review_base_hash(&case_ref)
            .expect("case hash");
        let error = settle_case_in_workspace(
            &workspace,
            &case_ref,
            &base_hash,
            current_case_agent_suggestion(&workspace, &case_ref)
                .expect("current case suggestion")
                .as_ref()
                .map(|suggestion| suggestion.agent_suggestion_uid.as_str()),
            &HashMap::from([("decision".to_string(), "accept".to_string())]),
        )
        .expect_err("missing destructive confirmation must fail");
        assert_eq!(error.error_code, "invalid_case_decision");
        assert!(
            render_builtin_page(&workspace, &model)
                .expect("Review template renders")
                .contains("permanently delete")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A frontend the person installed, already resolved and trusted.
    ///
    /// Discovery, the trust gate and safe mode belong to AFUI and are driven
    /// end to end in `tests/review_frontend.rs`; these tests are about what
    /// afmail does with a frontend once it has one.
    /// Lay out a frontend and resolve it — with every file it is meant to have
    /// already written.
    ///
    /// AFUI reads a frontend's trusted files once, when it decides to trust
    /// them, and serves that snapshot. A test that resolved first and wrote its
    /// template afterwards used to work because reads went back to disk per
    /// request; now that file is simply not part of the frontend, and the
    /// built-in page renders instead — which quietly turns a test of the safety
    /// rules into a test of the built-in page. Files go in first.
    fn frontend_at(root: &std::path::Path, files: &[(&str, String)]) -> UiFrontend {
        let frontend_root = root.join("frontends/afmail/case_review");
        std::fs::create_dir_all(frontend_root.join("templates/en-US/review"))
            .expect("frontend Review directory");
        std::fs::write(
            frontend_root.join("frontend.json"),
            format!(
                r#"{{"frontend_id":"test_review_ui","ui_api_version":"{}"}}"#,
                crate::review_frontend::UI_API_VERSION
            ),
        )
        .expect("frontend manifest");
        for (relative, contents) in files {
            let path = frontend_root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("frontend file directory");
            }
            std::fs::write(&path, contents).expect("frontend file");
        }
        let _guard = FRONTEND_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // SAFETY: serialised by `FRONTEND_ENV_LOCK`; resolution captures the
        // concrete location before the variable is removed.
        unsafe { std::env::set_var(agent_first_ui::CONFIG_DIR_ENV, root) };
        let frontend = UiFrontend::resolve_with_safe_mode(
            std::path::Path::new("/nonexistent"),
            "afmail",
            "case_review",
            crate::review_frontend::UI_API_VERSION,
            false,
        )
        .expect("test frontend resolves");
        unsafe { std::env::remove_var(agent_first_ui::CONFIG_DIR_ENV) };
        frontend
    }

    /// Where a Review page lives inside a frontend, relative to its root.
    const PAGE: &str = "templates/en-US/review/page.html.j2";

    /// The hooks [`REQUIRED_REVIEW_HOOKS`] names, as a page fragment.
    ///
    /// Spelled out in these fixtures rather than injected by the renderer,
    /// because a test page has to be the same kind of page a person writing a
    /// frontend would have to write.
    const HOOKS: &str = "<main data-afmail-review><div data-afmail-review-content>\
<section data-afmail-agent-workbench><div data-afmail-agent-suggestion-content></div>\
</section></div></main>";

    #[test]
    fn custom_review_page_must_place_exactly_one_trusted_runtime_marker() {
        let (root, workspace, case_ref, _) = temp_workspace("trusted-runtime-marker");
        let model = case_review_document(&workspace, &case_ref);
        let without_marker = frontend_at(
            &root.join("no-marker"),
            &[(
                PAGE,
                format!("<!doctype html><html><body>{HOOKS}</body></html>"),
            )],
        );
        let error = render_page_from(&workspace, &model, TEST_NONCE, &without_marker)
            .expect_err("Review page without a trusted runtime marker must fail");
        assert_eq!(error.error_code, "review_template_invalid");

        let with_marker = frontend_at(
            &root.join("marker"),
            &[(
                PAGE,
                format!(
                    "<!doctype html><html><body>{HOOKS}<!-- afmail:trusted-runtime --></body></html>"
                ),
            )],
        );
        let page = render_page_from(&workspace, &model, TEST_NONCE, &with_marker)
            .expect("trusted runtime marker renders");
        // The page's script runs only because the session's own policy names
        // this nonce; a page rendered against any other value is inert.
        assert!(page.contains(&format!("<script nonce=\"{TEST_NONCE}\"")));
        assert!(!page.contains(TRUSTED_RUNTIME_MARKER));
        assert!(
            page.find("<script nonce=").unwrap_or_default()
                < page.find("</body>").unwrap_or_default()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A frontend supplies presentation. It does not get its own escaping
    /// rules, its own JavaScript, or a way to make the page say a save
    /// happened — everything the built-in UI is held to still holds.
    #[test]
    fn a_frontend_is_held_to_the_same_review_contract_as_the_builtin_ui() {
        let (root, workspace, case_ref, _) = temp_workspace("frontend-contract");
        let model = case_review_document(&workspace, &case_ref);
        let with_script = frontend_at(
            &root.join("script"),
            &[(
                PAGE,
                format!(
                    "<html><body><script>fetch('/submit')</script>{HOOKS}\
                     <!-- afmail:trusted-runtime --></body></html>"
                ),
            )],
        );
        let error = render_page_from(&workspace, &model, TEST_NONCE, &with_script)
            .expect_err("a frontend may not ship JavaScript");
        assert_eq!(error.error_code, "review_template_unsafe");

        // Values a frontend prints are escaped by the same rules, so a case
        // subject cannot become markup just because the UI is custom.
        let printing = frontend_at(
            &root.join("printing"),
            &[(
                PAGE,
                format!(
                    "<html><body><main>{{{{ contents.collection.collection.collection_name }}}}</main>\
                     {HOOKS}<!-- afmail:trusted-runtime --></body></html>"
                ),
            )],
        );
        let page = render_page_from(&workspace, &model, TEST_NONCE, &printing)
            .expect("frontend Review page renders");
        assert!(page.contains("<main>Review</main>"), "{page}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A frontend that supplies only `review/page.html.j2` still gets the
    /// rest of the *active* language pack — not a hardcoded `en-US` default,
    /// and not a blank composition. This is what makes the per-file overlay
    /// worth having: replacing the entry does not mean re-authoring
    /// `i18n/catalog.j2`, the shared layouts, or the message/draft/notes
    /// editors in all nine languages just to keep the page working.
    #[test]
    fn overriding_only_the_entry_still_serves_the_rest_of_the_active_language_pack() {
        let (root, workspace, case_ref, _) = temp_workspace("frontend-keeps-localization");
        let mut config = MailConfig::load(workspace.root()).expect("default config loads");
        config.workspace.language_bcp47 = Some("zh-CN".to_string());
        config.write(workspace.root()).expect("zh-CN config writes");

        let model = case_review_document(&workspace, &case_ref);
        // Composed exactly like afmail's own zh-CN entry — import the
        // catalog, include the shared layout — plus one marker so the test
        // can tell the override rendered rather than the built-in page
        // having served the whole thing.
        let frontend = frontend_at(
            &root.join("frontend"),
            &[(
                "templates/zh-CN/review/page.html.j2",
                "<!-- overridden-entry -->\
                 {% import \"i18n/catalog.j2\" as i18n %}\
                 {% include \"review/page-layout.html.j2\" %}"
                    .to_string(),
            )],
        );

        let page = render_page_from(&workspace, &model, TEST_NONCE, &frontend)
            .expect("overridden entry still renders");
        assert!(page.contains("<!-- overridden-entry -->"), "{page}");
        // Nothing but page.html.j2 was supplied by the frontend: the shared
        // layout, the message/draft/notes editors, and the catalog itself
        // all had to fall back — and they must fall back to zh-CN, not
        // en-US and not an empty string.
        assert!(
            page.contains("Agent 正在回复"),
            "an entry-only override must still pull the active language's own catalog text: {page}"
        );
        assert!(!page.contains("Agent is replying"), "{page}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arbitrary_review_partials_keep_locale_precedence_and_explicit_shared_lookup() {
        let (root, workspace, case_ref, _) = temp_workspace("frontend-partial-loader");
        let mut config = MailConfig::load(workspace.root()).expect("default config loads");
        config.workspace.language_bcp47 = Some("zh-CN".to_string());
        config.write(workspace.root()).expect("zh-CN config writes");

        let model = case_review_document(&workspace, &case_ref);
        let frontend = frontend_at(
            &root.join("frontend"),
            &[
                (
                    "templates/shared/custom/banner.html.j2",
                    "<p>shared banner</p>".to_string(),
                ),
                (
                    "templates/zh-CN/custom/banner.html.j2",
                    "<p>localized banner</p>".to_string(),
                ),
                (
                    "templates/shared/custom/footer.txt.j2",
                    "<footer>explicit shared footer</footer>".to_string(),
                ),
                (
                    "templates/zh-CN/review/page.html.j2",
                    format!(
                        "{{% include \"custom/banner.html.j2\" %}}\
                         {{% include \"shared/custom/footer.txt.j2\" %}}\
                         {HOOKS}<!-- afmail:trusted-runtime -->"
                    ),
                ),
            ],
        );

        let page = render_page_from(&workspace, &model, TEST_NONCE, &frontend)
            .expect("arbitrary Review partials render");
        assert!(page.contains("localized banner"), "{page}");
        assert!(!page.contains("shared banner"), "{page}");
        assert!(page.contains("explicit shared footer"), "{page}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// The built-in case page: message bodies and subjects are escaped, the
    /// per-item settle markup is present and unkeyed by any page-wide
    /// `action=` field, and the runtime script it ships is the one that
    /// drives it.
    #[test]
    fn case_page_is_server_rendered_from_typed_documents_and_escapes_untrusted_message_content() {
        let (root, workspace, case_ref, draft_name) = temp_workspace("typed-page");
        let mut model = case_review_document(&workspace, &case_ref);
        let raw = b"From: Alice <alice@example.com>\r\nTo: me@example.com\r\nSubject: Readable <img src=x onerror=alert(1)>\r\nDate: Mon, 20 Jul 2026 08:00:00 +0000\r\nMessage-ID: <review@example.com>\r\n\r\nReadable human message <script>alert(1)</script>\r\n";
        let parsed = crate::mail::parse_inbound_message(
            "message_review_test".to_string(),
            raw,
            crate::types::ImapRef {
                mailbox_name: "INBOX".to_string(),
                uid_validity: 1,
                uid: 1,
            },
        )
        .expect("test message parses");
        std::fs::write(
            workspace
                .root()
                .join(".afmail/messages/message_review_test.eml"),
            raw,
        )
        .expect("raw test message writes");
        workspace
            .write_message_artifacts(&parsed.message)
            .expect("test message artifacts write");
        let mut message_document = workspace
            .triage_template_document(&parsed.message.message_id)
            .expect("triage render document")
            .context
            .message;
        let unsafe_body = "Readable human message <script>alert(1)</script>";
        message_document.message.subject =
            Some("Readable <img src=x onerror=alert(1)>".to_string());
        message_document.message.body_text = unsafe_body.to_string();
        message_document.analysis.body.segments =
            vec![crate::render_document::MessageBodySegment {
                kind: crate::render_document::MessageBodySegmentKind::Authored,
                start_bytes: 0,
                end_bytes: unsafe_body.len(),
            }];
        let ReviewContents::Case(contents) = &mut model.contents else {
            unreachable!();
        };
        contents.collection.messages.push(message_document);
        let page = render_builtin_page(&workspace, &model).expect("typed Review template renders");
        assert!(page.contains("Notes"));
        assert!(page.contains("Reply"));
        assert!(page.contains("Readable human message"));
        assert!(page.contains("Readable human message &lt;script&gt;alert(1)&lt;"));
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(!page.contains("<img src=x onerror=alert(1)>"));
        assert!(page.contains("message-card"));
        assert!(!page.contains("<pre"));
        // No embedded document for a client to hydrate from.
        assert!(!page.contains("type=\"application/json\""));
        assert!(!page.contains("document.createElement"));
        assert!(page.contains("data-afmail-review-content"));
        assert!(page.contains("template.content.cloneNode(true)"));
        assert!(page.contains("for (const form of [...dirty])"));
        assert!(page.contains("refreshGeneration += 1"));
        assert!(page.contains("Autosaves to this workspace"));
        assert!(
            page.contains("setTimeout(() =&gt; persist(form), 650)")
                || page.contains("setTimeout(() => persist(form), 650)")
        );
        assert!(page.contains("beforeunload"));
        assert!(page.contains("data-copy-settled-repair-required="));
        assert!(page.contains(
            "if (result.repair_required) form.dataset.afmailSettlementPersisted = 'true'"
        ));
        assert!(
            page.contains("button.disabled = form.dataset.afmailSettlementPersisted === 'true'")
        );
        assert!(page.contains("Quoted history and attachments remain unchanged."));
        // This case has no canonical suggestion, so Review offers the live
        // Agent turn instead of inventing a decision menu. The trusted runtime
        // still carries settlement behavior for pages that do have one.
        assert!(page.contains(r#"data-afmail-autosave="notes""#));
        assert!(!page.contains(r#"name="action""#));
        assert!(page.contains("data-afmail-agent-form"));
        assert!(page.contains(r#"aria-keyshortcuts="Meta+Enter Control+Enter""#));
        assert!(page.contains(r#"data-apple-label="⌘↵""#));
        assert!(page.contains(r#"data-control-label="Ctrl↵""#));
        assert!(page.contains("agentForm.requestSubmit()"));
        assert!(page.contains("selectingAgentMessage.contains(focusNode)"));
        assert!(page.contains("contain: paint"));
        assert!(!page.contains(r#"data-afmail-settle="case""#));
        assert!(page.contains("data-afmail-settle-button"));
        assert!(page.contains("type: 'settle'"));
        assert!(page.contains("afui.connect"));

        let runtime = include_str!("review_runtime.js");
        let parsed_index = runtime
            .find("const parsed = new DOMParser()")
            .expect("runtime parses refreshed Review content");
        let dirty_recheck_index = runtime[parsed_index..]
            .find("if (dirty.size > 0 || localEditGeneration !== localEditsAtStart) return;")
            .map(|index| parsed_index + index)
            .expect("runtime rechecks live edits after awaiting refreshed content");
        let replacement_index = runtime
            .find("currentSuggestion.replaceWith(nextSuggestion)")
            .expect("runtime replaces canonical suggestion content");
        assert!(
            dirty_recheck_index < replacement_index,
            "live edits must be checked before refreshed DOM replaces any Review content"
        );
        assert!(runtime.contains("localEditGeneration += 1"));
        assert!(runtime.contains("const localEditsAtStart = localEditGeneration"));
        assert!(!runtime.contains("applePlatform ? '⌘↵'"));
        assert!(runtime.contains("agentShortcut.dataset.appleLabel"));
        assert!(runtime.contains("agentShortcut.dataset.controlLabel"));
        assert!(runtime.contains("sessionRuntime.call"));
        assert!(!runtime.contains("afui.request"));
        assert!(!runtime.contains("afui.channel"));

        assert!(
            saved_draft_target(
                &ReviewSurface::Case {
                    case_ref: case_ref.clone()
                },
                &model,
                &draft_name,
            )
            .is_ok()
        );
        assert_eq!(
            saved_draft_target(&ReviewSurface::Case { case_ref }, &model, "../other.md")
                .expect_err("unlisted draft must fail")
                .error_code,
            "review_draft_not_allowed"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
