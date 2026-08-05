use super::common::ReviewMode;

#[derive(Debug)]
pub enum CaseCommand {
    /// Create a new case and return its stable UID/ref.
    Create(CaseCreateArgs),
    /// List compact case locators.
    List { archived: bool, all: bool },
    /// Show the exact typed case collection document used by Markdown and Review.
    Show { case_ref: String },
    /// Open a review session for this case, including editable notes.
    Review { case_ref: String, mode: ReviewMode },
    /// Set or clear the current typed Agent suggestion for this case.
    Suggestion { action: CaseSuggestionAction },
    /// Add a message to this existing case.
    Add {
        case_ref: String,
        message_id: Vec<String>,
        summary: Option<String>,
        reason: Option<String>,
    },
    /// Set or replace one case entry summary.
    SetSummary {
        case_ref: String,
        message_id: String,
        summary: String,
        reason: Option<String>,
    },
    /// Remove one message from this case and return it to triage.
    Remove {
        case_ref: String,
        message_id: String,
        reason: Option<String>,
    },
    /// Move this case to another group.
    Move {
        case_ref: String,
        group: String,
        reason: Option<String>,
    },
    /// Rename this case's human-readable name without changing its UID.
    Rename {
        case_ref: String,
        name: String,
        reason: Option<String>,
    },
    /// Show or edit case notes.
    Notes { action: CaseNotesAction },
    /// Archive this active case.
    Archive {
        case_ref: String,
        reason: Option<String>,
    },
    /// Move this archived case back to an active case group.
    Unarchive {
        case_ref: String,
        group: String,
        reason: Option<String>,
    },
    /// Move every message in this case to local trash and queue Trash moves.
    Trash {
        case_ref: String,
        reason: Option<String>,
    },
    /// Delete an empty case.
    Delete {
        case_ref: String,
        allow_notes: bool,
        reason: Option<String>,
    },
    /// Add a case organization tag.
    Tag {
        case_ref: String,
        tag: String,
        reason: Option<String>,
    },
    /// Remove a case organization tag.
    Untag {
        case_ref: String,
        tag: String,
        reason: Option<String>,
    },
    /// Create, edit, validate, queue, or remove local case drafts.
    Draft { action: CaseDraftAction },
    /// Merge another case into this case.
    Merge {
        case_ref: String,
        other_case_ref: String,
        reason: Option<String>,
    },
}

#[derive(Debug)]
pub enum CaseSuggestionAction {
    /// Replace the current suggestion and clear any human review of the prior round.
    Set(CaseSuggestionSetArgs),
    /// Clear exactly the current suggestion round.
    Clear {
        case_ref: String,
        agent_suggestion_uid: String,
    },
}

#[derive(Debug)]
pub struct CaseSuggestionSetArgs {
    pub case_ref: String,
    pub case_review_base_hash: String,
    pub reason: String,
    pub confidence_percent: Option<u8>,
    pub action: CaseSuggestionActionKind,
}

#[derive(Debug)]
pub enum CaseSuggestionActionKind {
    Archive,
    Unarchive { group: String },
    Move { group: String },
    Rename { name: String },
    Tag { tag: String },
    Untag { tag: String },
    Trash,
    Delete { allow_notes: bool },
}

#[derive(Debug, Clone)]
pub struct CaseCreateArgs {
    /// Human-readable case name used in case.md and the directory suffix.
    pub name: String,
    /// Required case category/group, for example support, invoice, 客服, 发票, 合同, or 退款.
    pub group: Option<String>,
    /// Optional first message to add to the case.
    pub message: Option<String>,
    /// Short summary for the first message.
    pub summary: Option<String>,
    /// Why this case is being created.
    pub reason: Option<String>,
}

#[derive(Debug)]
pub enum CaseNotesAction {
    /// Show notes markdown.
    Show { case_ref: String },
    /// Append text to notes markdown.
    Append { case_ref: String, text: String },
    /// Replace notes markdown with text.
    Replace { case_ref: String, text: String },
}

#[derive(Debug)]
pub enum CaseDraftAction {
    /// Scaffold a new outbound draft (not a reply) in this case.
    New {
        case_ref: String,
        to: Vec<String>,
        cc: Vec<String>,
        bcc: Vec<String>,
        subject: String,
        identity: Option<String>,
        body: Option<String>,
        body_file: Option<String>,
    },
    /// Scaffold a reply draft to a message, prefilled and quoting the original.
    Reply {
        case_ref: String,
        message_id: String,
        bcc: Vec<String>,
        identity: Option<String>,
        body: Option<String>,
        body_file: Option<String>,
        all: bool,
    },
    /// Change an existing editable draft in place.
    Change {
        case_ref: String,
        draft_name: String,
        subject: Option<String>,
        to: Vec<String>,
        cc: Vec<String>,
        clear_cc: bool,
        bcc: Vec<String>,
        clear_bcc: bool,
        identity: Option<String>,
        body: Option<String>,
        body_file: Option<String>,
    },
    /// Show an existing draft as a typed render document.
    Show {
        case_ref: String,
        draft_name: String,
    },
    /// Open a review session for this draft.
    Review {
        case_ref: String,
        draft_name: String,
        mode: ReviewMode,
    },
    /// Validate a draft under the case drafts directory.
    Validate {
        case_ref: String,
        draft_name: String,
    },
    /// List local drafts with their current status.
    List { case_ref: String },
    /// Queue this draft to be uploaded to the remote Drafts mailbox.
    Upload {
        case_ref: String,
        draft_name: String,
    },
    /// Queue this draft to be sent and recorded in the case after push succeeds.
    Send {
        case_ref: String,
        draft_name: String,
    },
    /// Copy or reference a file and add it to a draft's attachments.
    Attach {
        case_ref: String,
        draft_name: String,
        path: String,
    },
    /// Remove a local draft and any queued outbound item for it.
    Remove {
        case_ref: String,
        draft_name: String,
        reason: Option<String>,
    },
}
