//! What one open channel can be asked: the JSON call, the refusal reading,
//! and the two byte-level reads the artifact handling needs.

use serde_json::Value;

use super::super::RUN_ROUTE;
use super::token::Token;
use super::Channel;
use crate::deploy::DeployError;

/// The named stop a trajectory reported, when the envelope carries one.
///
/// A refused Weles run can carry its `{ok, blocked, detail, …}` report
/// inside `stdout_tail` while `result` is null. Read that nested report so
/// the caller sees the trajectory's cause, not only an HTTP status or exit code.
fn trajectory_stop(payload: &Value) -> Option<String> {
    named_stop(payload)
        .or_else(|| payload.get("result").and_then(named_stop))
        .or_else(|| nested_stop(payload))
}

/// The report inside a captured stream. A trajectory prints it on its own
/// stdout, so it arrives as a line inside `stdout_tail` — and when the run
/// was driven through another layer, as a line inside a report inside that
/// string. Both are searched, newest line first: a trajectory's stop reason
/// can sit one level deeper than the first reading looks, and the operator
/// then gets the raw 502 envelope instead of it.
fn nested_stop(payload: &Value) -> Option<String> {
    match payload {
        Value::String(text) => text
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .find_map(|report| trajectory_stop(&report)),
        Value::Object(fields) => fields
            .values()
            .find_map(|field| named_stop(field).or_else(|| nested_stop(field))),
        Value::Array(items) => items
            .iter()
            .find_map(|item| named_stop(item).or_else(|| nested_stop(item))),
        _ => None,
    }
}

/// One report's stop: the code it named, and the sentence beside it.
fn named_stop(report: &Value) -> Option<String> {
    let blocked = report.get("blocked").and_then(Value::as_str)?;
    match report.get("detail").and_then(Value::as_str) {
        Some(detail) if !detail.trim().is_empty() => Some(format!("{blocked}: {detail}")),
        _ => Some(blocked.to_owned()),
    }
}

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
        let response = crate::wait::request(request).await.map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within(format!("the Weles API did not answer {route}"))
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            DeployError::unreachable(format!(
                "the Weles API answered {route} unreadably: {error}"
            ))
        })?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DeployError(format!(
                "the Weles API requires a bearer token and {}",
                self.token.describe()
            ))
            .stating(crate::primitives::failure::FailureCode::Auth));
        }
        // A refusal may carry a plain-text body, which `require_success`
        // quotes; a success that is not JSON is a damaged answer.
        let payload: Value = match serde_json::from_str(&body) {
            Ok(payload) => payload,
            Err(error) if status.is_success() => {
                return Err(DeployError::unreachable(format!(
                "the Weles API answered {route} with {status} and a body that is not JSON: {error}"
            )))
            }
            Err(_) => Value::Null,
        };
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
        let detail = trajectory_stop(&payload)
            .or_else(|| {
                payload
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                body.lines()
                    .rfind(|line| !line.trim().is_empty())
                    .unwrap_or("no reason given")
                    .to_string()
            });
        Err(DeployError(format!(
            "the Weles API refused {route} with {status}: {detail}"
        ))
        .stating(refusal_code(status)))
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

    /// The completed `/run` envelope and, when its trajectory failed, the
    /// refusal it earned. Weles writes browser diagnostics before answering that
    /// 502, so a failed run that carries a run id is an outcome, not an error:
    /// its caller keeps the id needed to read those diagnostics.
    pub(in crate::deploy::weles_capture) async fn run_outcome(
        &self,
        body: &Value,
    ) -> Result<(Value, Option<String>), DeployError> {
        let (status, response_body, payload) = self
            .json_request(reqwest::Method::POST, RUN_ROUTE, Some(body))
            .await?;
        let ran = payload
            .get("run_id")
            .and_then(Value::as_str)
            .is_some_and(|run_id| !run_id.is_empty());
        match Self::require_success(RUN_ROUTE, status, &response_body, payload.clone()) {
            Ok(payload) => Ok((payload, None)),
            Err(refusal) if ran => Ok((payload, Some(refusal.message))),
            Err(refusal) => Err(refusal),
        }
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
        let response = crate::wait::request(request).await.map_err(|error| {
            DeployError::from(crate::cli::entry::error::CmdError::from(error))
                .within(format!("the Weles API did not answer {route}"))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DeployError(format!(
                "the Weles API requires a bearer token and {}",
                self.token.describe()
            ))
            .stating(crate::primitives::failure::FailureCode::Auth));
        }
        if !status.is_success() {
            let detail = match response.text().await {
                Ok(text) if !text.trim().is_empty() => text,
                Ok(_) => "no reason given".to_string(),
                Err(error) => format!("its reason could not be read: {error}"),
            };
            return Err(DeployError(format!(
                "the Weles API refused {route} with {status}: {detail}"
            ))
            .stating(refusal_code(status)));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| {
                DeployError::unreachable(format!(
                    "the Weles API answered {route} unreadably: {error}"
                ))
            })
    }
}

/// The class of a Weles API refusal: its HTTP status where that status says
/// one, and otherwise a refusal, since the API answered and said no.
fn refusal_code(status: reqwest::StatusCode) -> crate::primitives::failure::FailureCode {
    use crate::primitives::failure::FailureCode;
    match FailureCode::from_upstream_status(status.as_u16()) {
        FailureCode::Unknown => FailureCode::Refused,
        known => known,
    }
}
