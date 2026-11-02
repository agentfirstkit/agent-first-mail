//! Keep only server status replies, never commands, credentials or message literals.
use crate::error::AppError;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;

#[derive(Clone, Default)]
pub(crate) struct AppendDiagnostics(Rc<RefCell<StatusReader>>);

#[derive(Default)]
struct StatusReader {
    active: bool,
    line: Vec<u8>,
    over_limit: bool,
    rejection: Option<(String, bool)>,
}

impl AppendDiagnostics {
    pub(crate) fn clear(&self) {
        *self.0.borrow_mut() = StatusReader {
            active: true,
            ..StatusReader::default()
        };
    }

    pub(crate) fn stop(&self) {
        self.0.borrow_mut().active = false;
    }

    pub(crate) fn enrich(&self, mut error: AppError) -> AppError {
        self.0.borrow_mut().active = false;
        if let Some((response, rejected)) = &self.0.borrow().rejection {
            let rejected = *rejected
                || error
                    .details
                    .as_ref()
                    .and_then(|details| details.get("remote_effect_rejected"))
                    .and_then(serde_json::Value::as_bool)
                    == Some(true);
            error.message = response.clone();
            error.details = Some(serde_json::json!({
                "server_response": response,
                "remote_effect_rejected": rejected,
            }));
        }
        error
    }
}

pub(crate) struct ObservedStream<T> {
    inner: T,
    diagnostics: AppendDiagnostics,
}

impl<T> ObservedStream<T> {
    pub(crate) fn new(inner: T, diagnostics: AppendDiagnostics) -> Self {
        Self { inner, diagnostics }
    }
}

impl<T: Read> Read for ObservedStream<T> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        let mut reader = self.diagnostics.0.borrow_mut();
        if !reader.active {
            return Ok(count);
        }
        for byte in &bytes[..count] {
            if reader.line.len() < 16 * 1024 {
                reader.line.push(*byte);
            } else {
                reader.over_limit = true;
            }
            if *byte == b'\n' {
                if !reader.over_limit {
                    let rejected = match imap_proto::Response::parse(&reader.line) {
                        Ok((
                            _,
                            imap_proto::Response::Done {
                                status: imap_proto::Status::No | imap_proto::Status::Bad,
                                ..
                            },
                        )) => Some(true),
                        Ok((
                            _,
                            imap_proto::Response::Data {
                                status:
                                    imap_proto::Status::No
                                    | imap_proto::Status::Bad
                                    | imap_proto::Status::Bye,
                                ..
                            },
                        )) => Some(false),
                        _ => None,
                    };
                    if let Some(rejected) = rejected
                        && reader
                            .rejection
                            .as_ref()
                            .is_none_or(|(_, prior_rejected)| !prior_rejected)
                    {
                        reader.rejection = Some((
                            String::from_utf8_lossy(&reader.line).trim().to_string(),
                            rejected,
                        ));
                    }
                }
                reader.line.clear();
                reader.over_limit = false;
            }
        }
        Ok(count)
    }
}

impl<T: Write> Write for ObservedStream<T> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_server_rejection_and_bye_are_retained_without_client_data() {
        for (reply, rejected) in [
            ("a2 NO [OVERQUOTA] mailbox full\r\n", true),
            ("a2 BAD invalid APPEND\r\n", true),
            ("* BYE [UNAVAILABLE] try later\r\n", false),
        ] {
            let diagnostics = AppendDiagnostics::default();
            let mut stream = ObservedStream::new(
                std::io::Cursor::new(reply.as_bytes().to_vec()),
                diagnostics.clone(),
            );
            diagnostics.clear();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap() != 0 {}
            let error = diagnostics.enrich(AppError::new("imap_append_failed", "generic"));
            assert_eq!(error.message, reply.trim());
            assert_eq!(error.details.unwrap()["remote_effect_rejected"], rejected);
        }
    }

    #[test]
    fn inactive_observation_and_reset_never_reuse_a_previous_rejection() {
        let diagnostics = AppendDiagnostics::default();
        let reply = b"a1 NO old reply\r\n";
        let mut stream = ObservedStream::new(std::io::Cursor::new(reply), diagnostics.clone());
        let mut data = Vec::new();
        stream.read_to_end(&mut data).unwrap();
        assert_eq!(
            diagnostics
                .enrich(AppError::new("test", "unchanged"))
                .message,
            "unchanged"
        );
        diagnostics.clear();
        assert!(
            diagnostics
                .enrich(AppError::new("test", "unchanged"))
                .details
                .is_none()
        );
    }
}
