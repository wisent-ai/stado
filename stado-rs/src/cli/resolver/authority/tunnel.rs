use std::process::Stdio;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};

use crate::cli::resolver::authority::ssh_proxy_command;

/// How long a freshly opened forward may take to accept its first connection.
///
/// Read by [`crate::doctor`] too: a probe that reaches the fleet through this
/// transport cannot be bounded tighter than the transport is declared to need,
/// or it reports a healthy registry as unreachable every time the channel has
/// gone cold.
pub const TUNNEL_OPEN_BUDGET: Duration = Duration::from_secs(30);
/// How often the opening forward is probed inside that budget.
const TUNNEL_OPEN_POLL: Duration = Duration::from_millis(100);

/// One long-lived SSH forward to one `host:port` behind one destination, shared
/// by every connection that needs it.
///
/// The transport used to be `ssh -W` per request: one operating-system process,
/// with a pipe pair, for every object read. Multiplexing made those share a
/// remote session, which is what the old comment claimed, but not a process --
/// a resolver up for thirty minutes held 178 descriptors, 122 of them pipes,
/// for about sixty requests in flight. Under a release the fan-out is unbounded,
/// so the process walked into its own descriptor budget: `accept failed: Too
/// many open files`, launchd counted 166 restarts, and every death dropped the
/// connections in flight -- four publications died mid-run and every fleet agent
/// hung on its next object read, which froze the capacity heartbeats the
/// release pipeline picks builders from.
///
/// A forward costs one process for as long as the destination is in use, and a
/// request costs the two sockets it would cost anyway. Fan-out stops being a
/// function of traffic, so there is nothing to bound and nothing to refuse.
///
/// The forwarding process is not the forward. When a control master is already
/// up -- and [`select_resolver_ssh_path`] leaves one up, because its probe runs
/// under the same `ControlMaster=auto` -- `ssh -N -L` is a multiplex slave: it
/// hands the listener to the master and exits 0 immediately, before the master
/// has bound it. Judging the forward by that corpse read every hand-off as a
/// dead transport: `refused connection: the SSH forward to <destination>
/// exited before it accepted: exit status: 0`, 56 times in one resolver
/// lifetime on this workstation and 41 of them on `stado-object-api`, each one
/// answered to the client as `HTTP 502 upstream unavailable`. So the port is
/// what proves a forward here, and `child` only says whether the process that
/// asked for it failed.
pub(crate) struct Tunnel {
    pub(crate) local: u16,
    child: tokio::process::Child,
}

impl Tunnel {
    /// Open a forward and wait, bounded, until it accepts.
    pub(crate) async fn open(destination: &str, host: &str, port: u16) -> Result<Self, String> {
        // Ask the OS for a free loopback port and release it: `ssh -L` needs a
        // number before it starts, and this is the only way to learn one that
        // is free. A racing binder is handled by the caller, which drops the
        // forward and opens another.
        let probe = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("no free loopback port for an SSH forward: {error}"))?;
        let local = probe
            .local_addr()
            .map_err(|error| format!("loopback probe has no address: {error}"))?
            .port();
        drop(probe);
        let mut child = ssh_proxy_command()
            .args([
                "-N",
                "-L",
                &format!("127.0.0.1:{local}:{host}:{port}"),
                destination,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start the SSH forward: {error}"))?;
        let deadline = tokio::time::Instant::now() + TUNNEL_OPEN_BUDGET;
        loop {
            // The port first, and the process only when the port is still
            // shut. A slave that handed its listener to a live control master
            // is already gone by the time the master binds, so asking the
            // process first turns every hand-off into a refusal.
            if let Ok(open) = TcpStream::connect(("127.0.0.1", local)).await {
                drop(open);
                return Ok(Self { local, child });
            }
            match child.try_wait() {
                // A clean exit with the port still shut is the hand-off in
                // progress: the master owns the listener now and has the rest
                // of the budget to bind it. A forward that never arrives is
                // reported by the deadline below, naming the wait rather than
                // an exit status that was never the failure.
                Ok(Some(status)) if status.success() => {}
                Ok(Some(status)) => {
                    return Err(format!(
                        "the SSH forward to {destination} exited before it accepted: {status}"
                    ))
                }
                Ok(None) => {}
                Err(error) => return Err(format!("SSH forward wait failed: {error}")),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "the SSH forward to {destination} did not accept within {}s",
                    TUNNEL_OPEN_BUDGET.as_secs()
                ));
            }
            tokio::time::sleep(TUNNEL_OPEN_POLL).await;
        }
    }

    /// Whether this forward may still carry traffic.
    ///
    /// A multiplex slave exits 0 as soon as the control master takes its
    /// listener, so a dead child with a clean status leaves a live forward
    /// behind; only a non-zero exit says the transport failed. Treating the
    /// clean exit as death discarded a warm forward on every single request,
    /// which put every request back into the opening race above instead of
    /// once per destination. What actually proves a forward is the dial in
    /// [`ResolverState::tunnel_connect`], and that already retries once
    /// against a freshly opened one.
    pub(crate) fn usable(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => status.success(),
            Err(_) => false,
        }
    }
}
