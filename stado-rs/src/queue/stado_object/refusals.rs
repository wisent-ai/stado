//! The refusals the gateway answers with, and which of them are a window
//! rather than a verdict.
//!
//! Two status-and-body pairs are ridden out; every other answer is turned
//! into a [`StorageError`] on the first refusal.

use reqwest::{Response, StatusCode};

use crate::queue::StorageError;

use super::StadoObjectBackend;

impl StadoObjectBackend {
    /// The body the object gateway answers with while its authorization
    /// boundary is closed. Matched on the body and not on the status alone:
    /// `503` is also how an ingress in front of the gateway says it has no
    /// upstream, and that is not a window anything should wait out.
    const BOUNDARY_UNAVAILABLE_BODY: &'static str = "object authorization unavailable";

    /// The body the resolver's forward answers with while it has no channel to
    /// the host that serves this store — [`crate::cli::resolver`]'s
    /// `UPSTREAM_REFUSAL`, sent as `502` before the prompt close.
    ///
    /// Also a window, not a verdict, and for the same reason as the boundary
    /// above: the forward is opening, or has just been re-opened after going
    /// cold, and the next attempt reaches the store. Left unretried it decided
    /// releases. On 2026-09-03 the 0.14.0 train published both platforms and
    /// then died in delivery on `registry store unreachable (stado:registry.json):
    /// Stado object API error HTTP 502: upstream unavailable`, and `stado host
    /// reclaim` reported the queue store unreadable off the same answer while
    /// the very next read of the same prefix succeeded.
    const FORWARD_UNAVAILABLE_BODY: &'static str = "upstream unavailable";

    /// Attempts, including the first, before a closed boundary is reported.
    const BOUNDARY_ATTEMPTS: usize = 6;

    /// Send a request the gateway may refuse while it revalidates its grants.
    ///
    /// The object gateway reads its verifier grants from Skarbiec and answers
    /// `503 {"error":"object authorization unavailable"}` for as long as that
    /// read is in flight — the same mechanism the dashboard logs as
    /// "integration authorization boundary is closed; revalidating inline". It
    /// is a window, not a verdict. `deploy.yml`'s Linux publisher has ridden it
    /// out with twelve tries for as long as it has existed ("The writer may
    /// briefly reload authorization"), and every other caller in the fleet died
    /// on the first refusal: the `weles-worker 0.5.26` submission ended at
    /// 2026-08-29T23:02:53Z on exactly this body, three seconds after the same
    /// client had read a storage state successfully through the same gateway.
    ///
    /// Exactly two status-and-body pairs are retried, and only
    /// [`Self::BOUNDARY_ATTEMPTS`] times with a linear backoff: the closed
    /// authorization boundary above, and the forward's own
    /// [`Self::FORWARD_UNAVAILABLE_BODY`] under `502`. Any other body under
    /// either status, and every other status, reaches the caller unchanged — a
    /// gateway that is genuinely unauthorized, and a proxy that is genuinely
    /// pointed at nothing, must still say so on the first answer.
    pub(super) async fn send_through_boundary(
        builder: reqwest::RequestBuilder,
    ) -> Result<Response, StorageError> {
        let Some(mut candidate) = builder.try_clone() else {
            // A streaming body cannot be replayed, so there is nothing to retry
            // with; the caller gets the gateway's first answer.
            return Ok(builder.send().await?);
        };
        for attempt in 1..=Self::BOUNDARY_ATTEMPTS {
            let response = candidate.send().await?;
            let transient = matches!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE | StatusCode::BAD_GATEWAY
            );
            if !transient || attempt == Self::BOUNDARY_ATTEMPTS {
                return Ok(response);
            }
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            let window = body.contains(Self::BOUNDARY_UNAVAILABLE_BODY)
                || body.contains(Self::FORWARD_UNAVAILABLE_BODY);
            if !window {
                return Err(StorageError::Stado { status, body });
            }
            let Some(next) = builder.try_clone() else {
                return Err(StorageError::Stado { status, body });
            };
            candidate = next;
            tokio::time::sleep(std::time::Duration::from_secs(attempt as u64)).await;
        }
        // The loop returns on its last attempt, so this is unreachable while
        // BOUNDARY_ATTEMPTS is non-zero; stated rather than left to a panic.
        Err(StorageError::Other(
            "Stado object API boundary retry made no attempt".to_string(),
        ))
    }

    pub(super) async fn response_error(response: Response) -> StorageError {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        StorageError::Stado { status, body }
    }
}
