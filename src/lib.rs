#[cfg(feature = "api")]
pub mod api;
#[cfg(not(feature = "api"))]
#[path = "api_disabled.rs"]
pub mod api;
// One safe way to turn a draft's `attachments:` list into bytes.
pub(crate) mod attachments;
pub(crate) mod body_markdown;
pub mod channels;
pub mod cli;
pub mod config;
#[cfg(feature = "demo")]
pub(crate) mod demo;
#[cfg(not(feature = "demo"))]
#[path = "demo_disabled.rs"]
pub(crate) mod demo;
mod draft_content;
pub mod error;
pub mod frontmatter;
pub(crate) mod imap_client;
mod imap_diagnostics;
pub(crate) mod ingest;
pub(crate) mod layout;
pub mod mail;
pub mod markdown;
pub mod outbound;
pub(crate) mod plan_id;
pub(crate) mod progress;
pub mod push_queue;
pub mod remote;
pub(crate) mod render_document;
#[cfg(feature = "review")]
pub(crate) mod review_frontend;
pub mod runner;
pub mod senders;
pub mod skill_admin;
mod slug;
pub mod sources;
pub mod store;
pub mod templates;
pub mod types;
mod util;
pub(crate) mod workspace_identity;
mod workspace_lock;
