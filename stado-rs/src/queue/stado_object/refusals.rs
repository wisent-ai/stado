//! The refusals the gateway answers with.
//!
//! Every answer reaches the caller on the first refusal, as a
//! [`StorageError`] carrying the gateway's own status and body.

use reqwest::Response;

use crate::queue::StorageError;
use crate::wait::{self, Kind};

use super::StadoObjectBackend;

impl StadoObjectBackend {
    /// Send one request to the object gateway, saying on stderr which
    /// request it is and where before the answer is awaited, so a gateway
    /// that never answers is named instead of looking like a hung command.
    /// A closed authorization boundary (`503 object authorization
    /// unavailable`) and a forward with no channel (`502 upstream
    /// unavailable`) reach the caller as the gateway's own answer, like
    /// every other refusal.
    pub(super) async fn send_through_boundary(
        builder: reqwest::RequestBuilder,
    ) -> Result<Response, StorageError> {
        Ok(wait::send(Kind::ObjectApi, "object API", builder).await?)
    }

    pub(super) async fn response_error(response: Response) -> StorageError {
        let status = response.status().as_u16();
        let url = response.url().to_string();
        let body = response.text().await.unwrap_or_default();
        StorageError::Stado { status, url, body }
    }
}
