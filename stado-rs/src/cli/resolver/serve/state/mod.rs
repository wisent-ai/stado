use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::RwLock;

use crate::service_resolution::{self, ResolverAdapter, ResolverConfig};

mod handover;
mod resolve;
mod session;
mod tunnels;
use crate::targets::{self, RegistryStore};

use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::resolver::directory::source::snapshot_source;
use crate::cli::resolver::directory::source::SnapshotSource;
use crate::cli::resolver::report::published::now_iso;
use crate::cli::resolver::report::published::publish;
use crate::cli::resolver::report::published::PublishedAdapter;
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
    /// The adapters this process listens on now. Replaced in place when the
    /// directory declares another set ([`super::reconcile_adapters`]), so a
    /// change does not end the process and every connection with it.
    pub(super) adapters: std::sync::RwLock<Vec<ResolverAdapter>>,
    /// The declared adapters this process does not serve, each with its
    /// reason ([`super::adapters::bind_adapters`]); published beside the
    /// served ones so a reader of the route meets the reason.
    pub(super) unserved: std::sync::RwLock<Vec<PublishedAdapter>>,
    /// Whether the adapter on this host's own store address is left
    /// unserved ([`super::adapters::self_served_store`]).
    pub(super) refuses_store_adapter: bool,
    pub(super) config: std::sync::RwLock<ResolverConfig>,
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
    /// Reload the directory. `Some(config)` when the directory now declares
    /// another resolver configuration for this host, after the snapshot has
    /// been taken: the caller rebinds what changed and keeps serving.
    pub(super) async fn refresh(&self) -> Result<Option<ResolverConfig>, String> {
        let (document, store_version, generation) = self.read_registry().await?;
        let serialized = serde_json::to_string(&document)
            .map_err(|error| format!("cannot serialize validated registry snapshot: {error}"))?;
        // The copy is recorded only once the generation is judged below: a
        // copy recorded before the rollback refusal took the old authority's
        // lower generation, and every one-shot command reading that copy
        // went back to the authority the role had moved away from.
        let next_source = snapshot_source(self.local_store.clone(), &document, &self.local_target)?;
        let next_config = service_resolution::resolver_config(&document, &self.local_target)?;
        // A lock a panicking writer left poisoned still holds the last
        // configuration it wrote; that is the one compared.
        let changed = match self.config.read() {
            Ok(current) => *current != next_config,
            Err(poisoned) => *poisoned.into_inner() != next_config,
        };
        let mut current = self.snapshot.write().await;
        // A generation below the held one is refused, and the refusal names
        // both sides' store versions and when the held one was loaded, so a
        // reader can tell a real rollback of the authority from a generation
        // this host took from a copy the authority never committed. The one
        // generation the operator accepted (`stado resolver accept`) is
        // adopted once, and the acceptance is spent.
        let mut adopted = false;
        if generation < current.directory_generation {
            let refusal = format!(
                "service directory rollback rejected: the authority answered generation {generation} \
                 (store version {store_version}), below generation {} this resolver loaded at {} \
                 (store version {}); each directory change committed at the authority advances its \
                 generation by one, and this host follows again once the authority passes {}, or at \
                 once after `stado resolver accept --generation {generation}` on this host",
                current.directory_generation,
                current.loaded_at_iso,
                current.store_version,
                current.directory_generation
            );
            match crate::cli::resolver::accept::accepted(generation) {
                Ok(true) => {
                    crate::cli::resolver::accept::consume()
                        .map_err(|cause| format!("{refusal}; {cause}"))?;
                    eprintln!(
                        "stado resolver adopts directory generation {generation} below {}: the operator accepted it",
                        current.directory_generation
                    );
                    adopted = true;
                }
                Ok(false) => return Err(refusal),
                Err(cause) => return Err(format!("{refusal}; {cause}")),
            }
        }
        if !adopted
            && generation == current.directory_generation
            && service_resolution::directory(&document)?
                != service_resolution::directory(&current.document)?
        {
            return Err(format!(
                "service directory changed without advancing generation {generation}"
            ));
        }
        // An adopted generation is a change the operator accepted: it is
        // said as loaded, as an advance is.
        let advanced = adopted || generation > current.directory_generation;
        // A refused cache does not fail the refresh: the document this
        // process just read is good and serving it is the job. It does get
        // recorded, so `publish_serving` carries it to `resolver status`.
        // An adopted generation is the operator's acceptance, which the copy
        // follows as the resolver does.
        let recorded = if adopted {
            targets::store_adopted_last_good(&serialized, &store_version)
        } else {
            targets::store_last_good(&serialized, &store_version)
        };
        current.document = document;
        current.store_version = store_version;
        current.directory_generation = generation;
        current.loaded_at = Instant::now();
        current.loaded_at_iso = now_iso();
        drop(current);
        match recorded {
            Ok(()) => *self.last_good_refusal.write().await = None,
            Err(refusal) => *self.last_good_refusal.write().await = Some(refusal.kind()),
        }
        *self.source.write().await = next_source;
        // Said after the markers are kept, so a reader of this line knows
        // this host's markers already follow that generation.
        self.keep_markers().await;
        if advanced {
            eprintln!("stado resolver loaded directory generation {generation}");
        }
        Ok(changed.then_some(next_config))
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

    pub(super) fn gateway_url(&self, service: &str, consumer: &str) -> Option<String> {
        let adapters = match self.adapters.read() {
            Ok(adapters) => adapters,
            Err(poisoned) => poisoned.into_inner(),
        };
        adapters
            .iter()
            .find(|adapter| adapter.service == service && adapter.consumer == consumer)
            .map(|adapter| format!("http://{}", adapter.bind))
    }

    /// The adapters this process listens on, and those it does not serve. A
    /// lock poisoned by a panic elsewhere is taken over, as every reader here
    /// does: skipping the write would publish the previous adapters forever.
    pub(super) fn hold(&self, serving: Vec<ResolverAdapter>, unserved: Vec<PublishedAdapter>) {
        match self.adapters.write() {
            Ok(mut held) => *held = serving,
            Err(poisoned) => *poisoned.into_inner() = serving,
        }
        match self.unserved.write() {
            Ok(mut held) => *held = unserved,
            Err(poisoned) => *poisoned.into_inner() = unserved,
        }
    }

    /// The address of every adapter this process listens on, then every
    /// declared adapter it does not serve with its reason.
    pub(super) fn published_adapters(&self) -> Vec<PublishedAdapter> {
        let adapters = match self.adapters.read() {
            Ok(adapters) => adapters,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut published: Vec<PublishedAdapter> = adapters
            .iter()
            .map(|adapter| PublishedAdapter {
                service: adapter.service.clone(),
                consumer: adapter.consumer.clone(),
                bind: adapter.bind.clone(),
                unserved: None,
            })
            .collect();
        let unserved = match self.unserved.read() {
            Ok(unserved) => unserved,
            Err(poisoned) => poisoned.into_inner(),
        };
        published.extend(unserved.iter().cloned());
        published
    }

    /// Publish what this process holds right now, with the refresh interval
    /// it runs on.
    pub(super) async fn publish_serving(&self) {
        let (refresh_seconds, api_bind) = match self.config.read() {
            Ok(config) => (config.refresh_seconds, config.api_bind.clone()),
            Err(poisoned) => {
                let config = poisoned.into_inner();
                (config.refresh_seconds, config.api_bind.clone())
            }
        };
        let read_from = match &*self.source.read().await {
            SnapshotSource::Authority { target, .. } => Some(target.clone()),
            SnapshotSource::Local(_) => None,
        };
        let current = self.snapshot.read().await;
        publish(
            &PublishedState::serving(
                &self.local_target,
                current.directory_generation,
                &current.store_version,
                &current.loaded_at_iso,
                *self.last_good_refusal.read().await,
                self.published_adapters(),
            )
            .read_from(read_from)
            .refreshing(refresh_seconds)
            .answering_at(&api_bind),
        );
    }

    /// The registry generation this process holds, from memory, never the
    /// store, so it answers while the store stands, and only within the
    /// declared max-stale; past it the refusal names the age. `read_from`
    /// names the authority a copy was refreshed from, and is null on the
    /// authority itself: a reader on this host takes either, while `stado
    /// resolver snapshot`, which serves other hosts, answers only the
    /// authority's own, so no host serves another a copy of a copy.
    pub(super) async fn held_snapshot(&self) -> Result<Value, String> {
        let read_from = match &*self.source.read().await {
            SnapshotSource::Authority { target, .. } => Some(target.clone()),
            _ => None,
        };
        let current = self.snapshot.read().await;
        let age = current.loaded_at.elapsed();
        if age > self.max_stale {
            return Err(format!(
                "the registry generation {} this resolver holds was loaded at {} ({} s ago), \
                 past the declared max-stale of {} s; every refresh since failed \
                 (stado resolver status names why)",
                current.directory_generation,
                current.loaded_at_iso,
                age.as_secs(),
                self.max_stale.as_secs()
            ));
        }
        Ok(serde_json::json!({
            "store_version": current.store_version,
            "document": current.document,
            "read_from": read_from,
        }))
    }
}
