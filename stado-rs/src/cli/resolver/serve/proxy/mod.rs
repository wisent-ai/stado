//! The listening side of the resolver: accept connections and hand each one on.

use std::sync::Arc;

use tokio::net::TcpListener;

use crate::cli::resolver::serve::state::ResolverState;
use crate::service_resolution::ResolverAdapter;

/// Accept connections and proxy each one. An accept that fails ends the
/// adapter with the accept's own error, which the resolver publishes as
/// `failed`.
pub(super) async fn serve_adapter(
    listener: TcpListener,
    adapter: ResolverAdapter,
    state: Arc<ResolverState>,
) -> Result<(), String> {
    loop {
        let (client, _) = listener
            .accept()
            .await
            .map_err(|error| format!("{} accept failed: {error}", adapter.bind))?;
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
mod refusal;

use connection::proxy_connection;
