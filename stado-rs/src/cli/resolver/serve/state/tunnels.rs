//! The native SSH sessions a resolver's adapters share, one per active host
//! and service address, and the hosts none of whose paths answered.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LockResult};
use std::time::{Duration, Instant};

use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::targets;

/// An active host whose every SSH path refused the last session this
/// resolver tried, and the refusal: a connection to it within the refresh
/// interval is answered with it at once, since another handshake started
/// then would stand where this one stood. The first connection after the
/// interval tries again.
pub(in crate::cli::resolver::serve) struct Unreached {
    at: Instant,
    cause: String,
}

/// Whether this resolver already said that it read a lock a panicked holder
/// poisoned; the line is printed once, not on every connection.
static RECOVERY_SAID: AtomicBool = AtomicBool::new(false);

/// The guard `locked` holds, recovered when a panicked holder poisoned it:
/// the values behind these locks are whole after any panic, so they are
/// read as they stand, and the first recovery is said on stderr naming
/// `what` was held.
fn recovered<G>(locked: LockResult<G>, what: &str) -> G {
    locked.unwrap_or_else(|poisoned| {
        if !RECOVERY_SAID.swap(true, Ordering::Relaxed) {
            eprintln!(
                "stado resolver: the {what} lock was poisoned by a holder that panicked; its \
                 values are read as they stand"
            );
        }
        poisoned.into_inner()
    })
}

impl super::ResolverState {
    /// Why `active_host` is refused without a handshake: its paths all
    /// refused within the last refresh interval. `None` when it was not
    /// tried, answered, or that refusal is older than the interval. A lock a
    /// panicked holder poisoned still holds whole values (a refresh interval,
    /// a map of plain records), so it is read through
    /// [`std::sync::PoisonError::into_inner`] and said once on stderr; read
    /// as nothing remembered, every refused host would be tried again.
    fn recently_unreached(&self, active_host: &str) -> Option<String> {
        let seconds = recovered(self.config.read(), "configuration").refresh_seconds;
        let interval = Duration::from_secs(seconds);
        let unreached = recovered(self.unreached.lock(), "unreached-host memory");
        let last = unreached.get(active_host)?;
        let ago = last.at.elapsed();
        (ago < interval).then(|| {
            format!(
                "not tried again: every SSH path of {active_host} refused {}s ago, within this \
                 resolver's {}s refresh interval ({}); the first connection after it tries again",
                ago.as_secs(),
                interval.as_secs(),
                last.cause
            )
        })
    }

    /// Keep what the last session attempt to `active_host` said: a refusal
    /// is remembered for [`Self::recently_unreached`], a session forgets it.
    /// A poisoned lock is recovered as there, never skipped.
    fn note_attempt(&self, active_host: &str, refused: Option<&str>) {
        let mut unreached = recovered(self.unreached.lock(), "unreached-host memory");
        match refused {
            Some(cause) => {
                let record = Unreached {
                    at: Instant::now(),
                    cause: cause.to_string(),
                };
                unreached.insert(active_host.to_string(), record);
            }
            None => {
                unreached.remove(active_host);
            }
        }
    }

    /// Open a channel and retain its session until the caller finishes copying.
    ///
    /// Two things would make connections through this adapter queue while the
    /// remote object API refuses channels at once: dropping a healthy session
    /// on every refused channel, so the next connection opens a new SSH
    /// session from scratch; and opening while holding the one lock all
    /// services and consumers share, so each connection waits for every
    /// handshake queued before it. A session is opened without the shared
    /// lock, and it is dropped only when it is closed; a channel the service
    /// refuses leaves it in place and is reported to the caller at once. A
    /// host none of whose paths answered within the refresh interval is
    /// refused at once, naming that refusal ([`Self::recently_unreached`]).
    pub(in crate::cli::resolver::serve) async fn tunnel_connect(
        &self,
        active_host: &str,
        paths: &[targets::SshConnectionPath],
        host: &str,
        port: u16,
    ) -> Result<(russh::ChannelStream<russh::client::Msg>, Arc<Tunnel>), String> {
        let key = format!("{active_host}|{host}:{port}");
        let tunnel = self.shared_tunnel(active_host, &key, paths).await?;
        let ended = match tunnel.connect(host, port).await {
            Ok(stream) => return Ok((stream, tunnel)),
            // The session is alive: the service end refused this one
            // channel, and a new session would be refused the same way.
            Err(error) if tunnel.usable() => return Err(error),
            Err(error) => error,
        };
        // The shared session had ended: it leaves the pool, and this
        // connection, which has sent no bytes yet, gets the session opened
        // in its place.
        {
            let mut tunnels = self.tunnels.lock().await;
            if tunnels
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &tunnel))
            {
                tunnels.remove(&key);
            }
        }
        let replacement = self
            .shared_tunnel(active_host, &key, paths)
            .await
            .map_err(|error| format!("{ended}; no session could replace it: {error}"))?;
        match replacement.connect(host, port).await {
            Ok(stream) => Ok((stream, replacement)),
            Err(error) => Err(format!("{ended}; on the session that replaced it: {error}")),
        }
    }

    /// The pooled session for `key`, or one opened without the shared lock
    /// and pooled; when another connection pooled one meanwhile, that one.
    /// A host whose paths all refused within the refresh interval is not
    /// tried again; a fresh attempt's outcome is remembered either way.
    async fn shared_tunnel(
        &self,
        active_host: &str,
        key: &str,
        paths: &[targets::SshConnectionPath],
    ) -> Result<Arc<Tunnel>, String> {
        let held = {
            let mut tunnels = self.tunnels.lock().await;
            match tunnels.get(key).filter(|tunnel| tunnel.usable()) {
                Some(tunnel) => Some(Arc::clone(tunnel)),
                None => {
                    tunnels.remove(key);
                    None
                }
            }
        };
        if let Some(tunnel) = held {
            return Ok(tunnel);
        }
        if let Some(refused) = self.recently_unreached(active_host) {
            return Err(refused);
        }
        let opened = match Tunnel::open(paths).await {
            Ok(opened) => Arc::new(opened),
            Err(refused) => {
                self.note_attempt(active_host, Some(&refused));
                return Err(refused);
            }
        };
        self.note_attempt(active_host, None);
        let mut tunnels = self.tunnels.lock().await;
        Ok(Arc::clone(
            tunnels
                .entry(key.to_string())
                .or_insert_with(|| Arc::clone(&opened)),
        ))
    }
}
