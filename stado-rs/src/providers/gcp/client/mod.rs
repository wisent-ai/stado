//! GCE REST v1 transport: [`GceClient`], its request helpers, and the
//! [`GceError`] classification the provider matches substrings on.
//!
//! Moved verbatim out of the former single-file `providers/gcp`. The
//! instance-scoped reads (`instance_status`, `aggregated_instances`) sit in
//! the `instances` component.

mod instances;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

/// GCE REST API v1 base.
pub const COMPUTE_API_BASE: &str = "https://compute.googleapis.com/compute/v1";
/// OAuth scope matching the Python google-cloud-compute client.
pub(in crate::providers::gcp) const CLOUD_PLATFORM_SCOPE: &str =
    "https://www.googleapis.com/auth/cloud-platform";

/// GCE transport/API error. The `Api` message embeds the error codes
/// (`error.errors[].reason` for synchronous failures, the LRO
/// `error.errors[].code` for operation failures) plus the API message text
/// so the Python substring classification ("QUOTA_EXCEEDED",
/// "ZONE_RESOURCE_POOL_EXHAUSTED", "STOCKOUT", "already exists") works on
/// `error.to_string()` exactly like it did on `str(exc)` from the SDK.
#[derive(Debug, thiserror::Error)]
pub enum GceError {
    /// Python: ADC lookup failure at client construction.
    #[error("no GCP credentials found for the GCE compute API: {0}")]
    Auth(String),
    /// Transport failure (Python: SSL EOF / RetryError from the SDK).
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Non-2xx response or failed LRO; message carries codes + body text.
    #[error("{0}")]
    Api(String),
}

/// Bearer-authenticated GCE REST v1 client. Cheap to clone.
#[derive(Clone)]
pub struct GceClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    project: String,
    base_url: String,
    auth: Option<Arc<dyn gcp_auth::TokenProvider>>,
    /// Delay between LRO polls. Python `op.result()` uses the SDK default
    /// (~1s); tests shrink this to milliseconds.
    poll_interval: Duration,
}

impl GceClient {
    /// Bind the client to the public GCE API, resolving GCP credentials
    /// (cloud-platform scope). No credentials is a hard error (same as the
    /// Python SDK client construction).
    pub async fn new(project: &str) -> Result<Self, GceError> {
        let auth = crate::skarbiec::gcp_provider()
            .await
            .map_err(|err| GceError::Auth(err.to_string()))?;
        Ok(Self::assemble(
            project,
            COMPUTE_API_BASE,
            Some(auth),
            Duration::from_secs(1),
        ))
    }

    /// Bind to an explicit base URL without credentials (loopback mocks in
    /// tests) and with a near-zero LRO poll interval.
    fn assemble(
        project: &str,
        base_url: &str,
        auth: Option<Arc<dyn gcp_auth::TokenProvider>>,
        poll_interval: Duration,
    ) -> Self {
        GceClient {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                project: project.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
                poll_interval,
            }),
        }
    }

    /// The project this client is bound to (Python `self.project`).
    pub fn project(&self) -> &str {
        &self.inner.project
    }

    /// Fresh (cached by gcp_auth until expiry) bearer token; None in tests.
    async fn token(&self) -> Result<Option<String>, GceError> {
        let Some(auth) = &self.inner.auth else {
            return Ok(None);
        };
        let token = auth
            .token(&[CLOUD_PLATFORM_SCOPE])
            .await
            .map_err(|err| GceError::Auth(err.to_string()))?;
        Ok(Some(format!("Bearer {}", token.as_str())))
    }

    /// Send one request; the raw response is returned unchecked so callers
    /// can apply their own status handling (404 allowances).
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, GceError> {
        let mut request = self
            .inner
            .http
            .request(method, format!("{}{path}", self.inner.base_url))
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = self.token().await? {
            request = request.header(reqwest::header::AUTHORIZATION, token);
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(body).unwrap_or_else(|_| "{}".into()));
        }
        Ok(request.send().await?)
    }

    /// Lift a non-2xx response into [`GceError::Api`], embedding the
    /// `error.errors[].reason|code` values and the message text so Python's
    /// substring classification keeps working.
    async fn api_error(response: reqwest::Response, desc: &str) -> GceError {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let error = parsed.get("error").cloned().unwrap_or(Value::Null);
        let mut codes = Vec::new();
        if let Some(entries) = error.get("errors").and_then(Value::as_array) {
            for entry in entries {
                // GCE uses `reason` on synchronous error bodies and `code`
                // on LRO error bodies; carry both forms.
                for key in ["reason", "code"] {
                    if let Some(code) = entry.get(key).and_then(Value::as_str) {
                        codes.push(code.to_string());
                        break;
                    }
                }
            }
        }
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        let detail = if message.is_empty() && codes.is_empty() {
            text.chars().take(280).collect::<String>()
        } else {
            format!("{} {message}", codes.join(" ")).trim().to_string()
        };
        GceError::Api(format!("GCE {desc} -> HTTP {status}: {detail}"))
    }

    /// GET a JSON resource; non-2xx is an [`GceError::Api`].
    pub async fn get(&self, path: &str, desc: &str) -> Result<Value, GceError> {
        let response = self.send(reqwest::Method::GET, path, None).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| GceError::Api(format!("GCE {desc} -> invalid JSON: {err}")))
    }

    /// GET that maps 404 to `None` (Python's `except NotFound`).
    pub async fn get_allow_404(&self, path: &str, desc: &str) -> Result<Option<Value>, GceError> {
        let response = self.send(reqwest::Method::GET, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| GceError::Api(format!("GCE {desc} -> invalid JSON: {err}")))
    }

    /// POST a JSON body, returning the parsed response (an Operation for
    /// instance inserts).
    pub async fn post(&self, path: &str, body: &Value, desc: &str) -> Result<Value, GceError> {
        let response = self.send(reqwest::Method::POST, path, Some(body)).await?;
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        let text = response.text().await.unwrap_or_default();
        serde_json::from_str(&text)
            .map_err(|err| GceError::Api(format!("GCE {desc} -> invalid JSON: {err}")))
    }

    /// DELETE a resource; `false` on 404 (idempotent NotFound). Does NOT
    /// wait for the returned operation — Python's `client.delete` call
    /// never invokes `op.result()` either.
    pub async fn delete_allow_404(&self, path: &str, desc: &str) -> Result<bool, GceError> {
        let response = self.send(reqwest::Method::DELETE, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::api_error(response, desc).await);
        }
        Ok(true)
    }

    /// Poll a zone operation until DONE (Python `op.result()`). When the
    /// operation completes with an error, the `error.errors[].code` values
    /// (e.g. QUOTA_EXCEEDED, ZONE_RESOURCE_POOL_EXHAUSTED) are surfaced in
    /// the [`GceError::Api`] message — this is where Python's substring
    /// classification reads them from.
    pub async fn wait_zone_operation(
        &self,
        zone: &str,
        operation: &str,
        desc: &str,
    ) -> Result<(), GceError> {
        let path = format!(
            "/projects/{}/zones/{zone}/operations/{operation}",
            self.project()
        );
        loop {
            let op = self
                .get(&path, &format!("get operation {operation}"))
                .await?;
            if op.get("status").and_then(Value::as_str) == Some("DONE") {
                if let Some(error) = op.get("error") {
                    let mut codes = Vec::new();
                    let mut messages = Vec::new();
                    if let Some(entries) = error.get("errors").and_then(Value::as_array) {
                        for entry in entries {
                            if let Some(code) = entry.get("code").and_then(Value::as_str) {
                                codes.push(code.to_string());
                            }
                            if let Some(message) = entry.get("message").and_then(Value::as_str) {
                                messages.push(message.to_string());
                            }
                        }
                    }
                    return Err(GceError::Api(format!(
                        "GCE {desc} operation {operation} failed: {} {}",
                        codes.join(" "),
                        messages.join("; ")
                    )));
                }
                return Ok(());
            }
            tokio::time::sleep(self.inner.poll_interval).await;
        }
    }
}
