//! The GCP REST executor: one authenticated client for the batch, and the
//! transport every typed method in this tree goes through.

use std::time::{Duration, Instant};

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
        let auth = crate::skarbiec::gcp_provider()
            .await
            .map_err(|error| CmdError::click(format!("GCP authentication failed: {error}")))?;
        let token = auth
            .token(&[CLOUD_PLATFORM_SCOPE])
            .await
            .map_err(|error| CmdError::click(format!("GCP token failed: {error}")))?;
        let http = reqwest::Client::builder()
            .user_agent(format!(
                "stado/{} resource-operations",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(
                chrono::Duration::minutes(true as i64)
                    .num_seconds()
                    .try_into()
                    .unwrap_or_default(),
            ))
            .build()?;
        Ok(Self {
            http,
            token: token.as_str().to_string(),
            project: project.to_string(),
        })
    }

    async fn get_allow_404(&self, url: &str, description: &str) -> Result<Option<Value>, CmdError> {
        let response = self.http.get(url).bearer_auth(&self.token).send().await?;
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
        let response = request.send().await?;
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
            })
        }
    }

    async fn wait_operation(&self, operation: &Value) -> Result<(), CmdError> {
        if operation.get("already_absent").and_then(Value::as_bool) == Some(true)
            || operation.is_null()
        {
            return Ok(());
        }
        let url = operation
            .get("selfLink")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                (operation.get("kind").and_then(Value::as_str) == Some("sql#operation"))
                    .then(|| {
                        operation.get("name").and_then(Value::as_str).map(|name| {
                            format!(
                                "https://sqladmin.googleapis.com/sql/v1beta4/projects/{}/operations/{name}",
                                self.project
                            )
                        })
                    })
                    .flatten()
            });
        let Some(url) = url else {
            return Ok(());
        };
        let deadline = Instant::now()
            + Duration::from_secs(
                chrono::Duration::hours(true as i64)
                    .num_seconds()
                    .try_into()
                    .unwrap_or_default(),
            );
        loop {
            let value = self
                .get_allow_404(&url, "poll resource operation")
                .await?
                .ok_or_else(|| CmdError::click("resource operation disappeared while polling"))?;
            if value.get("status").and_then(Value::as_str) == Some("DONE") {
                if value.get("error").is_some_and(|error| !error.is_null()) {
                    return Err(CmdError::click(format!(
                        "resource operation failed: {}",
                        value["error"]
                    )));
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(CmdError::click(format!(
                    "resource operation did not finish before timeout: {url}"
                )));
            }
            tokio::time::sleep(Duration::from_secs(true as u64)).await;
        }
    }

    fn compute_url(&self, path: &str) -> String {
        format!("https://compute.googleapis.com/compute/v1{path}")
    }
}

fn api_error(description: &str, status: reqwest::StatusCode, body: &str) -> CmdError {
    CmdError::click(format!(
        "{description} -> HTTP {}: {}",
        status.as_u16(),
        body.chars().take(u8::MAX as usize).collect::<String>()
    ))
}
