mod case_archive;
mod command_result;
pub mod contact;
mod draft;
mod ids;
mod message;
mod push;
mod suggestion;

pub use case_archive::{
    CASE_SCHEMA_NAME, CaseMessages, CollectionAgentSuggestion, CollectionHumanSuggestion,
    MESSAGE_COLLECTION_SCHEMA_VERSION, MessageCollection, MessageCollectionItem,
    NOTIFICATION_SCHEMA_NAME, NotificationMessageItem, NotificationMessages,
    SUGGESTIBLE_COLLECTION_SCHEMA_VERSION,
};
pub use command_result::CommandResult;
pub use contact::{CONTACT_KIND, ContactFrontmatter};
pub use draft::{
    DraftCompleteness, DraftProjectionIssue, DraftStateProjection, DraftStatus, DraftSyncIntent,
};
pub use ids::{CaseUid, MessageId, NotificationUid, PushId};
pub use message::{
    AttachmentRef, AuthAlignment, AuthVerdict, CaseRelationshipEvidence,
    CaseRelationshipSuggestion, ImapRef, MESSAGE_SCHEMA_VERSION, MailDirection,
    MessageAuthentication, MessageFile, MessageMetadata, MessageStatus, RemoteLocation,
    RemoteState, RemoteSyncState, WorkspacePendingPush, WorkspacePushState, WorkspaceState,
};
pub use push::{
    DraftPush, MessageActionPush, MessagePush, MessagePushAction, OutboundAction, OutboundPush,
    PUSH_ITEM_SCHEMA_VERSION, PreparedOutbound, PushAuditPending, PushItem, PushKind,
    PushLocalStage, PushLocation, PushPayload, PushStepState, PushStepStatus,
};
pub use suggestion::{
    CaseAgentSuggestion, CaseHumanSuggestion, CaseSuggestedAction, HumanSuggestionOutcome,
    MessageAgentSuggestion, MessageHumanSuggestion, MessageSuggestedAction,
    NotificationAgentSuggestion, NotificationHumanSuggestion, NotificationSuggestedAction,
};
