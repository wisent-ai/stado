//! Vast host REST transport: [`VastClient`], its Bearer-authenticated
//! `request` helper and the [`VAST_BASE`] endpoint.
//!
//! Moved verbatim out of the former single-file `providers/vast`. The
//! Skarbiec credential lookup sits in the `credentials` component, the
//! machine-id resolution in `machine`, the marketplace offer operations in
//! `offers`, and the Python-shaped JSON readers those two share in `json`.

mod credentials;
pub(super) mod json;
mod machine;
mod offers;

use std::sync::Arc;

use serde_json::Value;

use super::VastError;

pub use credentials::{
    read_vast_api_key, vast_api_key_available, VastCredentialChannel, VastCredentialReading,
};
pub use machine::{parse_machine_id_env, system_hostname};
pub use offers::ListMachineParams;

/// Python `_VAST_BASE`.
pub const VAST_BASE: &str = "https://console.vast.ai/api/v0";

/// Bearer-authenticated REST client against the Vast host API (Python
/// module-level `_request` + the public operations). Cheap to clone.
#[derive(Debug, Clone)]
pub struct VastClient {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl VastClient {
    /// Bind the client to the public Vast API with an explicit key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, VAST_BASE)
    }

    /// Bind with a custom base URL (loopback mocks in tests).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        VastClient {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                api_key: api_key.into(),
                base_url: base_url.into(),
            }),
        }
    }

    /// Resolve the key from `stado-vast/api_key` in Skarbiec. The refusal
    /// names the channel that answered and what it said, because an empty
    /// key has three different causes and the operator needs the one in
    /// front of them.
    pub async fn from_env() -> Result<Self, VastError> {
        let reading = read_vast_api_key().await;
        match reading.key.clone() {
            Some(key) => Ok(Self::new(key)),
            None => Err(VastError::config(reading.refusal())),
        }
    }

    /// Python `_request`: Bearer-authenticated call; HTTP error statuses
    /// become a RuntimeError-style message with the body head (280 chars).
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, VastError> {
        let verb = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| VastError::config(format!("invalid HTTP method {method:?}")))?;
        let mut request = self
            .inner
            .http
            .request(verb, format!("{}{path}", self.inner.base_url))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.inner.api_key),
            )
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = crate::wait::request(request).await?;
        let status = response.status();
        let text = response.text().await.map_err(|err| VastError::Api {
            status: None,
            detail: format!(
                "Vast.ai {method} {path} -> HTTP {}, and its body could not be read: {err}",
                status.as_u16()
            ),
        })?;
        if !status.is_success() {
            return Err(VastError::Api {
                status: Some(status.as_u16()),
                detail: format!(
                    "Vast.ai {method} {path} -> HTTP {}: {text}",
                    status.as_u16()
                ),
            });
        }
        // A success with no body is an answer with nothing in it, kept as
        // null rather than read as an empty object nobody sent.
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|err| VastError::Api {
            status: None,
            detail: format!("Vast.ai {method} {path} -> invalid JSON: {err}"),
        })
    }
}
