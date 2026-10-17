//! Transport construction: API key, base URL, and Skarbiec wiring.
//!
//! The validated-base-URL constructor and accessor, and the Skarbiec-backed
//! constructor whose key is read from the vault on every request.

use super::super::types::BoxError;
use super::BoxHttpTransport;

/// The setting that names another Boat API base, as `stado config` spells
/// it and as its environment override does.
const API_URL_SETTING: &str = "boat.api_url (environment override BOAT_API_URL)";

impl BoxHttpTransport {
    /// Strip the key, require it, and pin the base URL to an HTTPS API base
    /// without query or fragment.
    pub fn new(api_key: &str, base_url: &str) -> Result<Self, BoxError> {
        let key = api_key.trim();
        if key.is_empty() {
            return Err(BoxError::configuration(
                "a Boat API key is required to build the Boat transport",
            ));
        }
        let base_url = base_url.trim_end_matches('/');
        let parsed = url::Url::parse(base_url).map_err(|error| {
            BoxError::configuration(format!(
                "{API_URL_SETTING} must be an HTTPS API base without query or fragment; {base_url:?} is not a URL: {error}"
            ))
        })?;
        let host = match parsed.host_str() {
            Some(host) if !host.is_empty() => host,
            Some(_) | None => {
                return Err(BoxError::configuration(format!(
                    "{API_URL_SETTING} must be an HTTPS API base without query or fragment; {base_url:?} names no host"
                )))
            }
        };
        if parsed.scheme() != "https" || parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(BoxError::configuration(format!(
                "{API_URL_SETTING} must be an HTTPS API base without query or fragment; {base_url:?} is not"
            )));
        }
        // Rebuild scheme://netloc + path without trailing slash.
        let mut normalized = format!("{}://{host}", parsed.scheme());
        if let Some(port) = parsed.port() {
            normalized.push_str(&format!(":{port}"));
        }
        normalized.push_str(parsed.path().trim_end_matches('/'));
        Ok(Self::assemble(key, &normalized))
    }

    /// Same wiring without the HTTPS scheme check, so a loopback server can
    /// stand in for Boat.
    fn assemble(api_key: &str, base_url: &str) -> Self {
        BoxHttpTransport {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
        }
    }

    /// The validated base URL (`https://host[/path]` without trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Build a transport whose bearer token is resolved from the Boat
    /// credential item in Skarbiec on each request.
    pub fn from_skarbiec(base_url: &str) -> Result<Self, BoxError> {
        let mut transport = Self::new("skarbiec", base_url)?;
        transport.api_key.clear();
        Ok(transport)
    }
}
