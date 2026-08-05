mod api;
mod build;
mod cases;
mod common;
mod contacts;
mod demo;
mod message;
mod notification;
mod root;
mod skill;
mod spec;

pub use api::{ApiAction, ApiMode};
pub use build::{ActionHandler, ArgumentError, Globals, action_handlers, build_command, globals};
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
pub use message::{MessageAction, MessageAttachmentAction, MessageListScope};
pub use notification::{
    NotificationCommand, NotificationCreateArgs, NotificationNotesAction,
    NotificationSuggestionAction, NotificationSuggestionActionKind, NotificationSuggestionSetArgs,
};
pub use root::Command;
pub use skill::{SkillAction, SkillAgentSelection, SkillScope, SkillTargetArgs, SkillWriteArgs};
pub use spec::cli_spec;
