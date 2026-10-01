//! Transport construction: API key, base URL, and Skarbiec wiring.
//!
//! Python `BoxHTTPTransport.__init__` plus the validated-base-URL accessor
//! and the Skarbiec-backed constructor.

use super::super::types::BoxError;
use super::BoxHttpTransport;

impl BoxHttpTransport {
    /// Python `BoxHTTPTransport.__init__`: strip the key, require it, and
    /// pin the base URL to an HTTPS API base without query or fragment.
    pub fn new(api_key: &str, base_url: &str) -> Result<Self, BoxError> {
        let key = api_key.trim();
        if key.is_empty() {
            return Err(BoxError::configuration(
                "BOX_API_KEY is required for Box provider",
            ));
        }
        let base_url = base_url.trim_end_matches('/');
        let parsed = url::Url::parse(base_url).map_err(|_| {
            BoxError::configuration(
                "BOX_API_URL must be an HTTPS API base without query or fragment",
            )
        })?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none_or(|h| h.is_empty())
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(BoxError::configuration(
                "BOX_API_URL must be an HTTPS API base without query or fragment",
            ));
        }
        // Rebuild scheme://netloc + path without trailing slash (Python
        // urlunsplit((scheme, netloc, path.rstrip("/"), "", ""))).
        let mut normalized = format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or_default()
        );
        if let Some(port) = parsed.port() {
            normalized.push_str(&format!(":{port}"));
        }
        normalized.push_str(parsed.path().trim_end_matches('/'));
        Ok(Self::assemble(key, &normalized))
    }

    /// Test-only constructor: same wiring, without the HTTPS scheme check,
    /// so a loopback mock can stand in for ascii.dev.
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

    /// Build a transport whose bearer token is resolved from
    /// `stado-box/api_key` in Skarbiec on each request.
    pub fn from_skarbiec(base_url: &str) -> Result<Self, BoxError> {
        let mut transport = Self::new("skarbiec", base_url)?;
        transport.api_key.clear();
        Ok(transport)
    }
}
