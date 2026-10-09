//! The GCP REST executor: one authenticated client for the batch, and the
//! transport every typed method in this tree goes through.

use reqwest::Method;
use serde_json::{json, Value};

use crate::cli::CmdError;

mod inspect;
mod mutate;
mod paths;

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Clone)]
pub(super) struct GcpRest {
    http: reqwest::Client,
    token: String,
    project: String,
}

impl GcpRest {
    pub(in crate::cli::resources::executors) async fn new(project: &str) -> Result<Self, CmdError> {
        let auth = crate::skarbiec::gcp_provider().await.map_err(|error| {
            CmdError::click(format!("GCP authentication failed: {error}"))
                .stating(error.failure_code())
        })?;
        let token = auth.token(&[CLOUD_PLATFORM_SCOPE]).await.map_err(|error| {
            CmdError::click(format!("GCP token failed: {error}"))
                .stating(crate::primitives::failure::FailureCode::Auth)
        })?;
        let http = reqwest::Client::builder()
            .user_agent(format!(
                "stado/{} resource-operations",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;
        Ok(Self {
            http,
            token: token.as_str().to_string(),
            project: project.to_string(),
        })
    }

    async fn get_allow_404(&self, url: &str, description: &str) -> Result<Option<Value>, CmdError> {
        let response = crate::wait::request(self.http.get(url).bearer_auth(&self.token)).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_error(description, status, &text));
        }
        serde_json::from_str(&text).map(Some).map_err(|error| {
            CmdError::click(format!("{description} returned invalid JSON: {error}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })
    }

    async fn request_json(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        description: &str,
    ) -> Result<Value, CmdError> {
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = crate::wait::request(request).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(json!({"already_absent": true}));
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_error(description, status, &text));
        }
        if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(&text).map_err(|error| {
                CmdError::click(format!("{description} returned invalid JSON: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })
        }
    }

    async fn wait_operation(&self, operation: &Value) -> Result<(), CmdError> {
        if operation.get("already_absent").and_then(Value::as_bool) == Some(true)
            || operation.is_null()
        {
            return Ok(());
        }
        // Compute answers `POST {selfLink}/wait` only once the operation is
        // done or its own server-side wait elapses, so the loop asks again
        // without a client clock. Cloud SQL has no wait method; its
        // operation is read until it reports DONE.
        let request = match operation.get("selfLink").and_then(Value::as_str) {
            Some(link) => Some((Method::POST, format!("{link}/wait"))),
            None => (operation.get("kind").and_then(Value::as_str) == Some("sql#operation"))
                .then(|| operation.get("name").and_then(Value::as_str))
                .flatten()
                .map(|name| {
                    (
                        Method::GET,
                        format!(
                            "https://sqladmin.googleapis.com/sql/v1beta4/projects/{}/operations/{name}",
                            self.project
                        ),
                    )
                }),
        };
        let Some((method, url)) = request else {
            return Ok(());
        };
        loop {
            let value = self
                .request_json(method.clone(), &url, None, "wait for resource operation")
                .await?;
            if value.get("already_absent").and_then(Value::as_bool) == Some(true) {
                return Err(CmdError::click(format!(
                    "resource operation disappeared while waiting: {url}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            if value.get("status").and_then(Value::as_str) == Some("DONE") {
                if value.get("error").is_some_and(|error| !error.is_null()) {
                    return Err(CmdError::click(format!(
                        "resource operation failed: {}",
                        value["error"]
                    ))
                    .stating(crate::primitives::failure::FailureCode::InfraDown));
                }
                return Ok(());
            }
        }
    }

    fn compute_url(&self, path: &str) -> String {
        format!("https://compute.googleapis.com/compute/v1{path}")
    }
}

fn api_error(description: &str, status: reqwest::StatusCode, body: &str) -> CmdError {
    CmdError::click(format!("{description} -> HTTP {}: {body}", status.as_u16()))
        .stating(crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16()))
}
