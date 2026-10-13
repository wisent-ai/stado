//! A session that stands. Every connection to one service endpoint on a
//! remote host rides the resolver's cached native SSH session to it. That
//! session can stop answering its channels while the host itself answers a
//! new SSH session at once: every relayed request then waits without end,
//! and refusing each later connection behind the held ones kept the service
//! from every consumer on this host for hours although a new session would
//! have carried it.
//!
//! So a connection on a held adapter first asks whether the held waits ride
//! the session it would be sent on. When the youngest held wait began after
//! that session answered, the session itself stands: a new session to the
//! same host is opened in its place, and when it answers the connection
//! rides it. The held connections stay on the standing session until their
//! clients leave. When the held waits all began before the current session
//! answered, they ride one already replaced and the connection rides the
//! current one. Only a host that answers no new session refuses the
//! connection, with the held wait and that failure. A service that holds its
//! requests on any session costs one new session per refresh interval, since
//! a wait counts as held only after it.

use std::sync::Arc;

use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::targets;

use super::ResolverState;

impl ResolverState {
    /// Decide what carries a connection on an adapter whose waits to
    /// `host:port` on `active_host` stand, the youngest for
    /// `youngest_held_seconds`: the sentence saying which session carries
    /// it, or the reason none can.
    pub(in crate::cli::resolver::serve) async fn renew_standing_session(
        &self,
        active_host: &str,
        paths: &[targets::SshConnectionPath],
        host: &str,
        port: u16,
        youngest_held_seconds: i64,
    ) -> Result<String, String> {
        let key = format!("{active_host}|{host}:{port}");
        let cached = self.tunnels.lock().await.get(&key).cloned();
        let Some(standing) = cached else {
            return Ok(format!(
                "no session to {active_host} for {host}:{port} is kept now, so the connection \
                 opens a new one"
            ));
        };
        let session_seconds = standing.age().as_secs() as i64;
        if session_seconds < youngest_held_seconds {
            return Ok(format!(
                "the held waits began before the session to {active_host} for {host}:{port} \
                 answered {session_seconds}s ago, so they ride one it replaced and the \
                 connection rides it"
            ));
        }
        let opened = Tunnel::open(paths).await.map_err(|error| {
            format!(
                "the session to {active_host} for {host}:{port} stands with them and a new \
                 session in its place did not open: {error}"
            )
        })?;
        let opened = Arc::new(opened);
        let mut tunnels = self.tunnels.lock().await;
        if tunnels
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &standing))
        {
            tunnels.insert(key, opened);
        }
        Ok(format!(
            "the session to {active_host} for {host}:{port} stands, answered {session_seconds}s \
             ago and holding connections unanswered for {youngest_held_seconds}s, while a new \
             session to {active_host} answered: new connections ride the new session and the \
             held ones stay on the standing one until their clients leave"
        ))
    }
}
