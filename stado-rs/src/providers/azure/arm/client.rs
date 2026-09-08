//! The bearer-authenticated ARM client itself: construction, raw request
//! dispatch, non-2xx lifting, and long-running-operation polling. The REST
//! verbs layered on these primitives live in the sibling `verbs` module.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{bearer_token, AzureError, ARM_API_BASE};

// --- ARM REST client ---

/// Bearer-authenticated ARM REST client. Cheap to clone.
#[derive(Clone)]
pub struct ArmClient {
    inner: Arc<ArmInner>,
}

struct ArmInner {
    http: reqwest::Client,
    subscription: String,
    base_url: String,
    /// True in prod (token chain attached); false on loopback test mocks.
    auth: bool,
    /// Delay between LRO polls (near-zero in tests).
    poll_interval: Duration,
}

impl ArmClient {
    /// Bind to the public ARM API; the token chain resolves on the first
    /// request.
    pub fn new(subscription: &str) -> Self {
        Self::assemble(subscription, ARM_API_BASE, true, Duration::from_secs(2))
    }

    /// Bind to an explicit base URL without auth (loopback mocks in
    /// tests) and with a near-zero LRO poll interval.
    fn assemble(subscription: &str, base_url: &str, auth: bool, poll_interval: Duration) -> Self {
        ArmClient {
            inner: Arc::new(ArmInner {
                http: reqwest::Client::new(),
                subscription: subscription.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
                poll_interval,
            }),
        }
    }

    /// The subscription this client is bound to (Python
    /// `self.subscription`).
    pub fn subscription(&self) -> &str {
        &self.inner.subscription
    }

    /// Send one request; the raw response is returned unchecked so
    /// callers can apply their own status handling (404 allowances, LRO
    /// headers). `url` may be a path under the API base or an absolute
    /// URL (LRO poll targets).
    pub(super) async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, AzureError> {
        let full = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{url}", self.inner.base_url)
        };
        let mut request = self
            .inner
            .http
            .request(method, full)
            .header(reqwest::header::ACCEPT, "application/json");
        if self.inner.auth {
            let token = bearer_token(&self.inner.http).await?;
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(body).unwrap_or_else(|_| "{}".into()));
        }
        Ok(request.send().await?)
    }

    /// Lift a non-2xx response into [`AzureError::Api`], embedding the
    /// ARM `error.code` + `error.message` so Python's substring
    /// classification keeps working.
    pub(super) async fn api_error(response: reqwest::Response, desc: &str) -> AzureError {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        let error = parsed.get("error").cloned().unwrap_or(Value::Null);
        let code = error.get("code").and_then(Value::as_str).unwrap_or("");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        let detail = if code.is_empty() && message.is_empty() {
            text.chars().take(280).collect::<String>()
        } else {
            format!("{code} {message}").trim().to_string()
        };
        AzureError::Api(format!("Azure {desc} -> HTTP {status}: {detail}"))
    }

    /// Poll an Azure-AsyncOperation URL until Succeeded/Failed/Canceled.
    pub(super) async fn poll_async_operation(
        &self,
        url: &str,
        desc: &str,
    ) -> Result<(), AzureError> {
        loop {
            let body = self.get(url, &format!("poll {desc}")).await?;
            let status = body.get("status").and_then(Value::as_str).unwrap_or("");
            match status {
                "Succeeded" => return Ok(()),
                "Failed" | "Canceled" => {
                    let error = body.get("error").cloned().unwrap_or(Value::Null);
                    let code = error.get("code").and_then(Value::as_str).unwrap_or("");
                    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
                    return Err(AzureError::Api(format!(
                        "Azure {desc} operation {status}: {}",
                        format!("{code} {message}").trim()
                    )));
                }
                _ => tokio::time::sleep(self.inner.poll_interval).await,
            }
        }
    }

    /// Poll a Location header URL until it stops returning 202.
    pub(super) async fn poll_location(&self, url: &str, desc: &str) -> Result<(), AzureError> {
        loop {
            let response = self.send(reqwest::Method::GET, url, None).await?;
            if response.status() == reqwest::StatusCode::ACCEPTED {
                tokio::time::sleep(self.inner.poll_interval).await;
                continue;
            }
            if !response.status().is_success() {
                return Err(Self::api_error(response, desc).await);
            }
            return Ok(());
        }
    }
}
