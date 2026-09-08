//! ARM REST transport shared by every Azure caller: audience constants,
//! the error type, the bearer-token source and the [`ArmClient`] built on
//! top of them.

mod client;
mod verbs;

pub use client::ArmClient;
pub(crate) use verbs::vm_path;

/// ARM REST base.
pub const ARM_API_BASE: &str = "https://management.azure.com";
/// Compute RP API version for VM resource paths. Crate-visible so the
/// agent's self-delete ([`crate::providers::local::azure_self`]) targets
/// the same VM contract this provider creates against.
pub(crate) const COMPUTE_API_VERSION: &str = "2023-09-01";
pub(super) const NETWORK_API_VERSION: &str = "2023-09-01";
/// OAuth scope for the client-credentials token request.
const ARM_SCOPE: &str = "https://management.azure.com/.default";
/// Resource for IMDS / az-CLI token requests.
const ARM_RESOURCE: &str = "https://management.azure.com";

/// Azure auth/transport/API error. The `Api` message embeds the ARM
/// `error.code` + `error.message` so the Python substring classification
/// ("QuotaExceeded", "OperationNotAllowed", "SkuNotAvailable", "already
/// exists") works on `error.to_string()`.
#[derive(Debug, thiserror::Error)]
pub enum AzureError {
    /// Token acquisition failed (or every chain source unavailable).
    #[error("no Azure credentials: {0}")]
    Auth(String),
    /// Transport failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Non-2xx ARM response or failed LRO; message carries code + text.
    #[error("{0}")]
    Api(String),
}

// --- Managed-identity / Skarbiec token source ---

/// Fresh bearer token for ARM, from the shared chain's per-scope cache.
async fn bearer_token(http: &reqwest::Client) -> Result<String, AzureError> {
    crate::azure_token::bearer_token(http, ARM_SCOPE, ARM_RESOURCE)
        .await
        .map_err(|err| match err {
            crate::azure_token::TokenError::Auth(msg) => AzureError::Auth(msg),
            crate::azure_token::TokenError::Http(err) => AzureError::Http(err),
        })
}
