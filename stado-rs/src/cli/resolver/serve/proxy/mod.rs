//! The listening side of the resolver: accept connections and hand each one on.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::service_resolution::ResolverAdapter;
use crate::cli::resolver::serve::state::ResolverState;

/// How many consecutive seconds a listener may refuse before it is reported
/// broken rather than pinched.
pub(super) const ACCEPT_FAILURE_LIMIT: u32 = 60;

/// Wait out an accept failure instead of dying of it.
///
/// `accept` answering `EMFILE` says the process is momentarily out of
/// descriptors, which every long-lived server survives by waiting. Returning it
/// killed the resolver instead: launchd counted 166 runs of
/// `com.wisent.stado-resolver` on this workstation, each restart dropping every
/// connection in flight, and four separate `release submit` runs died with
/// `error sending request for url (http://127.0.0.1:18776/...)` in the middle
/// of a publication because of it. One connection's resource error must not
/// close the door for all of them.
///
/// The errno itself is not named, because none of the interesting ones have a
/// stable `ErrorKind` and hard-coding platform numbers is a second thing to be
/// wrong. A failure is waited out and retried; a listener that refuses without
/// pause for [`ACCEPT_FAILURE_LIMIT`] consecutive attempts is the one reported
/// broken, which no descriptor pinch survives and a dead socket always is.
pub(super) async fn accept_backoff(bind: &str, error: &std::io::Error, failures: u32) {
    eprintln!(
        "stado resolver {bind} accept deferred ({failures}/{ACCEPT_FAILURE_LIMIT}), \
         retrying in 1s: {error}"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
}

pub(super) async fn serve_adapter(
    listener: TcpListener,
    adapter: ResolverAdapter,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    let mut failures = 0_u32;
    loop {
        let (client, _) = match listener.accept().await {
            Ok(accepted) => {
                failures = 0;
                accepted
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures >= ACCEPT_FAILURE_LIMIT {
                    return Err(format!(
                        "{} accept failed {failures} times in a row: {error}",
                        adapter.bind
                    ));
                }
                accept_backoff(&adapter.bind, &error, failures).await;
                continue;
            }
        };
        let state = Arc::clone(&state);
        let adapter = adapter.clone();
        tokio::spawn(async move {
            if let Err(error) = proxy_connection(client, &adapter, &state).await {
                eprintln!(
                    "stado resolver adapter service={} consumer={} rejected connection: {}",
                    adapter.service, adapter.consumer, error
                );
            }
        });
    }
}


mod connection;
mod idle;
mod refusal;

use connection::proxy_connection;
