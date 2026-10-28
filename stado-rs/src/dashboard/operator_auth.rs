//! Authentication for the native Desktop command API, retained independently
//! of the retired HTML dashboard.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde_json::{json, Value};
use url::Url;

use super::{trusted_request_host, Request};

/// The roles the backend messaging boundary reads, which configuration
/// validation holds `backend.messaging.skarbiec.items` to as well.
pub(crate) const EMAIL_ROLE: &str = "email-provider";
const APNS_ROLE: &str = "apns";
const FCM_ROLE: &str = "fcm";
const DEVICE_REGISTRY_ROLE: &str = "device-registry";
pub(crate) const REQUIRED_ROLES: &[&str] = &[APNS_ROLE, FCM_ROLE, DEVICE_REGISTRY_ROLE];
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// Why operator permission could not be decided. The display is what a
/// remote client is told; `cause` is what the server logs, because it names
/// vault items and fields no unauthenticated caller may read.
#[derive(Debug, thiserror::Error)]
pub(super) enum OperatorAuthError {
    #[error("backend messaging credential boundary is not configured")]
    Configuration { cause: String },
    #[error("backend messaging credential contract is unavailable")]
    Credential { cause: String },
    #[error("operator permission request failed: {0}")]
    Request(reqwest::Error),
    #[error("operator permission request returned HTTP {0}")]
    Response(reqwest::StatusCode),
}

impl OperatorAuthError {
    /// The whole reason, for the server's log.
    pub(super) fn cause(&self) -> String {
        match self {
            Self::Configuration { cause } | Self::Credential { cause } => {
                format!("{self}: {cause}")
            }
            Self::Request(_) | Self::Response(_) => self.to_string(),
        }
    }
}

fn credential(cause: impl std::fmt::Display) -> OperatorAuthError {
    OperatorAuthError::Credential {
        cause: cause.to_string(),
    }
}

/// Backend messaging items are read through Stado's Skarbiec identity.
fn messaging_vault() -> Result<crate::skarbiec::Client, OperatorAuthError> {
    let configured = crate::config::backend_messaging_skarbiec_items();
    let required = REQUIRED_ROLES.iter().copied().collect::<BTreeSet<_>>();
    let actual = configured
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let items_valid = actual.len() == configured.len()
        && required.is_subset(&actual)
        && actual
            .iter()
            .all(|role| required.contains(role) || *role == EMAIL_ROLE);
    if !items_valid {
        return Err(OperatorAuthError::Configuration {
            cause: format!(
                "backend.messaging.skarbiec.items is {configured:?}; it must name each of \
                 {REQUIRED_ROLES:?} once, and may add {EMAIL_ROLE}"
            ),
        });
    }
    crate::skarbiec::Client::stado().map_err(|error| OperatorAuthError::Configuration {
        cause: format!("Stado's Skarbiec identity cannot be used: {error}"),
    })
}

fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, OperatorAuthError> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or_else(|| {
            credential(format!(
                "{DEVICE_REGISTRY_ROLE}.{field} is not a non-empty string without surrounding space"
            ))
        })
}

async fn metadata() -> Result<(Url, String), OperatorAuthError> {
    let vault = messaging_vault()?;
    let raw_url = vault
        .read_field(DEVICE_REGISTRY_ROLE, "url")
        .await
        .map_err(|error| {
            credential(format!(
                "{DEVICE_REGISTRY_ROLE}.url cannot be read: {error}"
            ))
        })?;
    let raw_key = vault
        .read_field(DEVICE_REGISTRY_ROLE, "anon_key")
        .await
        .map_err(|error| {
            credential(format!(
                "{DEVICE_REGISTRY_ROLE}.anon_key cannot be read: {error}"
            ))
        })?;
    let base = Url::parse(required(&raw_url, "url")?)
        .map_err(|error| credential(format!("{DEVICE_REGISTRY_ROLE}.url is not a URL: {error}")))?;
    // The device registry is the hosted project's API: HTTPS, under the
    // provider's domain, an origin and nothing else. A URL without a host
    // is not it.
    let hosted = base
        .host_str()
        .is_some_and(|host| host.ends_with(".supabase.co"));
    if base.scheme() != "https"
        || !hosted
        || !base.username().is_empty()
        || base.password().is_some()
        || base.port().is_some()
        || (base.path() != "/" && !base.path().is_empty())
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(credential(format!(
            "{DEVICE_REGISTRY_ROLE}.url {base} is not the hosted project's HTTPS origin"
        )));
    }
    Ok((base, required(&raw_key, "anon_key")?.to_string()))
}

pub(super) async fn authorized(request: &Request) -> Result<bool, OperatorAuthError> {
    // Preserve the native local contract: the loopback listener's Host guard
    // rejects DNS rebinding, and a forwarded request never receives local trust.
    if request.header("x-forwarded-proto").is_none()
        && trusted_request_host(request.header("host"), None, false)
    {
        return Ok(true);
    }
    let deployment_id = crate::config::stado_deployment_id();
    // Without a bearer there is nothing to authorize: refused, not asked.
    let Some(authorization) = request.header("authorization").map(str::trim) else {
        return Ok(false);
    };
    if deployment_id.is_empty() || !authorization.starts_with("Bearer ") {
        return Ok(false);
    }
    let (base, anon_key) = metadata().await?;
    let endpoint = base
        .join("/rest/v1/rpc/stado_can_access")
        .map_err(|error| credential(format!("the permission endpoint under {base}: {error}")))?;
    let response = crate::wait::request(
        HTTP.post(endpoint)
            .header("apikey", anon_key)
            .header("Authorization", authorization)
            .json(&json!({
                "target_deployment_id": deployment_id,
                "requested_permission": "operate",
            })),
    )
    .await
    .map_err(OperatorAuthError::Request)?;
    if matches!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        return Ok(false);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(OperatorAuthError::Response(response.status()));
    }
    response
        .json::<Value>()
        .await
        .map(|value| value == Value::Bool(true))
        .map_err(OperatorAuthError::Request)
}
