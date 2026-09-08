use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::RwLock;

use crate::monitor::host_silence;
use crate::service_resolution::{self, ResolvedService, ResolverAdapter, ResolverConfig};
use crate::targets::{self, RegistryStore};

use crate::cli::resolver::authority::paths::select_resolver_ssh_path;
use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::resolver::directory::source::snapshot_source;
use crate::cli::resolver::directory::source::SnapshotSource;
use crate::cli::resolver::report::published::now_iso;
use crate::cli::resolver::report::published::publish;
use crate::cli::resolver::report::published::PublishedState;

pub(super) struct Snapshot {
    pub(super) document: Value,
    pub(super) store_version: String,
    pub(super) directory_generation: u64,
    pub(super) loaded_at: Instant,
    /// The same instant as a wall clock, because [`PublishedState`] is read by
    /// another process and a monotonic `Instant` means nothing there.
    pub(super) loaded_at_iso: String,
}

pub(super) struct ResolverState {
    pub(super) local_store: Option<Arc<RegistryStore>>,
    pub(super) source: RwLock<SnapshotSource>,
    pub(super) snapshot: RwLock<Snapshot>,
    pub(super) max_stale: Duration,
    pub(super) local_target: String,
    pub(super) adapters: Vec<ResolverAdapter>,
    pub(super) config: ResolverConfig,
    /// One forward per `destination|host:port`, opened on first use.
    pub(super) tunnels: tokio::sync::Mutex<std::collections::HashMap<String, Tunnel>>,
    /// Why this host last refused to refresh its last-known-good registry
    /// copy, as [`targets::LastGoodRefusal::kind`].
    ///
    /// Published, not just logged. This process reads the authority every
    /// few seconds and is the one thing on the host that would notice the
    /// fallback going stale; a refusal that only reached stderr left the
    /// operator's `resolver status` vouching for a host whose recovery copy
    /// had stopped advancing.
    pub(super) last_good_refusal: RwLock<Option<&'static str>>,
}

impl ResolverState {
    /// A connection to `host:port` behind the first of `paths` that answers,
    /// over the forward this resolver keeps for that pair, opening it if this
    /// is its first use.
    ///
    /// Opening holds the pool lock, so two requests for a cold destination
    /// cannot race into two forwards. Openings are rare; a warm destination
    /// only reads the port.
    ///
    /// Path selection is part of opening, not part of connecting. It used to
    /// run in `proxy_connection` ahead of this call, so every accepted
    /// connection to a host that declares an `ssh_fallbacks` entry -- as
    /// `charless-mac-mini` declares `lan` -- paid one `ssh <destination> true`
    /// process, bounded at twenty seconds, before any traffic moved. That is
    /// the process per request this transport exists to remove, and it also
    /// left a control master up for the forward to be handed to. Selecting
    /// under the pool lock costs one probe per forward instead of one per
    /// request, and a warm destination costs none.
    pub(super) async fn tunnel_connect(
        &self,
        active_host: &str,
        paths: &[targets::SshConnectionPath],
        host: &str,
        port: u16,
    ) -> Result<TcpStream, String> {
        // Keyed by the host, not by a destination: which of its paths answers
        // is chosen while the forward is opened, and one forward per host is
        // the point.
        let key = format!("{active_host}|{host}:{port}");
        // Two attempts: a forward that died between the liveness check and the
        // dial, or a local port some other process took, is retried once
        // against a freshly opened one. A second failure is the answer.
        for attempt in 0..2 {
            let (local, destination) = {
                let mut tunnels = self.tunnels.lock().await;
                let live = tunnels
                    .get_mut(&key)
                    .and_then(|tunnel| tunnel.usable().then_some(tunnel.local));
                match live {
                    Some(local) => (local, None),
                    None => {
                        // Dropping the entry kills the dead child.
                        tunnels.remove(&key);
                        let path = select_resolver_ssh_path(paths).await?;
                        let tunnel = Tunnel::open(&path.destination, host, port).await?;
                        let local = tunnel.local;
                        tunnels.insert(key.clone(), tunnel);
                        (local, Some(path.destination.clone()))
                    }
                }
            };
            match TcpStream::connect(("127.0.0.1", local)).await {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    self.tunnels.lock().await.remove(&key);
                    if attempt == 1 {
                        let named = destination.as_deref().unwrap_or(active_host);
                        return Err(format!(
                            "the SSH forward to {named} for {host}:{port} refused a \
                             connection on 127.0.0.1:{local}: {error}"
                        ));
                    }
                }
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    pub(super) async fn refresh(&self) -> Result<bool, String> {
        let source = self.source.read().await.clone();
        let (document, store_version, generation) =
            source.fetch(host_silence::READER_RESOLVER).await?;
        let serialized = serde_json::to_string(&document)
            .map_err(|error| format!("cannot serialize validated registry snapshot: {error}"))?;
        // A refused cache does not fail the refresh: the document this
        // process just read is good and serving it is the job. It does get
        // recorded, so `publish_serving` carries it to `resolver status`.
        match targets::store_last_good(&serialized, &store_version) {
            Ok(()) => *self.last_good_refusal.write().await = None,
            Err(refusal) => *self.last_good_refusal.write().await = Some(refusal.kind()),
        }
        let next_source = snapshot_source(self.local_store.clone(), &document, &self.local_target)?;
        let next_config = service_resolution::resolver_config(&document, &self.local_target)?;
        if next_config != self.config {
            return Ok(true);
        }
        let mut current = self.snapshot.write().await;
        if generation < current.directory_generation {
            return Err(format!(
                "service directory rollback rejected: generation {generation} < {}",
                current.directory_generation
            ));
        }
        if generation == current.directory_generation
            && service_resolution::directory(&document)?
                != service_resolution::directory(&current.document)?
        {
            return Err(format!(
                "service directory changed without advancing generation {generation}"
            ));
        }
        current.document = document;
        current.store_version = store_version;
        current.directory_generation = generation;
        current.loaded_at = Instant::now();
        current.loaded_at_iso = now_iso();
        drop(current);
        *self.source.write().await = next_source;
        Ok(false)
    }

    pub(super) async fn resolve(
        &self,
        service: &str,
        consumer: &str,
    ) -> Result<ResolvedService, String> {
        let current = self.snapshot.read().await;
        if current.loaded_at.elapsed() <= self.max_stale {
            return service_resolution::resolve(&current.document, service, consumer);
        }

        let sentence = format!(
            "service directory cache is stale (store generation {})",
            current.store_version
        );
        drop(current);
        // Staleness is published as a degraded state, but a structurally
        // validated last-known-good route remains usable. Refusing it made a
        // short authority outage recursive: the object adapter shut, so the
        // authority could no longer carry the registry snapshot that would
        // reopen the adapter. Keep serving the known route while the refresh
        // loop retries and record the condition against the authority host.
        let subject = self.source.read().await.subject_host(&self.local_target);
        host_silence::report_refusal_detached(
            subject,
            host_silence::READER_RESOLVER,
            host_silence::REASON_DIRECTORY_CACHE_STALE,
            sentence,
        );
        let current = self.snapshot.read().await;
        service_resolution::resolve(&current.document, service, consumer)
    }

    pub(super) fn gateway_url(&self, service: &str, consumer: &str) -> Option<String> {
        self.adapters
            .iter()
            .find(|adapter| adapter.service == service && adapter.consumer == consumer)
            .map(|adapter| format!("http://{}", adapter.bind))
    }

    /// Publish what this process holds right now.
    pub(super) async fn publish_serving(&self) {
        let current = self.snapshot.read().await;
        publish(&PublishedState::serving(
            &self.local_target,
            current.directory_generation,
            &current.store_version,
            &current.loaded_at_iso,
            *self.last_good_refusal.read().await,
        ));
    }
}
