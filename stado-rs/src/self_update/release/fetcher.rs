//! The release read: the download seam, the configuration check that binds
//! it to one immutable coordinate, and the public HTTPS object-route
//! implementation.

use async_trait::async_trait;

use crate::binary::release::canonical_coordinate;

use super::error::SelfUpdateError;

/// Download seam for exact release objects. Runtime implementations accept
/// only `<version>/<platform>/<name>` under their configured coordinate.
#[async_trait]
pub trait ReleaseFetcher: Send + Sync {
    /// Object bytes, or `None` when the object does not exist.
    async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError>;
}

fn release_coordinates_error(api_url: &str, version: &str, platform: &str) -> Option<String> {
    if !canonical_coordinate(version) || !canonical_coordinate(platform) {
        return Some(
            "release.version and release.platform must be exact non-empty coordinates".to_string(),
        );
    }
    if api_url.contains('<') && api_url.contains('>') {
        return Some("api.url contains an unresolved placeholder".to_string());
    }
    let parsed = match url::Url::parse(api_url) {
        Ok(parsed) => parsed,
        Err(error) => return Some(format!("api.url is not an absolute URL: {error}")),
    };
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || (parsed.path() != "/" && !parsed.path().is_empty())
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Some(
            "api.url must be an HTTPS origin without credentials, query, or fragment".to_string(),
        );
    }
    None
}

/// The route every release object is publicly read through.
const RELEASE_ROUTE: &str = "/api/release/object";

/// The state the release route answers for an object it does not hold.
const ABSENT_STATE: &str = "absent";

/// Public HTTPS Stado object-route fetcher bound to one exact configured
/// version and platform.
pub struct HttpReleaseFetcher {
    http: reqwest::Client,
    api_url: String,
    pub(in crate::self_update) version: String,
    pub(in crate::self_update) platform: String,
    pub(in crate::self_update) configuration_error: Option<String>,
}

impl HttpReleaseFetcher {
    /// Bind every fetch to the configured immutable release coordinate.
    pub fn new() -> Self {
        let api_url = crate::config::stado_api_url();
        let version = crate::config::stado_release_version();
        let platform = crate::config::stado_release_platform();
        let configuration_error = release_coordinates_error(&api_url, &version, &platform);
        Self {
            http: reqwest::Client::new(),
            api_url,
            version,
            platform,
            configuration_error,
        }
    }
}

impl Default for HttpReleaseFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpReleaseFetcher {
    /// Where release objects are read: the one registry `public_origins`
    /// declaration that publishes [`RELEASE_ROUTE`], because that is the
    /// origin the fleet says answers it (the configured `api.url` can name
    /// an edge that no longer forwards the route). Self-update is
    /// the recovery root, so a registry that cannot be read, or that declares
    /// no public origin at all, leaves the configured `api.url`; a declaration
    /// set that publishes the route from none or several origins is refused.
    async fn release_origin(&self) -> Result<String, SelfUpdateError> {
        let Ok(document) = crate::cli::registry::fetch_document().await else {
            return Ok(self.api_url.clone());
        };
        match crate::public_origin::publishing(&document, RELEASE_ROUTE) {
            Ok(origin) => Ok(origin.origin()),
            Err(_) if crate::public_origin::declarations(&document).is_empty() => {
                Ok(self.api_url.clone())
            }
            Err(refusal) => Err(SelfUpdateError::Fetch(refusal)),
        }
    }
}

#[async_trait]
impl ReleaseFetcher for HttpReleaseFetcher {
    async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError> {
        if let Some(error) = &self.configuration_error {
            return Err(SelfUpdateError::Fetch(error.clone()));
        }
        let expected_prefix = format!("{}/{}/", self.version, self.platform);
        let Some(name) = object_path.strip_prefix(&expected_prefix) else {
            return Err(SelfUpdateError::Fetch(
                "release object is outside the configured version/platform".to_string(),
            ));
        };
        if name.is_empty() || name.contains('/') {
            return Err(SelfUpdateError::Fetch(
                "release object name must be one exact path segment".to_string(),
            ));
        }
        let release_uri = format!("stado://releases/stado/{object_path}");
        let origin = self.release_origin().await?;
        let mut endpoint = url::Url::parse(&origin)
            .and_then(|base| base.join(RELEASE_ROUTE))
            .map_err(|error| SelfUpdateError::Fetch(format!("invalid release API: {error}")))?;
        endpoint.query_pairs_mut().append_pair("uri", &release_uri);
        let response = self
            .http
            .get(endpoint.clone())
            .send()
            .await
            .map_err(|error| SelfUpdateError::Fetch(format!("{endpoint}: {error}")))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            // Only the release route's own answer means the object is absent:
            // `{"state":"absent",...}`. Anything else at 404 is a host that
            // does not serve the route at all (a site edge answers its own
            // HTML 404 page), and reading that as
            // "no such release" would report a missing version instead of a
            // wrong api.url.
            let body = response
                .bytes()
                .await
                .map_err(|error| SelfUpdateError::Fetch(format!("{endpoint}: {error}")))?;
            let absent = serde_json::from_slice::<serde_json::Value>(&body)
                .is_ok_and(|answer| answer["state"] == ABSENT_STATE);
            if absent {
                return Ok(None);
            }
            return Err(SelfUpdateError::Fetch(format!(
                "{endpoint} -> HTTP {status} without the release route's absent answer, so {} \
                 does not serve /api/release/object; set api.url to the scheme and host of \
                 `stado web origin url /api/release/object`",
                origin
            )));
        }
        if !status.is_success() {
            return Err(SelfUpdateError::Fetch(format!(
                "{endpoint} -> HTTP {status}"
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| SelfUpdateError::Fetch(format!("{endpoint}: {error}")))?;
        Ok(Some(bytes.to_vec()))
    }
}
