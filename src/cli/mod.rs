mod archive;
mod cases;
mod common;
mod contacts;
mod hints;
mod message;
mod notification;
mod parse;
mod root;
mod skill;
#[cfg(feature = "ui")]
mod ui;

pub use archive::{ArchiveAction, ArchiveCaseAction, ArchiveCaseNotesAction, ArchiveListAction};
pub use cases::{CaseCommand, CaseCreateArgs, CaseDraftAction, CaseNotesAction};
pub use common::{
    ConfigAction, DoctorAction, LogAction, PurgeAction, PushAction, RemoteAction, RenderAction,
    TriageAction,
};
pub use contacts::{
    ContactCommand, ContactCreateArgs, ContactEmailAction, ContactNotesAction, ContactPhoneAction,
};
#[doc(hidden)]
pub use hints::error_hint;
pub use message::{MessageAction, MessageAttachmentAction};
pub use notification::{NotificationCommand, NotificationCreateArgs, NotificationNotesAction};
pub use parse::{command, parse_args, ParsedArgs};
pub use root::{Cli, Command};
pub use skill::{SkillAction, SkillAgentSelection, SkillScope, SkillTargetArgs, SkillWriteArgs};
#[cfg(feature = "ui")]
pub use ui::{UiAction, UiCommand};
