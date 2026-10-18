//! The request path: binary bodies and JSON envelopes, read whole.
//!
//! Binary and JSON requests over the shared `send`, which resolves the
//! bearer token from the vault on every call.

use serde_json::{Map, Value};

use super::super::types::BoxError;
use super::response::{api_error, parse_json, read_body, transport_error};
use super::BoxHttpTransport;

/// The field of the Boat credential item that holds the account's API key.
const CREDENTIAL_FIELD: &str = "api_key";

impl BoxHttpTransport {
    /// A raw body, with no `ok` or `type` validation: artifacts are not JSON
    /// envelopes.
    pub async fn request_binary(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, BoxError> {
        self.send(method, path, None, query, &[]).await
    }

    /// A JSON envelope with the `ok=true` and expected-`type` contract
    /// enforced.
    pub async fn request_json(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
        expected_types: &[&str],
    ) -> Result<Map<String, Value>, BoxError> {
        self.request_json_headed(method, path, body, query, &[], expected_types)
            .await
    }

    /// [`Self::request_json`] with request headers an operation requires,
    /// such as the confirmation a permanent deletion carries.
    pub async fn request_json_headed(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
        headers: &[(&str, &str)],
        expected_types: &[&str],
    ) -> Result<Map<String, Value>, BoxError> {
        let raw = self.send(method, path, body, query, headers).await?;
        let payload = parse_json(&raw)?;
        if payload.get("ok") != Some(&Value::Bool(true)) {
            return Err(BoxError::transport(format!(
                "Boat answered {method} {path} without ok=true"
            )));
        }
        let answered = payload.get("type").and_then(Value::as_str);
        if !expected_types.is_empty()
            && !answered.is_some_and(|kind| expected_types.contains(&kind))
        {
            return Err(BoxError::transport(format!(
                "Boat answered {method} {path} with type {answered:?}, not one of {expected_types:?}"
            )));
        }
        Ok(payload)
    }

    /// A JSON answer of an environment change, whose envelope is
    /// `success=true` instead of `ok` and a type, as Boat's OpenAPI document
    /// gives it for those operations.
    pub async fn request_change(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
    ) -> Result<Map<String, Value>, BoxError> {
        let raw = self.send(method, path, body, query, &[]).await?;
        let payload = parse_json(&raw)?;
        if payload.get("success") != Some(&Value::Bool(true)) {
            // Boat's error member is the only account of why the change did
            // not take. The rest of the answer is not quoted: a change may
            // echo the variable or file it was given.
            let said = match payload.get("error") {
                Some(error) => format!("its error: {error}"),
                None => format!(
                    "no error member, only {}",
                    payload.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            };
            return Err(BoxError::transport(format!(
                "Boat answered {method} {path} without success=true and {said}"
            )));
        }
        Ok(payload)
    }

    /// A JSON object answer that carries neither `ok` nor `type` nor
    /// `success`, as Boat's OpenAPI document gives it for sharing a sandbox;
    /// the caller checks the fields the operation documents.
    pub async fn request_object(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Map<String, Value>, BoxError> {
        let raw = self.send(method, path, body, &[], &[]).await?;
        parse_json(&raw)
    }

    /// Execute the request and return the raw body, mapping HTTP error
    /// statuses to [`BoxApiError`](super::super::BoxApiError) and network
    /// failures to [`BoxError::Transport`].
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
        headers: &[(&str, &str)],
    ) -> Result<Vec<u8>, BoxError> {
        let api_key = if self.api_key.is_empty() {
            let role =
                crate::capabilities::cloud_credential_role(crate::capabilities::ProviderId::Boat);
            match crate::skarbiec::read_string(&role, CREDENTIAL_FIELD)
                .await
                .map_err(|err| BoxError::configuration(err.to_string()))?
            {
                Some(value) if !value.is_empty() => value,
                Some(_) | None => {
                    return Err(BoxError::configuration(format!(
                        "no live Skarbiec item tagged stado:role:{role} carries a non-empty \
                         field {CREDENTIAL_FIELD}; store the Boat API key with stado credentials \
                         item put --host <vault host> --role {role} --type api-key and the JSON \
                         object on standard input"
                    )))
                }
            }
        } else {
            self.api_key.clone()
        };
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| BoxError::value(format!("invalid HTTP method {method:?}")))?;
        let mut request = self
            .client
            .request(method, self.url(path, query))
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(reqwest::header::ACCEPT, "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            let data = serde_json::to_vec(body).map_err(|err| BoxError::value(err.to_string()))?;
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(data);
        }
        let response = crate::wait::request(request)
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let raw = read_body(response).await?;
        if !status.is_success() {
            // An answer that echoes the bearer must not carry it into an
            // error, a log or a job record.
            let shown = String::from_utf8_lossy(&raw).replace(api_key.as_str(), "[REDACTED]");
            return Err(api_error(status.as_u16(), shown.as_bytes()));
        }
        Ok(raw)
    }
}
