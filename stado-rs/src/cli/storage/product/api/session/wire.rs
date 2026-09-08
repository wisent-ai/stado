//! Reading one response: the JSON shape, the success body under its ceiling,
//! and the refusal a failure renders as.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) fn generic_bearer(&self) -> Option<&str> {
        match &self.auth {
            RemoteObjectAuth::Generic(token) => Some(token),
            RemoteObjectAuth::PublisherOnly | RemoteObjectAuth::Public => None,
        }
    }

    pub(in crate::cli::storage) async fn response_json<T>(
        &self,
        response: reqwest::Response,
        operation: &str,
        bearer: Option<&str>,
    ) -> Result<T, CmdError>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();
        let body = self
            .success_body(response, max_object_api_json_body(), operation, bearer)
            .await?;
        serde_json::from_slice(&body).map_err(|error| {
            CmdError::click(format!(
                "Stado object API returned invalid JSON for {operation} (HTTP {status}): {error}"
            ))
        })
    }

    pub(in crate::cli::storage) async fn success_body(
        &self,
        mut response: reqwest::Response,
        limit: usize,
        operation: &str,
        bearer: Option<&str>,
    ) -> Result<Vec<u8>, CmdError> {
        if !response.status().is_success() {
            return Err(self.response_error(response, bearer).await);
        }
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(CmdError::click(format!(
                "Stado object API {operation} response exceeds the {limit}-byte limit"
            )));
        }
        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default();
        let mut body = Vec::with_capacity(capacity);
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            CmdError::click(format!(
                "Stado object API {operation} response body connection closed before completion \
                 after {} bytes: {error}",
                body.len()
            ))
        })? {
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(CmdError::click(format!(
                    "Stado object API {operation} response exceeds the {limit}-byte limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub(in crate::cli::storage) async fn response_error(
        &self,
        mut response: reqwest::Response,
        bearer: Option<&str>,
    ) -> CmdError {
        let status = response.status();
        let endpoint = response.url().clone();
        let declared_length = response.content_length();
        let max_body = max_object_api_error_body();
        let mut body = Vec::new();
        let mut truncated = false;
        while body.len() < max_body {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => {
                    let detail = response_body_detail(&body, self.generic_bearer(), bearer);
                    return CmdError::click(format!(
                        "Stado object API returned HTTP {status} from {endpoint}; partial response body: \
                         {detail}; body read failed: {error}"
                    ));
                }
            };
            let remaining = max_body - body.len();
            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
        }
        if body.len() == max_body
            && declared_length
                .map(|length| length > max_body as u64)
                .unwrap_or(true)
        {
            truncated = true;
        }
        let detail = response_body_detail(&body, self.generic_bearer(), bearer);
        let suffix = if truncated {
            " [response body truncated]"
        } else {
            ""
        };
        CmdError::click(format!(
            "Stado object API returned HTTP {status} from {endpoint}: {detail}{suffix}"
        ))
    }
}
