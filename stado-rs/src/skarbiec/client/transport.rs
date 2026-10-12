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

    /// Send one request built by [`Self::request`] and wait for its answer
    /// for as long as this host's resolver does not hold the adapter it
    /// rides. A request already sent when the vault stops answering would
    /// otherwise stand for as long as the vault does: at every refresh
    /// interval this host's resolver declares, the published waits are read
    /// again, and once a connection on this adapter has gone unanswered
    /// longer than that interval - this request's own among them - the
    /// request is dropped, which closes its connection, and refused with the
    /// held wait's sentence ([`SkarbiecError::Held`]). A broker reached
    /// without a resolver adapter, or a host that declares no resolver, has
    /// no such reading and is waited on as before.
    pub(super) async fn send(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, SkarbiecError> {
        let sent = crate::wait::send(crate::wait::Kind::Network, "Skarbiec", builder);
        let (Ok(address), Some(period)) = (url::Url::parse(&self.base_url), refresh_period())
        else {
            return Ok(sent.await?);
        };
        tokio::pin!(sent);
        let mut schedule = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            tokio::select! {
                answer = &mut sent => return Ok(answer?),
                _ = schedule.tick() => match crate::cli::resolver::held_at(&address) {
                    Ok(None) => {}
                    Ok(Some(held)) => {
                        return Err(SkarbiecError::Held(format!(
                            "{address} request abandoned unanswered: {held}"
                        )))
                    }
                    Err(unreadable) => {
                        return Err(SkarbiecError::Held(format!(
                            "{address} request abandoned unanswered: whether this host's \
                             resolver holds that adapter cannot be read: {unreadable}"
                        )))
                    }
                },
            }
        }
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

/// The refresh interval this host's resolver declares, read from the
/// last-known-good registry copy as the resolver's own held reading is:
/// `None` when this host keeps no copy or declares no resolver.
fn refresh_period() -> Option<std::time::Duration> {
    let document = crate::cli::resolver::last_good_document().ok()?;
    let target = crate::cli::resolver::current_target(&document).ok()?;
    crate::service_resolution::resolver_config(&document, &target)
        .ok()
        .map(|config| std::time::Duration::from_secs(config.refresh_seconds))
        .filter(|period| !period.is_zero())
}
