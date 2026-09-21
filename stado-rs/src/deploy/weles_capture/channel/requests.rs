//! What one open channel can be asked: the JSON call, the refusal reading,
//! and the two byte-level reads the artifact handling needs.

use serde_json::Value;

use super::super::RUN_ROUTE;
use super::token::Token;
use super::Channel;
use crate::deploy::DeployError;

/// The named stop a trajectory reported, when the envelope carries one.
///
/// Every Weles trajectory ends by printing one JSON report: `{ok, blocked,
/// detail, …}`. A refused run answers HTTP 502 with that report in
/// `stdout_tail` and `result: null`, and the reading below used to take the
/// whole envelope's last line instead — so `stado credentials seed-enrol`
/// told the operator `the Weles API refused /run with 502 Bad Gateway:
/// {"ok":false,"exitCode":4,…}` while the trajectory's own sentence about
/// which page it could not open sat inside it, unread (2026-09-20).
fn trajectory_stop(payload: &Value) -> Option<String> {
    named_stop(payload)
        .or_else(|| payload.get("result").and_then(named_stop))
        .or_else(|| nested_stop(payload))
}

/// The report inside a captured stream. A trajectory prints it on its own
/// stdout, so it arrives as a line inside `stdout_tail` — and when the run
/// was driven through another layer, as a line inside a report inside that
/// string. Both are searched, newest line first: on 2026-09-21 the
/// authenticator enrolment's `google_push_not_approved` sat one level deeper
/// than the first reading looked, and the operator got the raw 502 envelope.
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

#[cfg(test)]
mod tests {
    use super::trajectory_stop;
    use serde_json::json;

    #[test]
    fn a_stop_in_the_result_is_the_refusal() {
        let stop = trajectory_stop(&json!({
            "ok": false,
            "result": {"ok": false, "blocked": "setup_action_not_found",
                       "detail": "the authenticator page showed no \"Set up authenticator\""},
        }));
        assert_eq!(
            stop.as_deref(),
            Some(
                "setup_action_not_found: the authenticator page showed no \"Set up authenticator\""
            )
        );
    }

    /// What a refused enrolment really answers: `result` is null and the
    /// trajectory's report is the last JSON line of its captured stdout.
    #[test]
    fn a_stop_printed_on_stdout_is_found_there() {
        let stop = trajectory_stop(&json!({
            "ok": false,
            "exitCode": 4,
            "result": null,
            "stdout_tail": "[wsession] start() label=google-authenticator-enrol\n\
                {\"ok\":false,\"blocked\":\"google_sign_in_required\",\"detail\":\"the account is signed out in this profile\"}\n",
        }));
        assert_eq!(
            stop.as_deref(),
            Some("google_sign_in_required: the account is signed out in this profile")
        );
    }

    /// The 2026-09-21 authenticator enrolment: the report the operator needed
    /// — Google sent a push nobody approved — arrived inside a report inside
    /// the captured stdout, one level below where the first reading looked.
    #[test]
    fn a_stop_nested_one_level_deeper_is_still_found() {
        let inner = json!({
            "ok": false,
            "login_item": "claude-wisent-google-sso",
            "blocked": "google_push_not_approved",
            "detail": "Google asked the account's phone to approve the sign-in and no approval arrived",
        })
        .to_string();
        let stop = trajectory_stop(&json!({
            "ok": false,
            "exitCode": 3,
            "result": null,
            "stdout_tail": format!("[wsession] start()\n{{\"report\":{inner}}}\n"),
        }));
        assert_eq!(
            stop.as_deref(),
            Some(
                "google_push_not_approved: Google asked the account's phone to approve the sign-in and no approval arrived"
            )
        );
    }

    /// An envelope carrying no report must not be given one.
    #[test]
    fn an_envelope_without_a_report_names_no_stop() {
        assert_eq!(
            trajectory_stop(&json!({"ok": false, "error": "worker busy", "result": null})),
            None
        );
    }
}
