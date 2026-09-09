//! Least-privilege readers for one credential each: the key the paging path
//! needs and the release authority's private key. Neither read travels on the
//! coordinator's broad grant, and each has its own grant file on disk.

use super::super::client::Client;
use super::super::{GrantMode, SkarbiecError};

impl Client {
    /// Dedicated reader for the one credential the alert path needs.
    ///
    /// Alerts used the coordinator's own grant, which does not carry the
    /// resend key, so the only configured channel resolved to nothing and
    /// `doctor` reported that nothing anywhere would page an operator -- while
    /// the fleet had already provisioned a least-privilege consumer for
    /// exactly this key, with exactly one read on it, and put its token on
    /// disk. Paging is the last thing that should need a broad grant.
    pub fn alert_key_reader() -> Result<Self, SkarbiecError> {
        if crate::config::alert_skarbiec_token_file() == crate::config::skarbiec_token_file() {
            return Err(SkarbiecError::Deployment(
                "alert key reader token file must be distinct from the coordinator grant"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::skarbiec_url(),
            crate::config::alert_skarbiec_consumer(),
            crate::config::alert_skarbiec_token_file(),
            GrantMode::RereadPerRequest,
        )
    }

    /// Dedicated reader for the release authority's private key.
    ///
    /// `release submit` read it through the coordinator grant, exactly as alerts
    /// once read the resend key, and the vault refused with `403 consumer not
    /// authorized to read item field`. The fleet had already provisioned a
    /// least-privilege consumer holding one capability --
    /// `read:stado-release-signing#private_key` -- so the policy was right and
    /// the caller was reaching for the wrong identity. Signing material is the
    /// last thing that should travel on a broad grant.
    pub fn release_signing_reader() -> Result<Self, SkarbiecError> {
        if crate::config::release_signing_skarbiec_token_file()
            == crate::config::skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "release signing reader token file must be distinct from the coordinator grant"
                    .to_string(),
            ));
        }
        Self::direct(
            crate::config::release_skarbiec_url(),
            crate::config::release_signing_skarbiec_consumer(),
            crate::config::release_signing_skarbiec_token_file(),
            GrantMode::RereadPerRequest,
        )
    }
}
