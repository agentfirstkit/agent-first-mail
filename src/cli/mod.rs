mod api;
mod cases;
mod common;
mod contacts;
mod demo;
mod hints;
mod message;
mod notification;
mod parse;
mod root;
mod skill;

pub use api::{ApiAction, ApiMode};
pub use cases::{
    CaseCommand, CaseCreateArgs, CaseDraftAction, CaseNotesAction, CaseSuggestionAction,
    CaseSuggestionActionKind, CaseSuggestionSetArgs,
};
pub use common::{
    ConfigAction, DoctorAction, LogAction, MessageSuggestionAction, PurgeAction, PushAction,
    RemoteAction, RenderAction, ReviewMode, TriageAction, TriageSuggestionAction,
    TriageSuggestionSetArgs,
};
pub use contacts::{
    ContactCommand, ContactCreateArgs, ContactEmailAction, ContactNotesAction, ContactPhoneAction,
};
pub use demo::{DemoAction, DemoScenario};
#[doc(hidden)]
pub use hints::error_hint;
pub use message::{MessageAction, MessageAttachmentAction, MessageListScope};
pub use notification::{
    NotificationCommand, NotificationCreateArgs, NotificationNotesAction,
    NotificationSuggestionAction, NotificationSuggestionActionKind, NotificationSuggestionSetArgs,
};
pub use parse::{ParsedArgs, command, parse_args};
pub use root::{Cli, Command};
pub use skill::{SkillAction, SkillAgentSelection, SkillScope, SkillTargetArgs, SkillWriteArgs};
