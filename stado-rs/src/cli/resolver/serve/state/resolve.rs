//! Resolving one service for one consumer from the snapshot this resolver
//! holds, refusing a route that would loop back into its own listeners.

use crate::monitor::host_silence;
use crate::service_resolution::{self, ResolvedService};

use super::ResolverState;

impl ResolverState {
    /// The route `service` gives `consumer`: from the snapshot while it is
    /// fresh, and from the retained one, with the staleness reported against
    /// the authority, once it is not.
    pub(in crate::cli::resolver::serve) async fn resolve(
        &self,
        service: &str,
        consumer: &str,
    ) -> Result<ResolvedService, String> {
        let current = self.snapshot.read().await;
        if current.loaded_at.elapsed() <= self.max_stale {
            let resolved = service_resolution::resolve(&current.document, service, consumer)
                .map_err(String::from)?;
            return self.refuse_own_listener(resolved);
        }

        let sentence = format!(
            "service directory cache is stale (store generation {})",
            current.store_version
        );
        drop(current);
        // Publish staleness as degraded while serving the validated retained
        // route. Refusing it can close the adapter the authority itself needs
        // to refresh this document. Record the condition against that host.
        let subject = self.source.read().await.subject_host(&self.local_target);
        host_silence::report_refusal_detached(
            subject,
            host_silence::READER_RESOLVER,
            host_silence::REASON_DIRECTORY_CACHE_STALE,
            sentence,
        );
        let current = self.snapshot.read().await;
        let resolved = service_resolution::resolve(&current.document, service, consumer)
            .map_err(String::from)?;
        self.refuse_own_listener(resolved)
    }

    /// A route placed on this host whose endpoint is one of this process's
    /// own listeners - an adapter bind or the resolver API - would be
    /// relayed back into this process: the connection is accepted, forwarded
    /// to the same socket, accepted again, and the client waits on a chain
    /// that never reaches a server, with nothing to say so. Refused here,
    /// naming the route and the listener, before any channel is opened.
    fn refuse_own_listener(&self, resolved: ResolvedService) -> Result<ResolvedService, String> {
        if resolved.active_host != self.local_target {
            return Ok(resolved);
        }
        let endpoint = url::Url::parse(&resolved.endpoint.url)
            .map_err(|error| format!("invalid resolved endpoint: {error}"))?;
        let (Some(host), Some(port)) = (endpoint.host_str(), endpoint.port_or_known_default())
        else {
            return Ok(resolved);
        };
        let Some(target) = crate::service_resolution::listener_address(host, port) else {
            return Ok(resolved);
        };
        let mut listeners: Vec<(String, String)> = self
            .adapters
            .read()
            .map(|adapters| {
                adapters
                    .iter()
                    .map(|adapter| {
                        (
                            format!(
                                "adapter {} for consumer {}",
                                adapter.service, adapter.consumer
                            ),
                            adapter.bind.clone(),
                        )
                    })
                    .collect()
            })
            .map_err(|_poisoned| "the adapter list is poisoned".to_string())?;
        if let Ok(config) = self.config.read() {
            listeners.push(("the resolver API".to_string(), config.api_bind.clone()));
        }
        for (what, bind) in listeners {
            let Some((bind_host, bind_port)) = bind.rsplit_once(':') else {
                continue;
            };
            let Ok(bind_port) = bind_port.parse::<u16>() else {
                continue;
            };
            if crate::service_resolution::listener_address(bind_host, bind_port) == Some(target) {
                return Err(format!(
                    "service {} resolves to {}, which is {what} ({bind}) of this resolver on {}: \
                     the service directory must carry the address the service listens on, not \
                     the stable address published for its clients; relaying it would loop back \
                     into this process and no client would ever be answered",
                    resolved.name, resolved.endpoint.url, self.local_target
                ));
            }
        }
        Ok(resolved)
    }
}
