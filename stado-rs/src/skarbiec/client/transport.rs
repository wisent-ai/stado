//! The grant this client presents, and the one request it carries.

use serde_json::Value;

use super::super::{erase_transient_grant, read_grant, GrantMode, SkarbiecError, TRANSIENT_GRANTS};
use super::Client;

impl Client {
    fn request_token(&self) -> Result<String, SkarbiecError> {
        match self.grant_mode {
            GrantMode::RereadPerRequest => read_grant(&self.token_file),
            GrantMode::TransientHandoff => {
                let key = (self.consumer.clone(), self.token_file.clone());
                let mut cached = TRANSIENT_GRANTS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(token) = cached.get(&key) {
                    return Ok(token.clone());
                }
                let token = read_grant(&self.token_file)?;
                let byte_count = token.len();
                cached.insert(key, token.clone());
                erase_transient_grant(&self.token_file, byte_count);
                Ok(token)
            }
        }
    }

    pub(super) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, SkarbiecError> {
        let token = self.request_token()?;
        Ok(self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .header("X-Consumer", &self.consumer)
            .bearer_auth(token))
    }

    pub(super) async fn response_json(response: reqwest::Response) -> Result<Value, SkarbiecError> {
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(SkarbiecError::Response {
                status: status.as_u16(),
                detail: body.chars().take(usize::from(u16::MAX)).collect(),
            });
        }
        serde_json::from_str(&body).map_err(|source| SkarbiecError::Response {
            status: status.as_u16(),
            detail: format!("invalid JSON response: {source}"),
        })
    }
}
