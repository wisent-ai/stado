//! The bearer-authenticated ARM client itself: construction, raw request
//! dispatch, non-2xx lifting, and long-running-operation polling. The REST
//! verbs layered on these primitives live in the sibling `verbs` module.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{bearer_token, AzureError, ARM_API_BASE};

/// The status an Azure-AsyncOperation answer reports. Every status that is
/// not terminal is still running.
#[derive(serde::Deserialize)]
enum OperationStatus {
    Succeeded,
    Failed,
    Canceled,
    #[serde(other)]
    Running,
}

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
}

/// The wait an HTTP-date `Retry-After` names: the time left until `date`.
/// RFC 9110 §10.2.3 reads a date that has already passed as asking for the
/// next request now, so such a date names no wait.
fn until(date: chrono::DateTime<chrono::Utc>) -> Result<Duration, String> {
    let now = chrono::Utc::now();
    if date <= now {
        return Ok(Duration::ZERO);
    }
    (date - now)
        .to_std()
        .map_err(|cause| format!("the time until {date} is no wait: {cause}"))
}

/// The wait Azure names in a poll response's `Retry-After` header, which
/// RFC 9110 §10.2.3 states as a number of seconds or as an HTTP date. A
/// long-running operation that is still running and names no wait, or names
/// one in neither form, is an error naming the operation and the value,
/// never a wait this client invents.
fn retry_after(response: &reqwest::Response, desc: &str) -> Result<Duration, AzureError> {
    let still_running = || {
        format!(
            "Azure {desc} is still running and its poll answer (HTTP {})",
            response.status()
        )
    };
    let Some(header) = response.headers().get(reqwest::header::RETRY_AFTER) else {
        return Err(AzureError::Api(format!("{} names no Retry-After", still_running())));
    };
    let value = header.to_str().map_err(|cause| {
        AzureError::Api(format!(
            "{} names a Retry-After that is not text ({header:?}): {cause}",
            still_running()
        ))
    })?;
    let value = value.trim();
    match value.parse::<u64>() {
        Ok(seconds) => Ok(Duration::from_secs(seconds)),
        Err(seconds_cause) => match chrono::DateTime::parse_from_rfc2822(value) {
            Ok(date) => until(date.with_timezone(&chrono::Utc)).map_err(|cause| {
                AzureError::Api(format!("{} names Retry-After {value:?}: {cause}", still_running()))
            }),
            Err(date_cause) => Err(AzureError::Api(format!(
                "{} names Retry-After {value:?}, which is neither a number of seconds \
                 ({seconds_cause}) nor an HTTP date ({date_cause})",
                still_running()
            ))),
        },
    }
}

/// Hold the poll once for the wait Azure named before asking again: the
/// first tick of an interval set to that wait falls at its end. A wait of
/// nothing asks again at once.
async fn hold_for(wait: Duration) {
    if !wait.is_zero() {
        tokio::time::interval_at(tokio::time::Instant::now() + wait, wait)
            .tick()
            .await;
    }
}

/// The `code message` an ARM error object states, or `None` when the answer
/// holds no error object with either.
fn stated_error(answer: &Value) -> Option<String> {
    let error = answer.get("error")?;
    let code = error.get("code").and_then(Value::as_str);
    let message = error.get("message").and_then(Value::as_str);
    match (code, message) {
        (None, None) => None,
        (Some(code), None) => Some(code.to_string()),
        (None, Some(message)) => Some(message.to_string()),
        (Some(code), Some(message)) => Some(format!("{code} {message}")),
    }
}

impl ArmClient {
    /// Bind to the public ARM API; the token chain resolves on the first
    /// request.
    pub fn new(subscription: &str) -> Self {
        Self::assemble(subscription, ARM_API_BASE, true)
    }

    /// Bind to an explicit base URL without auth (loopback mocks in tests).
    fn assemble(subscription: &str, base_url: &str, auth: bool) -> Self {
        ArmClient {
            inner: Arc::new(ArmInner {
                http: reqwest::Client::new(),
                subscription: subscription.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
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
        // A body that cannot be serialized fails the request with its cause
        // instead of sending `{}` in its place.
        if let Some(body) = body {
            request = request.json(body);
        }
        Ok(crate::wait::request(request).await?)
    }

    /// Lift a non-2xx response into [`AzureError::Api`], embedding the
    /// ARM `error.code` + `error.message` so Python's substring
    /// classification keeps working. An answer that states no error object
    /// is quoted whole, and one that is not JSON says why beside it.
    pub(super) async fn api_error(response: reqwest::Response, desc: &str) -> AzureError {
        let status = response.status().as_u16();
        // A refusal whose body cannot be read says so: read as empty, the
        // operator saw `HTTP <status>:` with neither the provider's reason
        // nor why it is missing.
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                return AzureError::Api(format!(
                    "Azure {desc} -> HTTP {status}; its body could not be read: {}",
                    error.without_url()
                ))
            }
        };
        let detail = match serde_json::from_str::<Value>(&text) {
            Ok(answer) => match stated_error(&answer) {
                Some(stated) => stated,
                None => text,
            },
            Err(cause) => format!("{text} (the answer is not JSON: {cause})"),
        };
        AzureError::Api(format!("Azure {desc} -> HTTP {status}: {detail}"))
    }

    /// Poll an Azure-AsyncOperation URL until Succeeded/Failed/Canceled, at
    /// the interval each answer's `Retry-After` names. An answer that states
    /// no status is refused quoting it: read as running, it was polled for
    /// ever. A failed or canceled operation without an error object is
    /// reported by its whole answer.
    pub(super) async fn poll_async_operation(
        &self,
        url: &str,
        desc: &str,
    ) -> Result<(), AzureError> {
        loop {
            let response = self.send(reqwest::Method::GET, url, None).await?;
            if !response.status().is_success() {
                return Err(Self::api_error(response, &format!("poll {desc}")).await);
            }
            let wait = retry_after(&response, desc);
            let text = response.text().await?;
            let body: Value = serde_json::from_str(&text).map_err(|err| {
                AzureError::Api(format!("Azure poll {desc} -> invalid JSON: {err}"))
            })?;
            let Some(status) = body.get("status").and_then(Value::as_str) else {
                return Err(AzureError::Api(format!(
                    "Azure poll {desc} answered without a status: {text}"
                )));
            };
            let read = serde_json::from_value::<OperationStatus>(Value::String(status.to_string()))
                .map_err(|err| {
                    AzureError::Api(format!("Azure poll {desc} status {status:?} -> {err}"))
                })?;
            match read {
                OperationStatus::Succeeded => return Ok(()),
                OperationStatus::Failed | OperationStatus::Canceled => {
                    let detail = match stated_error(&body) {
                        Some(stated) => stated,
                        None => text,
                    };
                    return Err(AzureError::Api(format!(
                        "Azure {desc} operation {status}: {detail}"
                    )));
                }
                OperationStatus::Running => hold_for(wait?).await,
            }
        }
    }

    /// Poll a Location header URL until it stops returning 202, at the
    /// interval each `202` answer's `Retry-After` names.
    pub(super) async fn poll_location(&self, url: &str, desc: &str) -> Result<(), AzureError> {
        loop {
            let response = self.send(reqwest::Method::GET, url, None).await?;
            if response.status() == reqwest::StatusCode::ACCEPTED {
                hold_for(retry_after(&response, desc)?).await;
                continue;
            }
            if !response.status().is_success() {
                return Err(Self::api_error(response, desc).await);
            }
            return Ok(());
        }
    }
}
