use crate::config::{MailConfig, SmtpConfig};
use crate::error::{AppError, Result};
use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};

/// Everything a send needs before anything can leave the machine.
///
/// The relay configuration, the transport and the envelope are all built from
/// local state, and every way they can fail is a mistake that delivered
/// nothing. Doing it here lets the caller find those out while the step is
/// still plainly retryable, instead of after it has written down that this
/// step's outcome is unknown — which is a state a queue cannot recover from on
/// its own.
pub fn prepare_send(
    config: &MailConfig,
    envelope_from: &str,
    envelope_to: &[String],
) -> Result<(lettre::SmtpTransport, Envelope)> {
    let smtp = config.require_smtp()?;
    let sender = build_transport(&smtp)?;
    let from = envelope_from
        .parse::<Address>()
        .map_err(|e| AppError::new("smtp_send_failed", format!("invalid envelope from: {e}")))?;
    let mut to = Vec::new();
    for address in envelope_to {
        to.push(address.parse::<Address>().map_err(|e| {
            AppError::new(
                "smtp_send_failed",
                format!("invalid envelope recipient: {e}"),
            )
        })?);
    }
    let envelope = Envelope::new(Some(from), to)
        .map_err(|e| AppError::new("smtp_send_failed", e.to_string()))?;
    Ok((sender, envelope))
}

pub fn send_raw_message(
    config: &MailConfig,
    envelope_from: &str,
    envelope_to: &[String],
    raw: &[u8],
) -> Result<()> {
    // Preparation runs again here, after the caller recorded an unknown
    // outcome; a failure now (say a secret source that became unreadable)
    // still sent nothing and must say so.
    let (sender, envelope) =
        prepare_send(config, envelope_from, envelope_to).map_err(|mut error| {
            match error.details.as_mut() {
                Some(serde_json::Value::Object(details)) => {
                    details.insert("remote_calls_started".into(), false.into());
                }
                _ => error.details = Some(serde_json::json!({"remote_calls_started": false})),
            }
            error
        })?;
    // From here on the outcome genuinely can be unknown: the connection, the
    // handshake, the envelope exchange and DATA all happen inside this call.
    // A negative SMTP reply is different: the server explicitly rejected it.
    sender.send_raw(&envelope, raw).map_err(|error| {
        let rejected = error.is_permanent() || error.is_transient();
        let app_error = AppError::new("smtp_send_failed", error.to_string());
        if rejected {
            app_error.with_details(serde_json::json!({"remote_effect_rejected": true}))
        } else {
            app_error
        }
    })?;
    Ok(())
}

fn build_transport(config: &SmtpConfig) -> Result<SmtpTransport> {
    crate::config::validate_smtp_auth(
        config.username.as_deref(),
        config.password_secret.as_deref(),
    )?;
    let mut builder = if config.tls_wrapper {
        SmtpTransport::relay(&config.host)
            .map_err(|e| AppError::new("smtp_connect_failed", e.to_string()))?
            .port(config.port)
    } else if config.starttls {
        SmtpTransport::starttls_relay(&config.host)
            .map_err(|e| AppError::new("smtp_connect_failed", e.to_string()))?
            .port(config.port)
    } else {
        crate::util::ensure_plaintext_credentials_allowed(
            "SMTP",
            &config.host,
            config.username.is_some() && config.password_secret.is_some(),
        )?;
        SmtpTransport::builder_dangerous(&config.host).port(config.port)
    };
    if let (Some(username), Some(password)) = (&config.username, &config.password_secret) {
        builder = builder.credentials(Credentials::new(username.clone(), password.clone()));
    }
    Ok(builder.build())
}

pub(crate) fn remote_test(config: &SmtpConfig) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let transport = build_transport(config)?;
    let connected = transport
        .test_connection()
        .map_err(|error| retryable_smtp_test_error(config, "connect", error.to_string()))?;
    if !connected {
        return Err(retryable_smtp_test_error(
            config,
            "connect",
            "SMTP server closed the test connection".to_string(),
        ));
    }
    Ok(serde_json::json!({
        "status": "succeeded",
        "host": config.host,
        "port": config.port,
        "starttls": config.starttls,
        "tls_wrapper": config.tls_wrapper,
        "authenticated": config.username.is_some(),
        "message_sent": false,
        "duration_ms": started.elapsed().as_millis() as u64
    }))
}

fn retryable_smtp_test_error(
    config: &SmtpConfig,
    phase: &'static str,
    message: String,
) -> AppError {
    AppError::retryable("smtp_test_failed", message).with_details(serde_json::json!({
        "protocol": "smtp",
        "phase": phase,
        "host": config.host,
        "port": config.port,
        "starttls": config.starttls,
        "tls_wrapper": config.tls_wrapper
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn everything_that_cannot_have_sent_mail_fails_before_the_send() {
        // The queue writes "outcome unknown" immediately before the send and
        // cannot recover from it on its own, so a failure that plainly
        // delivered nothing has to surface earlier than that. These are all
        // local construction, and `prepare_send` is where they now happen.
        let mut config = MailConfig::default();
        config.smtp.host = Some("smtp.example.com".to_string());
        config.smtp.port = 587;
        config.smtp.starttls = true;

        // A recipient the envelope will not accept.
        assert!(
            prepare_send(&config, "me@example.com", &["not an address".to_string()]).is_err(),
            "an unusable recipient must fail before anything is sent"
        );
        // A sender the envelope will not accept.
        assert!(prepare_send(&config, "@", &["you@example.com".to_string()]).is_err());
        // No relay host at all.
        let unconfigured = MailConfig::default();
        assert!(
            prepare_send(
                &unconfigured,
                "me@example.com",
                &["you@example.com".to_string()]
            )
            .is_err()
        );

        // The send repeats preparation after the queue recorded an unknown
        // outcome; a failure there is tagged as having sent nothing.
        let error = send_raw_message(
            &unconfigured,
            "me@example.com",
            &["you@example.com".to_string()],
            b"Subject: x\r\n\r\nx",
        )
        .unwrap_err();
        assert_eq!(error.error_code, "config_missing");
        assert_eq!(
            error.details.unwrap()["remote_calls_started"],
            serde_json::json!(false)
        );

        // A usable configuration builds a transport and an envelope without
        // opening a connection — this must not touch the network.
        assert!(
            prepare_send(&config, "me@example.com", &["you@example.com".to_string()]).is_ok(),
            "preparation must not require the server to be reachable"
        );
    }

    #[derive(Clone, Copy)]
    enum SmtpScript {
        RejectRecipient,
        DeferSender,
        CloseAfterData,
    }

    /// One-connection SMTP server on a loopback port that follows `script`.
    fn scripted_smtp_server(script: SmtpScript) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind SMTP listener");
        let port = listener.local_addr().expect("SMTP listener address").port();
        let handle = std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let Ok(mut writer) = stream.try_clone() else {
                return;
            };
            let mut reader = BufReader::new(stream);
            let _ = writer.write_all(b"220 localhost ESMTP\r\n");
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let upper = line.to_ascii_uppercase();
                let reply: &[u8] = if upper.starts_with("MAIL FROM") {
                    match script {
                        SmtpScript::DeferSender => b"451 4.3.0 try again later\r\n",
                        _ => b"250 OK\r\n",
                    }
                } else if upper.starts_with("RCPT TO") {
                    match script {
                        SmtpScript::RejectRecipient => b"550 5.1.1 no such user\r\n",
                        _ => b"250 OK\r\n",
                    }
                } else if upper.starts_with("DATA") {
                    let _ = writer.write_all(b"354 end with <CRLF>.<CRLF>\r\n");
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == ".\r\n" {
                            break;
                        }
                    }
                    match script {
                        // The whole message arrived and the server said
                        // nothing: it may or may not have accepted it.
                        SmtpScript::CloseAfterData => return,
                        _ => b"250 OK\r\n",
                    }
                } else if upper.starts_with("QUIT") {
                    let _ = writer.write_all(b"221 bye\r\n");
                    return;
                } else {
                    b"250 OK\r\n"
                };
                if writer.write_all(reply).is_err() {
                    return;
                }
            }
        });
        (port, handle)
    }

    fn send_to_scripted_server(script: SmtpScript) -> AppError {
        let (port, server) = scripted_smtp_server(script);
        let mut config = MailConfig::default();
        config.smtp.host = Some("127.0.0.1".to_string());
        config.smtp.port = port;
        config.smtp.starttls = false;
        config.smtp.tls_wrapper = false;
        let result = send_raw_message(
            &config,
            "me@example.com",
            &["you@example.com".to_string()],
            b"Subject: test\r\n\r\nbody\r\n",
        );
        assert!(server.join().is_ok());
        result.expect_err("the scripted server never accepts a message")
    }

    #[test]
    fn a_negative_smtp_reply_is_rejected_and_a_dropped_connection_is_unknown() {
        for script in [SmtpScript::RejectRecipient, SmtpScript::DeferSender] {
            let error = send_to_scripted_server(script);
            assert_eq!(error.error_code, "smtp_send_failed");
            assert_eq!(
                error
                    .details
                    .as_ref()
                    .and_then(|details| details.get("remote_effect_rejected")),
                Some(&serde_json::json!(true)),
                "{error:?}"
            );
        }
        let error = send_to_scripted_server(SmtpScript::CloseAfterData);
        assert_eq!(error.error_code, "smtp_send_failed");
        assert!(error.details.is_none(), "{error:?}");
    }
}
