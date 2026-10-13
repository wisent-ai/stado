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
use crate::cli::resolver::report::waiting;
use crate::targets;

use super::ResolverState;

impl ResolverState {
    /// Decide what carries a connection on adapter `bind` whose waits to
    /// `host:port` on `active_host` stand, the youngest for
    /// `youngest_held_seconds`: the sentence naming the session that
    /// replaced a standing one, `None` when the current session (or a new
    /// one, when none is kept) carries it as it is, or the reason none can.
    /// A replaced session's held waits are marked so `stado resolver status`
    /// says what they still stand on and callers stop refusing on them.
    pub(in crate::cli::resolver::serve) async fn renew_standing_session(
        &self,
        bind: &str,
        active_host: &str,
        paths: &[targets::SshConnectionPath],
        host: &str,
        port: u16,
        youngest_held_seconds: i64,
    ) -> Result<Option<String>, String> {
        let key = format!("{active_host}|{host}:{port}");
        let cached = self.tunnels.lock().await.get(&key).cloned();
        let Some(standing) = cached else {
            return Ok(None);
        };
        let session_seconds = standing.age().as_secs() as i64;
        if session_seconds < youngest_held_seconds {
            return Ok(None);
        }
        let opened = Tunnel::open(paths).await.map_err(|error| {
            format!(
                "the session to {active_host} for {host}:{port} stands with them and a new \
                 session in its place did not open: {error}"
            )
        })?;
        let mut tunnels = self.tunnels.lock().await;
        if !tunnels
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &standing))
        {
            return Ok(None);
        }
        tunnels.insert(key, Arc::new(opened));
        waiting::session::replaced(bind, active_host, &format!("{host}:{port}"));
        Ok(Some(format!(
            "the session to {active_host} for {host}:{port} stands, answered {session_seconds}s \
             ago and holding connections unanswered for {youngest_held_seconds}s, while a new \
             session to {active_host} answered: new connections ride the new session and the \
             held ones stay on the standing one until their clients leave"
        )))
    }

    /// Replace every session this process holds a route's waits on past the
    /// refresh interval (`waiting::session::standing`), on each refresh
    /// tick: a caller that reads the published list refuses itself before it
    /// connects, so the connection that would replace the session never
    /// arrives. Each replacement, and each that fails, is said on stderr.
    pub(in crate::cli::resolver::serve) async fn renew_standing_sessions(&self) {
        for route in waiting::session::standing() {
            let said = format!(
                "stado resolver service={} consumer={}",
                route.service, route.consumer
            );
            let resolved = match self.resolve(&route.service, &route.consumer).await {
                Ok(resolved) if resolved.active_host == route.active_host => resolved,
                Ok(_) => continue,
                Err(error) => {
                    eprintln!("{said} cannot renew the session its waits stand on: {error}");
                    continue;
                }
            };
            let Some((host, port)) = route.endpoint.rsplit_once(':') else {
                eprintln!("{said} waits on {}, which names no port", route.endpoint);
                continue;
            };
            let Ok(port) = port.parse::<u16>() else {
                eprintln!("{said} waits on {}, whose port is no port", route.endpoint);
                continue;
            };
            let paths = crate::cli::resolver::authority::paths::resolved_ssh_paths(&resolved);
            match self
                .renew_standing_session(
                    &route.bind,
                    &route.active_host,
                    &paths,
                    host,
                    port,
                    route.youngest_seconds,
                )
                .await
            {
                Ok(Some(renewed)) => eprintln!("{said} {renewed}"),
                Ok(None) => {}
                Err(cause) => eprintln!("{said} {cause}"),
            }
        }
    }
}
