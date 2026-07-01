mod case_archive;
pub mod contact;
mod ids;
mod message;
mod push;

pub use case_archive::{
    CaseMessages, MessageCollection, MessageCollectionItem, NotificationMessageItem,
    NotificationMessages, CASE_SCHEMA_NAME, MESSAGE_COLLECTION_SCHEMA_VERSION,
    NOTIFICATION_SCHEMA_NAME,
};
pub use contact::{ContactFrontmatter, CONTACT_KIND};
pub use ids::{CaseUid, MessageId, NotificationUid, PushId};
pub use message::{
    AttachmentRef, AuthAlignment, AuthVerdict, ImapRef, MailDirection, MessageAuthentication,
    MessageContact, MessageFile, MessageStatus, RemoteLocation, RemoteState, RemoteSyncState,
    WorkspacePendingPush, WorkspacePushState, WorkspaceState,
};
pub use push::{
    MessageActionPush, MessagePushAction, OutboundAction, OutboundPush, PushItem, PushKind,
    PushLocalStage, PushLocation, PushPayload, PushStepState, PushStepStatus,
};
