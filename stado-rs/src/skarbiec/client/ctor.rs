//! Building a client: which grant it consumes, and the HTTP handle it carries.

use std::time::Duration;

use super::super::{checked_url, GrantMode, SkarbiecError};
use super::Client;

impl Client {
    /// The client for the Stado consumer grant this host is configured with.
    ///
    /// The mode is stated here rather than by the callers of this constructor,
    /// because they supply no coordinates and so know strictly less than it
    /// does: on an agent VM the startup template points `WC_SKARBIEC_TOKEN_FILE`
    /// at the platform's one-shot handoff, and everywhere else the configured
    /// grant is an operator-provisioned file that stays put.
    pub fn configured() -> Result<Self, SkarbiecError> {
        let token_file = crate::config::skarbiec_token_file();
        Self::new(
            crate::config::skarbiec_url(),
            crate::config::skarbiec_consumer(),
            token_file,
            GrantMode::for_grant_file(token_file),
        )
    }

    pub fn new(
        base_url: &str,
        consumer: &str,
        token_file: &str,
        grant_mode: GrantMode,
    ) -> Result<Self, SkarbiecError> {
        Self::build(base_url, consumer, token_file, true, grant_mode)
    }

    pub(crate) fn direct(
        base_url: &str,
        consumer: &str,
        token_file: &str,
        grant_mode: GrantMode,
    ) -> Result<Self, SkarbiecError> {
        Self::build(base_url, consumer, token_file, false, grant_mode)
    }

    /// One bound, on the whole request. A second bound on establishment alone
    /// used to sit beside it and covered no failure the request bound does not:
    /// the broker is on loopback or the tailnet, so a handshake that cannot
    /// finish fails with its own error, and a broker that accepts and then goes
    /// quiet was never inside the establishment bound in the first place.
    fn build(
        base_url: &str,
        consumer: &str,
        token_file: &str,
        route_store: bool,
        grant_mode: GrantMode,
    ) -> Result<Self, SkarbiecError> {
        let consumer = consumer.trim().to_string();
        let token_file = token_file.trim().to_string();
        if !route_store && consumer.is_empty() {
            return Err(SkarbiecError::MissingConsumer);
        }
        if !route_store && token_file.is_empty() {
            return Err(SkarbiecError::MissingTokenFile);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()?;
        let base_url = if route_store {
            base_url.trim().to_string()
        } else {
            checked_url(base_url)?
        };
        Ok(Self {
            http,
            base_url,
            consumer,
            token_file,
            route_store,
            grant_mode,
        })
    }
}
