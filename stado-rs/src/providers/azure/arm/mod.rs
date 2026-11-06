//! ARM REST transport shared by every Azure caller: the error type, the
//! bearer-token source and the [`ArmClient`] built on top of them. Which
//! Resource Manager it reaches is the deployment's declared cloud
//! (`remote::azure_cloud`).

mod client;
mod verbs;

pub use client::ArmClient;
pub(crate) use verbs::vm_path;

/// Compute RP API version for VM resource paths.
pub(crate) const COMPUTE_API_VERSION: &str = "2023-09-01";
pub(super) const NETWORK_API_VERSION: &str = "2023-09-01";

/// Azure auth/transport/API error. A refusal ARM states is [`Refused`]
/// with its HTTP status and `error.code`, so a caller decides by the code
/// ARM documents, never by words in its message.
///
/// [`Refused`]: AzureError::Refused
#[derive(Debug, thiserror::Error)]
pub enum AzureError {
    /// Token acquisition failed (or every chain source unavailable).
    #[error("no Azure credentials: {0}")]
    Auth(String),
    /// Transport failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// An answer that is not what the call asks for, quoted whole.
    #[error("{0}")]
    Api(String),
    /// ARM refused the request (`status`) or its long-running operation
    /// ended Failed or Canceled (no status), with the `error.code` ARM
    /// stated, if any, and the whole refusal as read.
    #[error("{detail}")]
    Refused {
        status: Option<u16>,
        code: Option<String>,
        detail: String,
    },
}

impl AzureError {
    /// The `error.code` ARM stated for this refusal; none for any other
    /// failure.
    pub fn arm_code(&self) -> Option<&str> {
        if let Self::Refused {
            code: Some(code), ..
        } = self
        {
            Some(code.as_str())
        } else {
            None
        }
    }
}

// --- Managed-identity / Skarbiec token source ---

/// Fresh bearer token for the declared cloud's Resource Manager, from the
/// shared chain's per-scope cache.
async fn bearer_token(http: &reqwest::Client) -> Result<String, AzureError> {
    let cloud = crate::remote::azure_cloud::declared().map_err(AzureError::Auth)?;
    crate::remote::azure_token::bearer_token(http, &cloud.scope(), &cloud.resource_manager)
        .await
        .map_err(|err| match err {
            crate::remote::azure_token::TokenError::Auth(msg) => AzureError::Auth(msg),
            crate::remote::azure_token::TokenError::Http(err) => AzureError::Http(err),
        })
}
