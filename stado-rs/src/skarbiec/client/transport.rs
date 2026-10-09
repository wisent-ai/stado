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

    /// The one request this client carries, refused before it is sent when
    /// the broker is reached through an adapter this host's resolver already
    /// holds a connection on, unanswered past the directory refresh interval
    /// ([`SkarbiecError::Held`]): sent then, it would stand behind that
    /// connection for as long as it stands.
    pub(super) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, SkarbiecError> {
        let address = format!("{}{path}", self.base_url);
        if let Ok(parsed) = url::Url::parse(&address) {
            match crate::cli::resolver::held_at(&parsed) {
                Ok(None) => {}
                Ok(Some(held)) => {
                    return Err(SkarbiecError::Held(format!(
                        "{method} {address} not sent: {held}"
                    )))
                }
                Err(unreadable) => {
                    return Err(SkarbiecError::Held(format!(
                        "{method} {address} not sent: whether this host's resolver holds that \
                         adapter cannot be read: {unreadable}"
                    )))
                }
            }
        }
        let token = self.request_token()?;
        Ok(self
            .http
            .request(method, address)
            .header("X-Consumer", &self.consumer)
            .bearer_auth(token))
    }

    pub(super) async fn response_json(response: reqwest::Response) -> Result<Value, SkarbiecError> {
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            let mut detail: String = body.chars().take(usize::from(u16::MAX)).collect();
            // A 403 is the broker refusing this consumer. When every stado read
            // answers it, the grant on the vault owner and the bearer file the
            // fleet holds have diverged; the repair is named where the status
            // is read, without looking for words in the broker's body.
            if status == reqwest::StatusCode::FORBIDDEN {
                detail.push_str(
                    " — if every stado read answers this, the stado grant no longer matches the \
                     bearer file: `stado credentials grant rebind --host <vault owner> --token-file \
                     <that host's stado token file>` binds it back with the same capabilities",
                );
            }
            return Err(SkarbiecError::Response {
                status: status.as_u16(),
                detail,
            });
        }
        serde_json::from_str(&body).map_err(|source| SkarbiecError::Response {
            status: status.as_u16(),
            detail: format!("invalid JSON response: {source}"),
        })
    }
}
