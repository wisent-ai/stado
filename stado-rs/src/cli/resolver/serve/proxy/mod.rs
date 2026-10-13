//! The listening side of the resolver: accept connections and hand each one on.

use std::sync::Arc;

use tokio::net::TcpListener;

use crate::cli::resolver::serve::state::ResolverState;
use crate::service_resolution::ResolverAdapter;

/// Accept connections and proxy each one. An accept that fails for want of
/// file descriptors is the held connections' doing: every relay older than
/// the refresh interval is ended with that sentence ([`shed`]), which frees
/// their descriptors, and the adapter accepts again. Any other accept
/// failure, or one that no held relay can be shed for, ends the adapter
/// with the accept's own error, which the resolver publishes as `failed`.
pub(super) async fn serve_adapter(
    listener: TcpListener,
    adapter: ResolverAdapter,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    loop {
        let (client, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) if shed::out_of_descriptors(&error) => {
                shed_or_fail(&adapter, &state, &error)?;
                continue;
            }
            Err(error) => return Err(format!("{} accept failed: {error}", adapter.bind)),
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

/// Shed the held relays, saying which and why; refuse to go on when none
/// older than the refresh interval could be shed, because then the
/// descriptors are held by something this loop cannot free.
fn shed_or_fail(
    adapter: &ResolverAdapter,
    state: &ResolverState,
    error: &std::io::Error,
) -> Result<(), String> {
    let refresh_seconds = match state.config.read() {
        Ok(config) => config.refresh_seconds,
        Err(poisoned) => poisoned.into_inner().refresh_seconds,
    };
    let reason = format!(
        "this host's resolver ran out of file descriptors accepting on {} ({error}): this \
         connection had waited longer than the {refresh_seconds}s refresh interval for an answer \
         that never came, so the resolver ended it to accept again",
        adapter.bind
    );
    let shed = shed::shed_held(refresh_seconds, &reason);
    for open in &shed.ended {
        eprintln!(
            "stado resolver adapter service={} consumer={} shed the wait of {} to {} on {:?} \
             held since {}",
            open.service, open.consumer, open.client, open.endpoint, open.active_host, open.since
        );
    }
    if shed.ended.is_empty() {
        return Err(format!(
            "{} accept failed: {error}; no relay older than the {refresh_seconds}s refresh \
             interval could be shed ({} held wait(s) stand on channels still opening), so the \
             descriptors are held by something this adapter cannot free",
            adapter.bind, shed.unsheddable
        ));
    }
    eprintln!(
        "stado resolver adapter service={} consumer={} shed {} held relay(s) after {} and accepts \
         again; {} held wait(s) stand on channels still opening",
        adapter.service,
        adapter.consumer,
        shed.ended.len(),
        error,
        shed.unsheddable
    );
    Ok(())
}

mod connection;
mod refusal;
mod relay;
mod shed;

pub(super) use shed::shed_replaced;

use connection::proxy_connection;
