//! The host service owns its long-running components, not child daemons.

use std::num::NonZeroU64;
use std::time::Duration;

use clap::Args;

use crate::cli::entry::spec::root::work::AgentOptions;
use crate::cli::hosts::{agent, coordinator};
use crate::cli::CmdError;

mod supervisor;

#[derive(Args)]
pub(crate) struct ServeArgs {
    #[command(flatten)]
    pub worker: AgentOptions,
    /// Exact coordinator entry to run here; omitting it leaves scheduling and the API disabled.
    #[arg(long)]
    pub coordinator: Option<String>,
    /// API bind address, using the existing dashboard configuration when omitted.
    #[arg(long, requires = "coordinator")]
    pub bind: Option<String>,
    /// API port, using the existing dashboard configuration when omitted.
    #[arg(long, requires = "coordinator")]
    pub port: Option<u16>,
    /// Seconds between release reconciliation passes.
    #[arg(long)]
    pub release_interval_seconds: NonZeroU64,
    /// Declared cadence for collecting and publishing this host's health beacon.
    #[arg(long)]
    pub health_interval_seconds: NonZeroU64,
}

pub(crate) async fn run(mut args: ServeArgs) -> Result<(), CmdError> {
    if args.worker.idle_shutdown {
        return Err(CmdError::usage(
            "serve owns persistent host services; --idle-shutdown belongs to an ephemeral worker",
        ));
    }
    if !crate::capabilities::ProviderId::Local.matches(&args.worker.kind) {
        return Err(CmdError::usage(
            "serve requires --kind local; ephemeral cloud workers use agent",
        ));
    }
    // Resolve registry overrides before starting components. The worker receives
    // the resolved GPU type and cannot later mutate process-wide configuration.
    let auto = args.worker.auto || args.worker.target.is_none();
    let (gpu_type, target) = agent::apply_registry_target(
        std::mem::take(&mut args.worker.gpu_type),
        args.worker.target.as_deref(),
        auto,
    )
    .await?;
    let target = target.ok_or_else(|| CmdError::click("serve resolved no registry target"))?;
    if !crate::capabilities::ProviderId::Local.matches(&target.kind) {
        return Err(CmdError::usage(format!(
            "serve target {} has kind {}; expected local",
            target.name, target.kind
        )));
    }
    args.worker.gpu_type = gpu_type;
    args.worker.target = None;
    args.worker.auto = false;

    let proxy_control =
        crate::release_agent::rollout::serving::control::prepare().map_err(CmdError::click)?;
    let mut supervisor = supervisor::Supervisor::new();
    supervisor.spawn("release-proxy", move || {
        crate::release_agent::rollout::serving::control::serve(proxy_control)
    })?;
    let resolver_target = target.name.clone();
    supervisor.spawn("resolver", move || async move {
        crate::cli::resolver::serve(&resolver_target).await
    })?;
    let release_target = target.name.clone();
    supervisor.spawn("release", move || async move {
        crate::release_agent::agent(
            &release_target,
            None,
            false,
            args.release_interval_seconds.get(),
        )
        .await
    })?;
    supervisor.spawn("host-health", move || {
        health_beacons(Duration::from_secs(args.health_interval_seconds.get()))
    })?;
    if let Some(name) = args.coordinator {
        supervisor.spawn("coordinator", move || {
            coordinator::run(Some(name), crate::coordinator::Invocation::Hosted)
        })?;
        supervisor.spawn("api", move || {
            crate::cli::dashboard::run(args.bind, args.port.map(i64::from), false)
        })?;
    }
    supervisor.spawn("worker", move || {
        agent::run(
            args.worker.gpu_type,
            None,
            false,
            false,
            args.worker.kind,
            args.worker.vast_auto_list,
            args.worker.vast_price_gpu,
            args.worker.vast_max_duration_s,
        )
    })?;
    eprintln!(
        "stado serve: target={} pid={} components={}",
        target.name,
        std::process::id(),
        supervisor.components().join(",")
    );
    supervisor.wait().await
}

async fn health_beacons(period: Duration) -> Result<(), CmdError> {
    let mut schedule = tokio::time::interval(period);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        schedule.tick().await;
        // Every collection runs to completion. A delayed pass skips obsolete
        // scheduled ticks instead of cancelling work or bursting repeated probes.
        if let Err(error) = crate::cli::host::collect_beacon(true).await {
            eprintln!("[stado serve host-health] collect-and-publish failed: {error}");
        }
    }
}
