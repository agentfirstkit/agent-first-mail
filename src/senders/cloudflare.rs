use super::SendReceipt;
use crate::config::{MailConfig, resolve_password_secret};
use crate::error::{AppError, Result};
use crate::types::PreparedOutbound;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::Duration;

const MAX_MESSAGE_BYTES: usize = 5 * 1024 * 1024;

struct Sender {
    endpoint_url: String,
    token_secret: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct ApiResponse {
    success: bool,
    result: Option<ProviderReceipt>,
}

/// Cloudflare's `result` as received. Unlike the stored [`SendReceipt`], it
/// ignores fields afmail does not read: a provider adding one must not turn an
/// accepted submission into an unknown outcome.
#[derive(Deserialize)]
struct ProviderReceipt {
    message_id: String,
    delivered: Vec<String>,
    queued: Vec<String>,
    permanent_bounces: Vec<String>,
    #[serde(default)]
    suppressed_recipients: Vec<String>,
}

impl From<ProviderReceipt> for SendReceipt {
    fn from(receipt: ProviderReceipt) -> Self {
        Self {
            message_id: receipt.message_id,
            delivered: receipt.delivered,
            queued: receipt.queued,
            permanent_bounces: receipt.permanent_bounces,
            suppressed_recipients: receipt.suppressed_recipients,
        }
    }
}

/// Mailbox comparison key. A provider may re-spell case in either part, so
/// both sides compare lowercased; the receipt keeps the provider's spelling.
fn recipient_key(address: &str) -> String {
    address.to_lowercase()
}

fn local_error(code: &'static str, message: &str) -> AppError {
    AppError::new(code, message).with_details(serde_json::json!({"remote_calls_started": false}))
}

impl Sender {
    fn new(config: &MailConfig) -> Result<Self> {
        let account_id = config.cloudflare.account_id.as_deref().ok_or_else(|| {
            local_error(
                "config_missing",
                "cloudflare.account_id is required for sending",
            )
        })?;
        if account_id.len() != 32 || !account_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(local_error(
                "config_invalid",
                "cloudflare.account_id must contain 32 hexadecimal characters for sending",
            ));
        }
        let token_secret = resolve_password_secret(
            "cloudflare.sending.api_token_secret",
            config.cloudflare.sending.api_token_secret.as_deref(),
        )
        .map_err(|error| error.with_details(serde_json::json!({"remote_calls_started": false})))?;
        if token_secret.is_empty() || !token_secret.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(local_error(
                "config_invalid",
                "cloudflare.sending.api_token_secret must be a nonempty API token",
            ));
        }
        Ok(Self {
            endpoint_url: format!(
                "https://api.cloudflare.com/client/v4/accounts/{account_id}/email/sending/send_raw"
            ),
            token_secret,
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_global(Some(Duration::from_secs(60)))
                .build()
                .into(),
        })
    }

    fn send(&self, prepared: &PreparedOutbound) -> Result<SendReceipt> {
        let request = request_body(prepared)?;
        let failed = |status: Option<u16>| {
            AppError::new(
                "cloudflare_send_failed",
                "Cloudflare email submission outcome could not be established",
            )
            .with_details(serde_json::json!({"http_status": status}))
        };
        // A POST is never replayed here. A lost response may follow an accepted send.
        let mut response = self
            .agent
            .post(&self.endpoint_url)
            .header("Authorization", format!("Bearer {}", self.token_secret))
            .header("Content-Type", "application/json")
            .send(request.as_slice())
            .map_err(|_| failed(None))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_string()
            .map_err(|_| failed(Some(status)))?;
        // Never include server prose or request bytes in errors: they can echo credentials.
        let api: ApiResponse = serde_json::from_str(&body).map_err(|_| failed(Some(status)))?;
        if !api.success
            && api.result.is_none()
            && matches!(status, 400 | 401 | 403 | 404 | 413 | 422 | 429)
        {
            return Err(AppError::new(
                "cloudflare_send_rejected",
                "Cloudflare rejected the email submission",
            )
            .with_details(serde_json::json!({
                "http_status": status,
                "remote_effect_rejected": true
            })));
        }
        if !(200..300).contains(&status) || !api.success {
            return Err(failed(Some(status)));
        }
        let receipt = SendReceipt::from(api.result.ok_or_else(|| failed(Some(status)))?);
        let mut reported = BTreeSet::new();
        for recipient in receipt
            .delivered
            .iter()
            .chain(&receipt.queued)
            .chain(&receipt.permanent_bounces)
            .chain(&receipt.suppressed_recipients)
        {
            if !reported.insert(recipient_key(recipient)) {
                return Err(failed(Some(status)));
            }
        }
        let expected = prepared
            .email()?
            .envelope_to
            .iter()
            .map(|address| recipient_key(address))
            .collect();
        if receipt.message_id.trim().is_empty() || reported != expected {
            return Err(failed(Some(status)));
        }
        Ok(receipt)
    }
}

fn request_body(prepared: &PreparedOutbound) -> Result<Vec<u8>> {
    let prepared = prepared.email()?;
    if prepared.delivery_raw.len() > MAX_MESSAGE_BYTES {
        return Err(local_error(
            "message_too_large",
            "Cloudflare sending limit is 5 MiB",
        ));
    }
    if prepared.envelope_to.is_empty() || prepared.envelope_to.len() > 50 {
        return Err(local_error(
            "draft_invalid",
            "Cloudflare sending requires between 1 and 50 envelope recipients",
        ));
    }
    for address in std::iter::once(&prepared.envelope_from).chain(&prepared.envelope_to) {
        address
            .parse::<lettre::address::Address>()
            .map_err(|_| local_error("draft_invalid", "invalid Cloudflare envelope address"))?;
    }
    let raw = std::str::from_utf8(&prepared.delivery_raw)
        .map_err(|_| local_error("draft_invalid", "Cloudflare raw MIME must be valid UTF-8"))?;
    serde_json::to_vec(&serde_json::json!({
        "from": prepared.envelope_from,
        "recipients": prepared.envelope_to,
        "mime_message": raw
    }))
    .map_err(|_| local_error("draft_invalid", "cannot serialize Cloudflare raw MIME"))
}

pub fn prepare_send(config: &MailConfig, prepared: &PreparedOutbound) -> Result<()> {
    Sender::new(config)?;
    request_body(prepared)?;
    Ok(())
}

pub fn send_raw_message(config: &MailConfig, prepared: &PreparedOutbound) -> Result<SendReceipt> {
    Sender::new(config)?.send(prepared)
}

/// Validate local account and token configuration without submitting an email.
pub(crate) fn check_config(config: &MailConfig) -> Result<serde_json::Value> {
    Sender::new(config)?;
    Ok(serde_json::json!({
        "status": "not_tested", "config_valid": true,
        "permissions_verified": false, "message_sent": false
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Read, Write};

    fn config() -> MailConfig {
        let mut config = MailConfig::default();
        config.cloudflare.account_id = Some("0123456789abcdef0123456789abcdef".into());
        config.cloudflare.sending.api_token_secret = Some("test-token".into());
        config
    }

    fn prepared() -> PreparedOutbound {
        PreparedOutbound {
            message_id: "<local@example.com>".into(),
            raw: b"To: visible@example.com\r\nBcc: hidden@example.com\r\n\r\nbody\r\n".to_vec(),
            delivery: crate::types::OutboundDelivery::Email(crate::types::PreparedEmail {
                delivery_raw: b"To: visible@example.com\r\n\r\nbody\r\n".to_vec(),
                envelope_from: "sender@example.com".into(),
                envelope_to: vec!["visible@example.com".into(), "hidden@example.com".into()],
            }),
        }
    }

    fn response() -> Value {
        json!({"success":true,"result":{
            "message_id":"<provider@example.com>",
            "delivered":["visible@example.com"],"queued":["hidden@example.com"],
            "permanent_bounces":[],"suppressed_recipients":[]
        }})
    }

    fn exchange(status: u16, body: Option<String>) -> (Result<SendReceipt>, Value) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = Sender::new(&config()).unwrap();
        sender.endpoint_url = format!("http://{}/send_raw", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, "POST /send_raw HTTP/1.1\r\n");
            let mut length = None;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut request = vec![0; length.unwrap()];
            reader.read_exact(&mut request).unwrap();
            if let Some(body) = body {
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            serde_json::from_slice(&request).unwrap()
        });
        let result = sender.send(&prepared());
        (result, server.join().unwrap())
    }

    #[test]
    fn raw_submission_preserves_envelope_bcc_and_recipient_receipt() {
        let (result, request) = exchange(200, Some(response().to_string()));
        let receipt = result.unwrap();
        assert_eq!(request["from"], "sender@example.com");
        assert_eq!(
            request["recipients"],
            json!(["visible@example.com", "hidden@example.com"])
        );
        assert_eq!(
            request["mime_message"],
            "To: visible@example.com\r\n\r\nbody\r\n"
        );
        assert_eq!(receipt.message_id, "<provider@example.com>");
        assert_eq!(receipt.queued, ["hidden@example.com"]);
    }

    #[test]
    fn partial_recipient_failure_is_preserved() {
        for field in ["permanent_bounces", "suppressed_recipients"] {
            let mut body = response();
            body["result"]["queued"] = json!([]);
            body["result"][field] = json!(["hidden@example.com"]);
            let (result, _) = exchange(200, Some(body.to_string()));
            assert_eq!(
                serde_json::to_value(result.unwrap()).unwrap()[field],
                json!(["hidden@example.com"])
            );
        }
    }

    #[test]
    fn only_explicit_client_rejections_are_safe_to_retry() {
        for status in [400, 401, 403, 429, 500, 503] {
            let (result, _) = exchange(
                status,
                Some(
                    json!({
                        "success":false,"result":null,"errors":[{"message":"test-token echoed"}]
                    })
                    .to_string(),
                ),
            );
            let error = result.unwrap_err();
            assert!(!format!("{error:?}").contains("test-token"));
            assert_eq!(
                error.details.unwrap().get("remote_effect_rejected"),
                if status < 500 {
                    Some(&json!(true))
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn lost_or_unverifiable_receipts_never_prove_rejection() {
        let mut missing = response();
        missing["result"]["queued"] = json!([]);
        let mut duplicate = response();
        duplicate["result"]["queued"] = json!(["visible@example.com", "hidden@example.com"]);
        for body in [
            None,
            Some("bad JSON test-token".into()),
            Some(missing.to_string()),
            Some(duplicate.to_string()),
        ] {
            let (result, _) = exchange(200, body);
            let error = result.unwrap_err();
            assert_eq!(error.error_code, "cloudflare_send_failed");
            assert!(
                error
                    .details
                    .unwrap()
                    .get("remote_effect_rejected")
                    .is_none()
            );
        }
    }

    #[test]
    fn unknown_provider_fields_do_not_hide_an_accepted_send() {
        let mut body = response();
        body["result"]["provider_trace"] = json!({"region":"test"});
        body["messages"] = json!([]);
        let (result, _) = exchange(200, Some(body.to_string()));
        assert_eq!(result.unwrap().message_id, "<provider@example.com>");
    }

    #[test]
    fn recipient_case_respelling_is_still_the_same_set() {
        let mut body = response();
        body["result"]["delivered"] = json!(["Visible@Example.COM"]);
        body["result"]["queued"] = json!(["HIDDEN@example.com"]);
        let (result, _) = exchange(200, Some(body.to_string()));
        // The receipt keeps the provider's spelling.
        assert_eq!(result.unwrap().delivered, ["Visible@Example.COM"]);
    }

    #[test]
    fn a_different_or_case_duplicated_recipient_set_is_unverifiable() {
        let mut other = response();
        other["result"]["queued"] = json!(["other@example.com"]);
        let mut duplicate = response();
        duplicate["result"]["queued"] = json!(["hidden@example.com", "VISIBLE@example.com"]);
        for body in [other, duplicate] {
            let (result, _) = exchange(200, Some(body.to_string()));
            assert_eq!(result.unwrap_err().error_code, "cloudflare_send_failed");
        }
    }

    #[test]
    fn local_errors_fail_before_any_submission() {
        let mut invalid = config();
        invalid.cloudflare.account_id = Some("../../other-account".into());
        assert!(prepare_send(&invalid, &prepared()).is_err());
        invalid = config();
        invalid.cloudflare.sending.api_token_secret = Some("bad\r\nheader".into());
        assert!(prepare_send(&invalid, &prepared()).is_err());
        let mut message = prepared();
        if let crate::types::OutboundDelivery::Email(email) = &mut message.delivery {
            email.delivery_raw = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        }
        assert_eq!(
            prepare_send(&config(), &message).unwrap_err().error_code,
            "message_too_large"
        );
        message = prepared();
        if let crate::types::OutboundDelivery::Email(email) = &mut message.delivery {
            email.envelope_to = vec!["visible@example.com".into(); 51];
        }
        assert_eq!(
            prepare_send(&config(), &message)
                .unwrap_err()
                .details
                .unwrap()["remote_calls_started"],
            false
        );
    }
}
