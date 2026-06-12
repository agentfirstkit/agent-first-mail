use crate::config::{ImapConfig, MailConfig, SpecialUseKind};
use crate::error::{AppError, Result};
use crate::imap_client::{ImapClientSession, MailboxInfo, MoveOutcome};
use crate::types::RemoteLocation;
use serde_json::Value;
use std::cell::RefCell;

/// Remote mail side effects used by push/pull code.
///
/// Workspace code depends on this trait so retry/state-machine tests can use a
/// fake provider without reaching IMAP or SMTP.
pub trait MailRemote {
    fn list_mailboxes(&self) -> Result<Value>;
    fn action_mailbox_folder(&self, mailbox_id: &str) -> Result<String>;
    fn append_message(&self, folder: &str, raw_eml: &[u8], draft: bool) -> Result<()>;
    fn move_message(
        &self,
        source_folder: &str,
        uid: u64,
        target_folder: &str,
        rfc822_message_id: Option<&str>,
    ) -> Result<MoveOutcome>;
    fn add_flags(&self, source_folder: &str, uid: u64, flags: &[String]) -> Result<()>;
    fn send_raw_message(
        &self,
        envelope_from: &str,
        envelope_to: &[String],
        raw: &[u8],
    ) -> Result<()>;
    fn find_by_message_id(
        &self,
        _folder: &str,
        _rfc822_message_id: &str,
    ) -> Result<Option<RemoteLocation>> {
        Err(AppError::new(
            "remote_operation_unsupported",
            "find_by_message_id is not supported by this provider",
        ))
    }
}

pub struct ImapSmtpRemote<'a> {
    config: &'a MailConfig,
    imap: Option<ImapConfig>,
    session: RefCell<Option<ImapClientSession>>,
    mailboxes: RefCell<Option<Vec<MailboxInfo>>>,
}

impl<'a> ImapSmtpRemote<'a> {
    pub fn new(config: &'a MailConfig) -> Self {
        Self {
            config,
            imap: None,
            session: RefCell::new(None),
            mailboxes: RefCell::new(None),
        }
    }

    fn imap(&self) -> Result<ImapConfig> {
        if let Some(imap) = &self.imap {
            return Ok(imap.clone());
        }
        self.config.require_imap()
    }

    fn with_session<T>(
        &self,
        operation: impl FnOnce(&mut ImapClientSession) -> Result<T>,
    ) -> Result<T> {
        if self.session.borrow().is_none() {
            let imap = self.imap()?;
            *self.session.borrow_mut() = Some(ImapClientSession::connect(&imap)?);
        }
        let mut session = self.session.borrow_mut();
        let Some(session) = session.as_mut() else {
            return Err(AppError::new(
                "imap_session_missing",
                "IMAP session was not initialized",
            ));
        };
        operation(session)
    }

    fn cached_mailboxes(&self) -> Result<Vec<MailboxInfo>> {
        if let Some(mailboxes) = self.mailboxes.borrow().clone() {
            return Ok(mailboxes);
        }
        let mailboxes = self.with_session(|session| session.list_mailboxes())?;
        *self.mailboxes.borrow_mut() = Some(mailboxes.clone());
        Ok(mailboxes)
    }
}

impl MailRemote for ImapSmtpRemote<'_> {
    fn list_mailboxes(&self) -> Result<Value> {
        let imap = self.imap()?;
        crate::imap_pull::remote_folders(self.config, &imap)
    }

    fn action_mailbox_folder(&self, mailbox_id: &str) -> Result<String> {
        let mailbox = self.config.mailbox(mailbox_id)?;
        if let Some(folder) = &mailbox.mailbox_name {
            return Ok(folder.clone());
        }
        if let Some(kind) = mailbox
            .special_use
            .as_deref()
            .and_then(SpecialUseKind::from_attribute)
        {
            let mailboxes = self.cached_mailboxes()?;
            return Ok(crate::imap_pull::resolve_special_use_from_mailboxes(
                self.config,
                kind,
                &mailboxes,
            )
            .mailbox_name);
        }
        self.config.offline_mailbox_name(mailbox_id)
    }

    fn append_message(&self, folder: &str, raw_eml: &[u8], draft: bool) -> Result<()> {
        self.with_session(|session| session.append_message(folder, raw_eml, draft))
    }

    fn move_message(
        &self,
        source_folder: &str,
        uid: u64,
        target_folder: &str,
        rfc822_message_id: Option<&str>,
    ) -> Result<MoveOutcome> {
        self.with_session(|session| {
            session.uid_mark_and_move(
                source_folder,
                uid,
                target_folder,
                rfc822_message_id,
                false,
                None,
            )
        })
    }

    fn add_flags(&self, source_folder: &str, uid: u64, flags: &[String]) -> Result<()> {
        self.with_session(|session| session.uid_store_flags(source_folder, uid, flags, true))
    }

    fn send_raw_message(
        &self,
        envelope_from: &str,
        envelope_to: &[String],
        raw: &[u8],
    ) -> Result<()> {
        crate::smtp_send::send_raw_message(self.config, envelope_from, envelope_to, raw)
    }

    fn find_by_message_id(
        &self,
        folder: &str,
        rfc822_message_id: &str,
    ) -> Result<Option<RemoteLocation>> {
        self.with_session(|session| session.find_uid_by_message_id(folder, rfc822_message_id))
            .map(Some)
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct FakeMailRemote {
    pub append_results: std::cell::RefCell<std::collections::VecDeque<Result<()>>>,
    pub move_results: std::cell::RefCell<std::collections::VecDeque<Result<MoveOutcome>>>,
    pub add_flags_results: std::cell::RefCell<std::collections::VecDeque<Result<()>>>,
    pub send_results: std::cell::RefCell<std::collections::VecDeque<Result<()>>>,
}

#[cfg(test)]
impl MailRemote for FakeMailRemote {
    fn list_mailboxes(&self) -> Result<Value> {
        Ok(serde_json::json!({"code": "remote_folders", "folders": []}))
    }

    fn action_mailbox_folder(&self, mailbox_id: &str) -> Result<String> {
        Ok(mailbox_id.to_string())
    }

    fn append_message(&self, _folder: &str, _raw_eml: &[u8], _draft: bool) -> Result<()> {
        self.append_results
            .borrow_mut()
            .pop_front()
            .unwrap_or(Ok(()))
    }

    fn move_message(
        &self,
        _source_folder: &str,
        _uid: u64,
        target_folder: &str,
        _rfc822_message_id: Option<&str>,
    ) -> Result<MoveOutcome> {
        self.move_results
            .borrow_mut()
            .pop_front()
            .unwrap_or(Ok(MoveOutcome {
                keyword_set: false,
                keyword_error: None,
                seen_set: false,
                seen_error: None,
                moved: true,
                target_location: Some(RemoteLocation {
                    mailbox_name: target_folder.to_string(),
                    mailbox_id: None,
                    uid_validity: Some(1),
                    uid: Some(1),
                    flags: Vec::new(),
                    observed_rfc3339: crate::store::now_rfc3339(),
                    missing_rfc3339: None,
                }),
            }))
    }

    fn add_flags(&self, _source_folder: &str, _uid: u64, _flags: &[String]) -> Result<()> {
        self.add_flags_results
            .borrow_mut()
            .pop_front()
            .unwrap_or(Ok(()))
    }

    fn send_raw_message(
        &self,
        _envelope_from: &str,
        _envelope_to: &[String],
        _raw: &[u8],
    ) -> Result<()> {
        self.send_results.borrow_mut().pop_front().unwrap_or(Ok(()))
    }
}
