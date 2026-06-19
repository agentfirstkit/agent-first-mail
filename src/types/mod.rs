mod case_archive;
pub mod contact;
mod ids;
mod message;
mod push;

pub use case_archive::{
    ArchiveMessageItem, ArchiveMessages, CaseMessages, MessageCollection, MessageCollectionItem,
    ARCHIVE_NOTIFICATION_SCHEMA_NAME, CASE_SCHEMA_NAME, MESSAGE_COLLECTION_SCHEMA_VERSION,
};
pub use contact::{ContactFrontmatter, CONTACT_KIND};
pub use ids::{ArchiveUid, CaseUid, MessageId, PushId};
pub use message::{
    AttachmentRef, AuthAlignment, AuthVerdict, ImapRef, MailDirection, MessageAuthentication,
    MessageContact, MessageFile, MessageStatus, RemoteLocation, RemoteState, RemoteSyncState,
    WorkspacePendingPush, WorkspacePushState, WorkspaceState,
};
pub use push::{
    MessageActionPush, MessagePushAction, OutboundAction, OutboundPush, PushItem, PushKind,
    PushLocation, PushPayload, PushStepState, PushStepStatus,
};
