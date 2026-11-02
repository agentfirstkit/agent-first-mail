use crate::config::{MailConfig, resolve_password_secret};
use crate::error::{AppError, Result};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

pub(super) struct QueueClient {
    endpoint: String,
    token: String,
    agent: ureq::Agent,
}
#[derive(Deserialize)]
pub(super) struct Message {
    pub body: Value,
    pub lease_id: String,
}
#[derive(Deserialize)]
pub(super) struct Event {
    pub account: String,
    pub action: String,
    pub bucket: String,
    pub object: EventObject,
}
#[derive(Deserialize)]
pub(super) struct EventObject {
    pub key: String,
    pub size: u64,
}
fn failed(operation: &str) -> AppError {
    AppError::retryable(
        "cloudflare_queue_failed",
        format!("Cloudflare Queue {operation} failed"),
    )
}
impl QueueClient {
    pub fn new(config: &MailConfig) -> Result<Self> {
        let source = &config.cloudflare;
        let account = source.account_id.as_deref().filter(|s| valid_id(s)).ok_or_else(|| AppError::new(
            "config_missing", "cloudflare.account_id must be a 32-character hexadecimal account ID for Queue consumption",
        ))?;
        let queue = source
            .queue
            .queue_id
            .as_deref()
            .filter(|s| valid_id(s))
            .ok_or_else(|| {
                AppError::new(
                    "config_missing",
                    "cloudflare.queue.queue_id must be a 32-character hexadecimal queue ID",
                )
            })?;
        let url: url::Url = source.queue.endpoint_url.parse().map_err(|_| {
            AppError::new("config_invalid", "invalid cloudflare.queue.endpoint_url")
        })?;
        if (url.scheme() != "https"
            && !(url.scheme() == "http"
                && url.host_str().is_some_and(crate::util::is_loopback_host)))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(AppError::new(
                "config_invalid",
                "cloudflare.queue.endpoint_url requires HTTPS without credentials, query, or fragment (HTTP is allowed on loopback)",
            ));
        }
        let token = resolve_password_secret(
            "cloudflare.queue.api_token_secret",
            source.queue.api_token_secret.as_deref(),
        )?;
        Ok(Self {
            endpoint: format!(
                "{}/accounts/{account}/queues/{queue}/messages",
                source.queue.endpoint_url.trim_end_matches('/')
            ),
            token,
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .into(),
        })
    }
    fn post(&self, action: &str, body: Value) -> Result<Value> {
        let bytes = serde_json::to_vec(&body).map_err(|_| failed("serialize request"))?;
        let mut response = self
            .agent
            .post(format!("{}/{action}", self.endpoint))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .send(bytes.as_slice())
            .map_err(|_| failed(action).with_details(json!({"stage":"transport"})))?;
        if !response.status().is_success() {
            return Err(failed(action).with_details(
                json!({"stage":"http_status", "http_status":response.status().as_u16()}),
            ));
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024)
            .read_to_vec()
            .map_err(|_| failed(action).with_details(json!({"stage":"read_body"})))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| failed(action).with_details(json!({"stage":"decode_body"})))?;
        if value["success"] != true {
            return Err(failed(action).with_details(json!({"stage":"api_result"})));
        }
        Ok(value)
    }
    pub fn pull(&self, config: &MailConfig) -> Result<Vec<Message>> {
        let value = self.post(
            "pull",
            json!({
                "batch_size": config.cloudflare.queue.batch_size,
                "visibility_timeout_ms": config.cloudflare.queue.visibility_timeout_s * 1000,
            }),
        )?;
        let messages: Vec<Message> = serde_json::from_value(value["result"]["messages"].clone())
            .map_err(|_| failed("decode"))?;
        if messages.len() > 100 || messages.iter().any(|message| message.lease_id.is_empty()) {
            return Err(failed("decode"));
        }
        Ok(messages)
    }
    pub fn settle(&self, acks: &[String], retries: &[String], retry_delay_s: u64) -> Result<()> {
        self.post("ack", json!({
            "acks": acks.iter().map(|id| json!({"lease_id":id})).collect::<Vec<_>>(),
            "retries": retries.iter().map(|id| json!({"lease_id":id,"delay_seconds":retry_delay_s})).collect::<Vec<_>>(),
        }))?;
        Ok(())
    }
}
fn valid_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
impl Message {
    pub fn event(&self) -> Result<Event> {
        let value = if let Some(text) = self.body.as_str() {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(text)
                .unwrap_or_else(|_| text.as_bytes().to_vec());
            serde_json::from_slice(&bytes).map_err(|_| failed("decode event"))?
        } else {
            self.body.clone()
        };
        serde_json::from_value(value).map_err(|_| failed("decode event"))
    }
}
