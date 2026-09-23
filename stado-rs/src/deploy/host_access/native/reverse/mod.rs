//! A reverse listener belongs to the host process, not an `ssh -N` child.
//! Its declared reconciliation cadence is independent of health publication.

use std::num::{NonZeroU16, NonZeroU64};

use anyhow::{bail, Context, Result};
use tokio::task::JoinHandle;

mod relay;

pub(super) use relay::relay;
pub(super) const LOOPBACK: &str = "127.0.0.1";

#[derive(Clone, Copy)]
pub(super) struct Ports {
    remote: NonZeroU16,
    local: NonZeroU16,
}

pub(crate) struct ReverseForward {
    destination: String,
    ports: Ports,
    task: Option<JoinHandle<Result<()>>>,
}

impl ReverseForward {
    pub(crate) fn new(
        destination: String,
        remote: NonZeroU16,
        local: NonZeroU16,
    ) -> Result<Self> {
        crate::deploy::host_users::validate_ssh_target(&destination)?;
        Ok(Self { destination, ports: Ports { remote, local }, task: None })
    }

    pub(crate) async fn run(mut self, interval: NonZeroU64) -> Result<()> {
        let mut schedule = tokio::time::interval(std::time::Duration::from_secs(interval.get()));
        schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            schedule.tick().await;
            self.reconcile().await;
        }
    }

    /// A failed transport must not stop the worker or interrupt its jobs.
    /// At most one connection attempt or registered listener exists at a time.
    async fn reconcile(&mut self) {
        if self.task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        if let Some(task) = self.task.take() {
            let detail = match task.await {
                Ok(Err(error)) => format!("{error:#}"),
                Ok(Ok(())) => "forwarding task returned unexpectedly".to_string(),
                Err(error) => format!("forwarding task failed: {error}"),
            };
            eprintln!(
                "stado reverse forward destination={} remote={LOOPBACK}:{} local={LOOPBACK}:{} stopped: {detail}",
                self.destination, self.ports.remote, self.ports.local
            );
        }
        let destination = self.destination.clone();
        let ports = self.ports;
        self.task = Some(tokio::spawn(async move { serve(&destination, ports).await }));
    }
}

impl Drop for ReverseForward {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn serve(destination: &str, ports: Ports) -> Result<()> {
    let mut session = super::connect_with(destination, Some(ports)).await?;
    session.tcpip_forward(LOOPBACK, u32::from(ports.remote.get())).await
        .with_context(|| format!("register reverse listener {LOOPBACK}:{} on {destination}", ports.remote))?;
    eprintln!(
        "stado reverse forward destination={destination} registered {LOOPBACK}:{} -> {LOOPBACK}:{} in pid={}",
        ports.remote, ports.local, std::process::id()
    );
    (&mut *session).await
        .with_context(|| format!("serve reverse listener {LOOPBACK}:{} on {destination}", ports.remote))?;
    bail!("SSH session ended while reverse listener {LOOPBACK}:{} was registered", ports.remote)
}
