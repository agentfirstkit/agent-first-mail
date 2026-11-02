use crate::config::{CloudflareSection, resolve_password_secret};
use crate::error::{AppError, Result};
use crate::ingest::MAX_MESSAGE_BYTES;
use crate::types::{DeliveryEnvelope, DeliverySource};
use rusty_s3::actions::ListObjectsV2;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use std::io::Read;
use std::time::Duration;

const SIGNATURE_TTL: Duration = Duration::from_secs(60);

pub(crate) struct R2Client {
    bucket: Bucket,
    credentials: Credentials,
    agent: ureq::Agent,
    control: Option<(String, String)>,
}
pub(crate) struct Object {
    pub key: String,
    pub size_bytes: u64,
}
pub(crate) struct Page {
    pub objects: Vec<Object>,
    pub continuation: Option<String>,
}
pub(crate) struct Download {
    pub raw: Vec<u8>,
    pub delivery: DeliveryEnvelope,
    pub quota_managed: bool,
}

fn failed(operation: &str, key: Option<&str>, status: Option<u16>) -> AppError {
    AppError::new("cloudflare_r2_failed", format!("R2 {operation} failed"))
        .with_details(serde_json::json!({"operation":operation,"key":key,"http_status":status}))
}
impl R2Client {
    pub(crate) fn new(config: &CloudflareSection) -> Result<Self> {
        let name = config
            .r2
            .bucket
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| AppError::new("config_missing", "cloudflare.r2.bucket is required"))?;
        let endpoint = config.r2.endpoint_url.clone().unwrap_or_else(|| {
            format!(
                "https://{}.r2.cloudflarestorage.com",
                config.account_id.as_deref().unwrap_or_default()
            )
        });
        let endpoint = endpoint.parse().map_err(|_| {
            AppError::new(
                "config_invalid",
                "cloudflare.r2.endpoint_url must be an HTTP(S) URL",
            )
        })?;
        let bucket =
            Bucket::new(endpoint, UrlStyle::Path, name.to_string(), "auto").map_err(|_| {
                AppError::new("config_invalid", "invalid cloudflare.r2 endpoint or bucket")
            })?;
        let key = config
            .r2
            .access_key_id
            .as_deref()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                AppError::new("config_missing", "cloudflare.r2.access_key_id is required")
            })?;
        let secret = resolve_password_secret(
            "cloudflare.r2.secret_access_key_secret",
            config.r2.secret_access_key_secret.as_deref(),
        )?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        let control = config.spool.control_url.as_ref().map(|value| {
            let url: url::Url = value.parse().map_err(|_| AppError::new("config_invalid", "cloudflare.spool.control_url must be an HTTPS URL"))?;
            if (url.scheme() != "https" && !(url.scheme() == "http" && url.host_str().is_some_and(crate::util::is_loopback_host)))
                || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
                return Err(AppError::new("config_invalid", "cloudflare.spool.control_url requires HTTPS without credentials, query, or fragment (HTTP is allowed on loopback)"));
            }
            let token = resolve_password_secret("cloudflare.spool.token_secret", config.spool.token_secret.as_deref())?;
            if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
                return Err(AppError::new("config_invalid", "cloudflare.spool.token_secret must be a token of at least 32 ASCII characters"));
            }
            Ok((value.trim_end_matches('/').to_string(), token))
        }).transpose()?;
        Ok(Self {
            bucket,
            credentials: Credentials::new(key, secret),
            agent,
            control,
        })
    }
    fn check(
        &self,
        operation: &str,
        key: Option<&str>,
        response: &ureq::http::Response<ureq::Body>,
    ) -> Result<()> {
        if response.status().is_success() {
            Ok(())
        } else {
            Err(failed(operation, key, Some(response.status().as_u16())))
        }
    }
    pub(crate) fn list(
        &self,
        prefix: &str,
        continuation: Option<&str>,
        max_keys: u16,
    ) -> Result<Page> {
        let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
        // Request raw XML keys: some S3 implementations encode spaces as `+`
        // with encoding-type=url, while the signing library decodes only %XX.
        action.query_mut().remove("encoding-type");
        action.with_prefix(prefix);
        action.with_max_keys(usize::from(max_keys));
        if let Some(token) = continuation {
            action.with_continuation_token(token);
        }
        let mut response = self
            .agent
            .get(action.sign(SIGNATURE_TTL).as_str())
            .call()
            .map_err(|_| failed("list", None, None))?;
        self.check("list", None, &response)?;
        let body = response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .map_err(|_| failed("list", None, None))?;
        let page = ListObjectsV2::parse_response(&body).map_err(|_| failed("list", None, None))?;
        Ok(Page {
            objects: page
                .contents
                .into_iter()
                .map(|object| {
                    let size_bytes = object
                        .size
                        .ok_or_else(|| failed("list", Some(&object.key), None))?;
                    Ok(Object {
                        key: object.key,
                        size_bytes,
                    })
                })
                .collect::<Result<_>>()?,
            continuation: page.next_continuation_token,
        })
    }
    pub(crate) fn get(&self, key: &str) -> Result<Download> {
        let url = self
            .bucket
            .get_object(Some(&self.credentials), key)
            .sign(SIGNATURE_TTL);
        let mut response = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|_| failed("get", Some(key), None))?;
        self.check("get", Some(key), &response)?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string)
        };
        let quota_managed = header("x-amz-meta-quota_managed").as_deref() == Some("true");
        let delivery = DeliveryEnvelope {
            envelope_from: header("x-amz-meta-envelope_from"),
            envelope_to: header("x-amz-meta-envelope_to").into_iter().collect(),
            received_rfc3339: header("x-amz-meta-received_rfc3339"),
            sources: vec![DeliverySource::CloudflareR2 {
                bucket: self.bucket.name().into(),
                key: key.into(),
            }],
        };
        let mut raw = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_MESSAGE_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|_| failed("get", Some(key), None))?;
        if raw.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(AppError::new(
                "message_too_large",
                "R2 message exceeds the 100 MiB limit",
            ));
        }
        Ok(Download {
            raw,
            delivery,
            quota_managed,
        })
    }
    pub(crate) fn acknowledge(
        &self,
        key: &str,
        archive_key: Option<&str>,
        quota_managed: bool,
    ) -> Result<()> {
        if let Some((url, token)) = &self.control {
            let body =
                serde_json::to_vec(&serde_json::json!({"key": key, "archive_key": archive_key}))
                    .map_err(|error| AppError::json("serialize spool acknowledgement", &error))?;
            // Omit absent optional keys; the control API distinguishes absent
            // archive intent from a malformed archive destination.
            let body = if archive_key.is_some() {
                body
            } else {
                serde_json::to_vec(&serde_json::json!({"key": key}))
                    .map_err(|error| AppError::json("serialize spool acknowledgement", &error))?
            };
            let mut response = self
                .agent
                .post(format!("{url}/v1/ack"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .send(body.as_slice())
                .map_err(|_| failed("quota_acknowledge", Some(key), None))?;
            self.check("quota_acknowledge", Some(key), &response)?;
            let text = response
                .body_mut()
                .with_config()
                .limit(65536)
                .read_to_string()
                .map_err(|_| failed("quota_acknowledge", Some(key), None))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|_| failed("quota_acknowledge", Some(key), None))?;
            if value
                .pointer("/result/acknowledged")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                return Err(failed("quota_acknowledge", Some(key), None));
            }
            return Ok(());
        }
        if quota_managed {
            return Err(AppError::new(
                "config_missing",
                "quota-managed objects require cloudflare.spool.control_url and cloudflare.spool.token_secret for acknowledgement",
            ));
        }
        if let Some(target) = archive_key {
            self.copy(key, target)?;
        }
        self.delete(key)
    }
    pub(crate) fn delete(&self, key: &str) -> Result<()> {
        let url = self
            .bucket
            .delete_object(Some(&self.credentials), key)
            .sign(SIGNATURE_TTL);
        let response = self
            .agent
            .delete(url.as_str())
            .call()
            .map_err(|_| failed("delete", Some(key), None))?;
        self.check("delete", Some(key), &response)
    }
    pub(crate) fn copy(&self, from: &str, to: &str) -> Result<()> {
        let source = self.bucket.object_url(from).path().to_string();
        let mut action = self.bucket.put_object(Some(&self.credentials), to);
        action
            .headers_mut()
            .insert("x-amz-copy-source", source.as_str());
        let mut response = self
            .agent
            .put(action.sign(SIGNATURE_TTL).as_str())
            .header("x-amz-copy-source", &source)
            .send_empty()
            .map_err(|_| failed("copy", Some(from), None))?;
        self.check("copy", Some(from), &response)?;
        let body = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_string()
            .map_err(|_| failed("copy", Some(from), None))?;
        if body.contains("<Error") || !body.contains("CopyObjectResult") {
            return Err(failed("copy", Some(from), Some(response.status().as_u16())));
        }
        Ok(())
    }
}
