use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::RwLock;

use crate::monitor::host_silence;
use crate::service_resolution::{self, ResolvedService, ResolverAdapter, ResolverConfig};
use crate::targets::{self, RegistryStore};

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
    /// Native SSH sessions, shared without helper processes or intermediary listeners.
    pub(super) tunnels: tokio::sync::Mutex<std::collections::HashMap<String, Arc<Tunnel>>>,
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
    /// Open a channel and retain its session until the caller finishes copying.
    ///
    /// Connections through this adapter used to wait minutes while the
    /// remote object API refused channels at once. Two things made that
    /// queue: every refused channel dropped a healthy session, so the next
    /// connection opened a new SSH session from scratch; and every open
    /// ran while holding the one lock all services and consumers share, so
    /// each connection waited for every handshake queued before it. A
    /// session is now opened without the shared lock, and it is dropped only
    /// when it is closed; a channel the service refuses leaves it in place and
    /// is reported to the caller at once.
    pub(super) async fn tunnel_connect(
        &self,
        active_host: &str,
        paths: &[targets::SshConnectionPath],
        host: &str,
        port: u16,
    ) -> Result<(russh::ChannelStream<russh::client::Msg>, Arc<Tunnel>), String> {
        let key = format!("{active_host}|{host}:{port}");
        // Preserve the existing single reconnect before any client bytes are sent.
        for attempt in 0..2 {
            let held = {
                let mut tunnels = self.tunnels.lock().await;
                match tunnels.get(&key).filter(|tunnel| tunnel.usable()) {
                    Some(tunnel) => Some(Arc::clone(tunnel)),
                    None => {
                        tunnels.remove(&key);
                        None
                    }
                }
            };
            let tunnel = match held {
                Some(tunnel) => tunnel,
                None => {
                    let opened = Arc::new(Tunnel::open(paths).await?);
                    let mut tunnels = self.tunnels.lock().await;
                    // Another connection may have opened one meanwhile; keep
                    // the first so every caller shares one session.
                    let kept = tunnels
                        .entry(key.clone())
                        .or_insert_with(|| Arc::clone(&opened));
                    Arc::clone(kept)
                }
            };
            match tunnel.connect(host, port).await {
                Ok(stream) => return Ok((stream, tunnel)),
                // The session is alive: the service end refused this one
                // channel, and a new session would be refused the same way.
                Err(error) if tunnel.usable() => return Err(error),
                Err(error) => {
                    let mut tunnels = self.tunnels.lock().await;
                    if tunnels
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &tunnel))
                    {
                        tunnels.remove(&key);
                    }
                    if attempt == 1 {
                        return Err(error);
                    }
                }
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    pub(super) async fn refresh(&self) -> Result<bool, String> {
        let source = self.source.read().await.clone();
        let (document, store_version, generation) = source
            .fetch(host_silence::READER_RESOLVER)
            .await
            .map_err(|error| error.to_string())?;
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
        let advanced = generation > current.directory_generation;
        current.document = document;
        current.store_version = store_version;
        current.directory_generation = generation;
        current.loaded_at = Instant::now();
        current.loaded_at_iso = now_iso();
        drop(current);
        *self.source.write().await = next_source;
        // Said after the markers are kept, so a reader of this line knows
        // this host's markers already follow that generation.
        self.keep_markers().await;
        if advanced {
            eprintln!("stado resolver loaded directory generation {generation}");
        }
        Ok(false)
    }

    /// Write the forward markers the loaded directory declares for this host.
    ///
    /// This process reads the directory on every refresh and is the one
    /// thing on each host that always runs, so it is the writer that keeps
    /// `~/.stado/forwards` true; a failure is logged and the next refresh
    /// tries again, because the markers are not what this process serves.
    pub(super) async fn keep_markers(&self) {
        let current = self.snapshot.read().await;
        match crate::cli::directory::keep_declared_markers(&current.document, &self.local_target) {
            Ok(written) => {
                for marker in written {
                    eprintln!("stado resolver wrote forward marker {marker}");
                }
            }
            Err(error) => eprintln!("stado resolver could not keep forward markers: {error}"),
        }
    }

    pub(super) async fn resolve(
        &self,
        service: &str,
        consumer: &str,
    ) -> Result<ResolvedService, String> {
        let current = self.snapshot.read().await;
        if current.loaded_at.elapsed() <= self.max_stale {
            return service_resolution::resolve(&current.document, service, consumer)
                .map_err(String::from);
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
        service_resolution::resolve(&current.document, service, consumer).map_err(String::from)
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
