//! One response body of the object route read to its end, as a wait: the
//! read is said on stderr before its first byte is awaited (`crate::wait`)
//! and ends with how it ended, so a body that stops arriving after its
//! headers is named instead of holding a command at no CPU.
//!
//! A body whose bytes stop arriving is a stall, measured against the one
//! progress window the object route declares: the refresh interval of this
//! host's resolver ([`crate::cli::resolver::bound_seconds`]), the interval
//! the route's sends are already abandoned at. No byte for that long ends
//! the read as stalled, naming the bytes it had and the interval; a host
//! that declares no interval has no measure, and the read waits, as its
//! sends do.

use std::time::Duration;

use crate::cli::storage::*;

/// How a body read ended.
pub(in crate::cli::storage) enum Ended {
    /// The stream ended: the caller compares what arrived with what it was
    /// promised.
    Closed,
    /// No byte arrived for one progress window, or the connection broke;
    /// the sentence names which and how many bytes had arrived.
    Broken(String),
}

/// Read `response`'s body onto `body`, at most `limit` bytes past what it
/// held when called (none for an unbounded read), as `what` (the operation
/// and object) on the route `url`. More bytes than `limit` is the refusal it
/// always was: the route sent bytes outside what was asked.
pub(in crate::cli::storage) async fn read(
    mut response: reqwest::Response,
    what: &str,
    body: &mut Vec<u8>,
    limit: Option<usize>,
) -> Result<Ended, CmdError> {
    let url = response.url().clone();
    let window = crate::cli::resolver::bound_seconds(&crate::cli::resolver::Route::Address(&url))
        .map_err(|unreadable| {
        CmdError::unreachable(format!(
            "{what}: its body is not read, because the progress window that measures it \
                 cannot be read: {unreadable}"
        ))
    })?;
    let start = body.len();
    let waiting = crate::wait::begin(
        crate::wait::Kind::ObjectApi,
        format!(
            "{what}: the body from byte {start}{}",
            limit.map_or(String::new(), |limit| format!(", {limit} bytes"))
        ),
        url.authority(),
    );
    loop {
        let next = response.chunk();
        let chunk = match &window {
            Some(bound) => {
                match tokio::time::timeout(Duration::from_secs(bound.seconds), next).await {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        let said = format!(
                            "{what}: no byte arrived for {}s, the refresh interval {}, after {} \
                         bytes of this body; `stado resolver status` lists what the adapter at \
                         {} holds",
                            bound.seconds,
                            bound.declared,
                            body.len() - start,
                            url.authority()
                        );
                        waiting.failed(&said);
                        return Ok(Ended::Broken(said));
                    }
                }
            }
            None => next.await,
        };
        match chunk {
            Ok(Some(chunk)) => {
                if limit.is_some_and(|limit| chunk.len() > limit - (body.len() - start)) {
                    let said = format!("{what} sent bytes outside the requested range");
                    waiting.failed(&said);
                    return Err(CmdError::click(said)
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => {
                waiting.done();
                return Ok(Ended::Closed);
            }
            Err(error) => {
                let said = format!(
                    "{what}: the connection closed after {} bytes of this body: {error}",
                    body.len() - start
                );
                waiting.failed(&said);
                return Ok(Ended::Broken(said));
            }
        }
    }
}
