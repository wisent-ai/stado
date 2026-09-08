//! What one open channel can be asked: the JSON call, the refusal reading,
//! and the two byte-level reads the artifact handling needs.

use serde_json::Value;

use super::super::RUN_ROUTE;
use super::token::Token;
use super::Channel;
use crate::deploy::DeployError;

impl Channel {
    /// The state of the bearer token this channel is using, for the report.
    pub fn token_state(&self) -> &'static str {
        self.token.state()
    }

    /// Whether the call crossed an ssh forward or stayed on this machine's
    /// loopback. The two answers are different claims about what was reached,
    /// and a report that says only `127.0.0.1` cannot tell them apart — which
    /// is how a marker naming a port nothing had ever bound went unnoticed on
    /// this fleet for weeks.
    pub fn transport(&self) -> &'static str {
        if self.forward.is_some() {
            "ssh"
        } else {
            "loopback"
        }
    }

    async fn json_request(
        &self,
        method: reqwest::Method,
        route: &str,
        body: Option<&Value>,
    ) -> Result<(reqwest::StatusCode, String, Value), DeployError> {
        let mut request = self
            .client
            .request(method, format!("{}{route}", self.base_url));
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Token::Present(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| {
            DeployError(format!("the Weles API did not answer {route}: {error}"))
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            DeployError(format!(
                "the Weles API answered {route} unreadably: {error}"
            ))
        })?;
        let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DeployError(format!(
                "the Weles API requires a bearer token and {}",
                self.token.describe()
            )));
        }
        Ok((status, body, payload))
    }

    fn require_success(
        route: &str,
        status: reqwest::StatusCode,
        body: &str,
        payload: Value,
    ) -> Result<Value, DeployError> {
        if status.is_success() && payload.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(payload);
        }
        let detail = payload
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                body.lines()
                    .rfind(|line| !line.trim().is_empty())
                    .unwrap_or("no reason given")
                    .to_string()
            });
        Err(DeployError(format!(
            "the Weles API refused {route} with {status}: {detail}"
        )))
    }

    /// One Weles API call. Successful responses are returned in full because
    /// the synchronous `/run` surface has no `{ data }` envelope.
    pub(in crate::deploy::weles_capture) async fn call(
        &self,
        route: &str,
        body: &Value,
    ) -> Result<Value, DeployError> {
        let (status, response_body, payload) = self
            .json_request(reqwest::Method::POST, route, Some(body))
            .await?;
        Self::require_success(route, status, &response_body, payload)
    }

    /// Keep a completed `/run` envelope even when its trajectory failed. Weles
    /// writes browser diagnostics before returning that 502; treating the HTTP
    /// status as if no run happened discards the exact network record needed to
    /// explain the failure.
    pub(super) async fn observe_run(&self, body: &Value) -> Result<Value, DeployError> {
        let (status, response_body, payload) = self
            .json_request(reqwest::Method::POST, RUN_ROUTE, Some(body))
            .await?;
        if payload
            .get("run_id")
            .and_then(Value::as_str)
            .is_some_and(|run_id| !run_id.is_empty())
        {
            return Ok(payload);
        }
        Self::require_success(RUN_ROUTE, status, &response_body, payload)
    }

    pub(in crate::deploy::weles_capture) async fn get_json(
        &self,
        route: &str,
    ) -> Result<Value, DeployError> {
        let (status, response_body, payload) =
            self.json_request(reqwest::Method::GET, route, None).await?;
        Self::require_success(route, status, &response_body, payload)
    }

    pub(in crate::deploy::weles_capture) async fn get_bytes(
        &self,
        route: &str,
    ) -> Result<Vec<u8>, DeployError> {
        let mut request = self.client.get(format!("{}{route}", self.base_url));
        if let Token::Present(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| {
            DeployError(format!("the Weles API did not answer {route}: {error}"))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DeployError(format!(
                "the Weles API requires a bearer token and {}",
                self.token.describe()
            )));
        }
        if !status.is_success() {
            let detail = response
                .text()
                .await
                .unwrap_or_else(|_| "no reason given".to_string());
            return Err(DeployError(format!(
                "the Weles API refused {route} with {status}: {detail}"
            )));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| {
                DeployError(format!(
                    "the Weles API answered {route} unreadably: {error}"
                ))
            })
    }
}
