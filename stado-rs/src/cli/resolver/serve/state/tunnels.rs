//! The native SSH sessions a resolver's adapters share, one per active host
//! and service address.

use std::sync::Arc;

use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::targets;

impl super::ResolverState {
    /// Open a channel and retain its session until the caller finishes copying.
    ///
    /// Two things would make connections through this adapter queue while the
    /// remote object API refuses channels at once: dropping a healthy session
    /// on every refused channel, so the next connection opens a new SSH
    /// session from scratch; and opening while holding the one lock all
    /// services and consumers share, so each connection waits for every
    /// handshake queued before it. A session is opened without the shared
    /// lock, and it is dropped only when it is closed; a channel the service
    /// refuses leaves it in place and is reported to the caller at once.
    pub(in crate::cli::resolver::serve) async fn tunnel_connect(
        &self,
        active_host: &str,
        paths: &[targets::SshConnectionPath],
        host: &str,
        port: u16,
    ) -> Result<(russh::ChannelStream<russh::client::Msg>, Arc<Tunnel>), String> {
        let key = format!("{active_host}|{host}:{port}");
        let tunnel = self.shared_tunnel(&key, paths).await?;
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
            .shared_tunnel(&key, paths)
            .await
            .map_err(|error| format!("{ended}; no session could replace it: {error}"))?;
        match replacement.connect(host, port).await {
            Ok(stream) => Ok((stream, replacement)),
            Err(error) => Err(format!("{ended}; on the session that replaced it: {error}")),
        }
    }

    /// The pooled session for `key`, or one opened without the shared lock
    /// and pooled; when another connection pooled one meanwhile, that one.
    async fn shared_tunnel(
        &self,
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
        let opened = Arc::new(Tunnel::open(paths).await?);
        let mut tunnels = self.tunnels.lock().await;
        Ok(Arc::clone(
            tunnels
                .entry(key.to_string())
                .or_insert_with(|| Arc::clone(&opened)),
        ))
    }
}
