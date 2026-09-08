//! The non-success answers, and the one shape they are all lifted into.
//!
//! Nothing in this backend inspects a status twice: a route matches the
//! statuses that are part of its contract (404 as absent, 409 and 412 as a
//! lost race) and hands everything else here, where the operation that
//! provoked it and a truncated body become one [`StorageError`].

use crate::queue::StorageError;

use super::AzureBlobBackend;

impl AzureBlobBackend {
    /// Lift a non-success response into [`StorageError::Other`], truncating
    /// the body like the provider's ARM error surface.
    pub(super) async fn api_error(response: reqwest::Response, op: &str) -> StorageError {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        StorageError::Other(format!(
            "Azure blob {op} -> HTTP {status}: {}",
            text.chars().take(280).collect::<String>()
        ))
    }

    /// Pass through success; anything else becomes an error via
    /// [`Self::api_error`].
    pub(super) async fn ensure_success(
        response: reqwest::Response,
        op: &str,
    ) -> Result<reqwest::Response, StorageError> {
        if response.status().is_success() {
            return Ok(response);
        }
        Err(Self::api_error(response, op).await)
    }
}
