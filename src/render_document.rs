//! Versioned, output-neutral documents shared by Markdown and Review templates.
//!
//! These types deliberately contain canonical mailbox/workspace facts plus
//! output-independent mail analysis. They do not contain localized copy,
//! display aliases, CSS classes, formatted dates, or pre-rendered markup.

use crate::frontmatter::DraftFrontmatter;
use crate::types::{
    ContactFrontmatter, DraftCompleteness, DraftStatus, MessageCollection, MessageFile,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) const RENDER_DOCUMENT_SCHEMA_VERSION: u64 = 3;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub content_trust: ContentTrust,
    pub content_provenance: ContentProvenance,
    pub message: MessageFile,
    pub analysis: MessageRenderAnalysis,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContentTrust {
    UntrustedRemote,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContentProvenance {
    pub source: ContentSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContentSource {
    RemoteMail,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageRenderAnalysis {
    pub direction: RenderMessageDirection,
    pub action: RenderMessageAction,
    pub body: MessageBodyAnalysis,
    /// The reading copy of an HTML body, present exactly when
    /// `message.body_markdown` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_markdown: Option<MarkdownBodyAnalysis>,
    pub security: MessageSecurityAnalysis,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RenderMessageDirection {
    Inbound,
    Outbound,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RenderMessageAction {
    Message,
    Reply,
    Forward,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageBodyAnalysis {
    pub segments: Vec<MessageBodySegment>,
}

/// `message.body_markdown` split the same way as `body_text` — byte ranges
/// for Markdown consumers — and already laid out as blocks for the Review
/// page, which may not render markup from a value and so writes the `<p>`
/// and `<a>` itself.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarkdownBodyAnalysis {
    pub segments: Vec<MessageBodySegment>,
    pub authored: Vec<crate::body_markdown::MarkdownBlock>,
    pub quoted: Vec<crate::body_markdown::MarkdownBlock>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageBodySegment {
    pub kind: MessageBodySegmentKind,
    pub start_bytes: usize,
    pub end_bytes: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageBodySegmentKind {
    Authored,
    Quoted,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageSecurityAnalysis {
    pub authentication_warning: bool,
    pub possible_bcc: bool,
    pub reply_to_differs: bool,
    pub sender_differs: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub collection: MessageCollection,
    pub messages: Vec<MessageRenderDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<CollectionLocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionMessageRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub collection: MessageCollection,
    pub message: MessageRenderDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<CollectionLocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionLocation {
    pub group: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageContextDocument {
    pub message: MessageRenderDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_messages: Vec<MessageRenderDocument>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TriageRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub context: MessageContextDocument,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageShowDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub context: MessageContextDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_refs: Vec<CaseReferenceDocument>,
    pub locator: MessageShowLocator,
    pub review_base_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageShowLocator {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_path: Option<String>,
    pub json_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionShowDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub context: CollectionRenderDocument,
    pub locator: CollectionShowLocator,
    pub review_base_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionShowLocator {
    pub collection_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_path: Option<String>,
    pub notes_path: String,
    pub messages_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaseReferenceDocument {
    pub case_uid: String,
    pub group: String,
    pub case_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub locator: DraftLocator,
    pub draft: DraftSourceDocument,
    pub analysis: DraftRenderAnalysis,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftLocator {
    pub draft_name: String,
    pub draft_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftSourceDocument {
    pub frontmatter: DraftFrontmatter,
    pub body_markdown: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftRenderAnalysis {
    pub authored_body_range: TextRange,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<DraftAttachmentAnalysis>,
    pub completeness: DraftCompleteness,
    pub status: DraftStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextRange {
    pub start_bytes: usize,
    pub end_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftAttachmentAnalysis {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmptyRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContactRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub locator: ContactLocator,
    pub contact: ContactFrontmatter,
    pub notes_markdown: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContactLocator {
    pub group: String,
    pub contact_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftReplyRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub message: MessageRenderDocument,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotesMergeRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub primary: NotesRenderDocument,
    pub merged_case_uid: String,
    pub merged: NotesRenderDocument,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NotesRenderDocument {
    pub schema_name: String,
    pub schema_version: u64,
    pub notes_path: String,
    pub text: String,
}

pub(crate) fn message_document_schema_name() -> String {
    "afmail_message_render_document".to_string()
}

pub(crate) fn collection_document_schema_name() -> String {
    "afmail_collection_render_document".to_string()
}

pub(crate) fn collection_message_document_schema_name() -> String {
    "afmail_collection_message_render_document".to_string()
}

pub(crate) fn triage_document_schema_name() -> String {
    "afmail_triage_render_document".to_string()
}

pub(crate) fn draft_document_schema_name() -> String {
    "afmail_draft_render_document".to_string()
}

pub(crate) fn notes_document_schema_name() -> String {
    "afmail_notes_render_document".to_string()
}
