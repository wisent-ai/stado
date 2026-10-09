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
    /// every other refusal. A gateway reached through this host's resolver
    /// whose adapter already holds a connection unanswered past the
    /// directory refresh interval is refused before the request is sent
    /// ([`StorageError::Held`]): a request sent then would stand behind the
    /// held ones for as long as they stand, and the registry read behind
    /// every command stood there for hours.
    pub(super) async fn send_through_boundary(
        builder: reqwest::RequestBuilder,
    ) -> Result<Response, StorageError> {
        let (client, request) = builder.build_split();
        let request = request?;
        match crate::cli::resolver::held_at(request.url()) {
            Ok(None) => {}
            Ok(Some(held)) => {
                return Err(StorageError::Held(format!(
                    "{} {} not sent: {held}",
                    request.method(),
                    request.url()
                )))
            }
            Err(unreadable) => {
                return Err(StorageError::Held(format!(
                    "{} {} not sent: whether this host's resolver holds that adapter cannot be \
                     read: {unreadable}",
                    request.method(),
                    request.url()
                )))
            }
        }
        Ok(wait::send_built(Kind::ObjectApi, "object API", client, request).await?)
    }

    pub(super) async fn response_error(response: Response) -> StorageError {
        let status = response.status().as_u16();
        let url = response.url().to_string();
        let body = response.text().await.unwrap_or_default();
        StorageError::Stado { status, url, body }
    }
}
