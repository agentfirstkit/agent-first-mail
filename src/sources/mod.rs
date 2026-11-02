#[cfg(feature = "cloudflare")]
pub(crate) mod cloudflare;
#[cfg(not(feature = "cloudflare"))]
#[path = "cloudflare_disabled.rs"]
pub(crate) mod cloudflare;
pub(crate) mod consumer_shutdown;
pub mod imap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailSourceKind {
    Imap,
    Cloudflare,
    File,
}

pub fn configured(config: &crate::config::MailConfig) -> Vec<MailSourceKind> {
    let mut sources = Vec::new();
    if config
        .imap
        .host
        .as_deref()
        .is_some_and(|host| !host.trim().is_empty())
    {
        sources.push(MailSourceKind::Imap);
    }
    if config.cloudflare.is_configured() {
        sources.push(MailSourceKind::Cloudflare);
    }
    if !config.files.sources.is_empty() {
        sources.push(MailSourceKind::File);
    }
    sources
}
#[path = "cloudflare/journal.rs"]
pub(crate) mod cloudflare_journal;
