//! The refusals the gateway answers with.
//!
//! Every answer reaches the caller on the first refusal, as a
//! [`StorageError`] carrying the gateway's own status and body.

use reqwest::Response;

use crate::cli::resolver::{unless_held, Route};
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
    /// every command stood there for hours. A request sent before the
    /// adapter stood is abandoned once the resolver publishes it held, with
    /// the same refusal naming the address ([`unless_held`]).
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
        let address = request.url().clone();
        let sent = wait::send_built(Kind::ObjectApi, "object API", client, request);
        match unless_held(Route::Address(&address), sent).await {
            Ok(Ok(answer)) => Ok(answer),
            // A refused connection on one of this host's resolver adapters
            // is the resolver not serving that port, not the store being
            // down: the store was never asked. The resolver published why,
            // and that reason is what the caller needs, not a loopback port
            // number nobody can place.
            // Kept as `Held`: the store was not asked, as for a held adapter,
            // so it stays `infra_down` and the agent tick still reads it as
            // the store not answering.
            Ok(Err(error)) if error.is_connect() => {
                let origin = address.origin().ascii_serialization();
                match crate::cli::resolver::not_serving_reason(&origin) {
                    Some(reason) => Err(StorageError::Held(format!(
                        "{} {origin} refused the connection, because {reason}; `stado resolver \
                         status` names the resolver's state and every adapter ({})",
                        address.as_str(),
                        crate::cli::http_failure(&error)
                    ))),
                    None => Err(StorageError::Http(error)),
                }
            }
            Ok(Err(error)) => Err(StorageError::Http(error)),
            Err(held) => Err(StorageError::Held(held)),
        }
    }

    pub(super) async fn response_error(response: Response) -> StorageError {
        let status = response.status().as_u16();
        let url = response.url().to_string();
        let body = crate::primitives::http::refused_body(response).await;
        StorageError::Stado { status, url, body }
    }
}
