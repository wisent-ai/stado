//! The authenticated Cloudflare API client: one bearer-token HTTP client that
//! turns every Cloudflare envelope into either its payload or a rendered error.

use reqwest::{Method, RequestBuilder};
use serde_json::Value;

use crate::cli::CmdError;

const API_ROOT: &str = "https://api.cloudflare.com/client/v4";

pub(in crate::cli::cloudflare) struct CloudflareClient {
    http: reqwest::Client,
    api_token: String,
}

impl CloudflareClient {
    pub(super) fn new(api_token: String) -> Result<Self, CmdError> {
        if api_token.trim().is_empty() || api_token.chars().any(char::is_whitespace) {
            return Err(CmdError::click(
                "Cloudflare credential field api_token is empty or malformed",
            ));
        }
        Ok(Self {
            http: reqwest::Client::builder().build()?,
            api_token,
        })
    }

    pub(in crate::cli::cloudflare) async fn get(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, CmdError> {
        self.send(
            self.http.get(format!("{API_ROOT}{path}")).query(query),
            "GET",
            path,
        )
        .await
    }

    pub(in crate::cli::cloudflare) async fn write(
        &self,
        method: Method,
        path: &str,
        body: &Value,
    ) -> Result<Value, CmdError> {
        self.send(
            self.http
                .request(method.clone(), format!("{API_ROOT}{path}"))
                .json(body),
            method.as_str(),
            path,
        )
        .await
    }

    pub(in crate::cli::cloudflare) async fn delete(&self, path: &str) -> Result<Value, CmdError> {
        self.send(
            self.http.delete(format!("{API_ROOT}{path}")),
            "DELETE",
            path,
        )
        .await
    }

    async fn send(
        &self,
        request: RequestBuilder,
        method: &str,
        path: &str,
    ) -> Result<Value, CmdError> {
        let response = request.bearer_auth(&self.api_token).send().await?;
        let status = response.status();
        let payload: Value = response.json().await.map_err(|error| {
            CmdError::click(format!(
                "Cloudflare {method} {path} returned unreadable JSON: {error}"
            ))
        })?;
        let success = payload
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if status.is_success() && success {
            return Ok(payload);
        }
        let detail = cloudflare_errors(&payload);
        Err(CmdError::click(format!(
            "Cloudflare {method} {path} failed with HTTP {}: {detail}",
            status.as_u16()
        )))
    }
}

fn cloudflare_errors(payload: &Value) -> String {
    let messages: Vec<String> = payload
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if messages.is_empty() {
        "Cloudflare returned no error detail".to_string()
    } else {
        messages.join("; ")
    }
}
