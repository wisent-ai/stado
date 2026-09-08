//! The non-success answers, and the one shape they are all lifted into.
//!
//! Nothing in this backend inspects a status twice: a route matches the
//! statuses that are part of its contract (404 as absent, 412 as a lost
//! precondition race) and hands every other answer here, where the status
//! and the body it came with become one [`StorageError`].

use crate::queue::StorageError;

/// Return the response on success, otherwise lift the status + body into
/// [`StorageError::Gcs`].
pub(super) async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, StorageError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Err(StorageError::Gcs { status, body })
}
