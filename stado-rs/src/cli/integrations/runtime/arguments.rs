//! The host service's options, and the installer's rendering of the same
//! options the CLI executes.

use std::num::{NonZeroU16, NonZeroU64};

use clap::{Args, ValueEnum};

use crate::cli::entry::spec::root::work::AgentOptions;

#[derive(Args)]
pub(crate) struct ServeArgs {
    #[command(flatten)]
    pub worker: AgentOptions,
    /// Run a persistent local worker; other host roles do not imply one.
    #[arg(long = "worker")]
    pub run_worker: bool,
    /// This device belongs to no fleet registry: the worker runs without a
    /// registry target, against the configured local queue. It cannot be
    /// combined with roles that need one (a resolver, release reconciliation,
    /// a worker --target or --auto). An ephemeral cloud machine runs its
    /// worker this way, with its provider's --kind and --idle-shutdown.
    #[arg(
        long,
        requires = "run_worker",
        conflicts_with_all = ["resolver", "release_interval_seconds", "target", "auto"]
    )]
    pub standalone: bool,
    /// Run the disk and memory cleanup watch inside this host process, reading
    /// the volume at the host's declared --health-interval-seconds.
    #[arg(long, requires = "health_interval_seconds")]
    pub disk_cleanup: bool,
    /// Dispatch unhandled failed jobs inside this process at the declared cadence.
    #[arg(long)]
    pub failure_fixer_interval_seconds: Option<NonZeroU64>,
    /// Preserve the failure fixer's command substring filter.
    #[arg(long, requires = "failure_fixer_interval_seconds")]
    pub failure_fixer_command_pattern: Option<String>,
    /// Run this host's declared service resolver inside the host process.
    #[arg(long)]
    pub resolver: bool,
    /// Exact coordinator entry to run here; add --api to expose its API listener.
    #[arg(long, conflicts_with = "control_plane")]
    pub coordinator: Option<String>,
    /// Preserve a bundled local or cloud scheduling loop inside this process.
    #[arg(long, value_enum, requires = "control_plane_interval_seconds")]
    pub control_plane: Option<crate::remote::control_plane::CoordinatorMode>,
    /// Scheduling cadence from the existing bundled control-plane declaration.
    #[arg(long, requires = "control_plane")]
    pub control_plane_interval_seconds: Option<i64>,
    /// Serve the configured API without claiming a coordinator role.
    #[arg(long)]
    pub api: bool,
    /// API bind address, using the existing dashboard configuration when omitted.
    #[arg(long)]
    pub bind: Option<String>,
    /// API port, using the existing dashboard configuration when omitted.
    #[arg(long)]
    pub port: Option<u16>,
    /// Serve ONLY GET /join.sh, GET /api/fleet/invite/key and
    /// POST /api/fleet/join; answer 404 to every other path and method.
    /// Publish this listener through a tunnel, never the full API.
    #[arg(long, requires = "api")]
    pub enrollment_only: bool,
    /// Serve the listening TCP socket this process was given as its standard
    /// input instead of binding one. The parent that bound the socket keeps
    /// the port reserved until this listener serves it. Refused with --bind
    /// or --port.
    #[arg(long, requires = "api")]
    pub inherited_listener: bool,
    /// JSON primary/backup endpoints for the API, independent of worker storage.
    #[arg(long, requires = "api", conflicts_with = "api_local_store")]
    pub api_storage: Option<crate::queue::ServerStorage>,
    /// The local root the API serves, independent of the storage the other
    /// roles use: `--api-storage` for a local primary, spelled as a path so a
    /// unit file can carry it (a unit argument cannot hold the JSON's quotes).
    #[arg(long, requires = "api", value_name = "PATH")]
    pub api_local_store: Option<std::path::PathBuf>,
    /// Enable release reconciliation at this declared cadence.
    #[arg(long)]
    pub release_interval_seconds: Option<NonZeroU64>,
    /// Enable host-health publication at this declared cadence.
    #[arg(long)]
    pub health_interval_seconds: Option<NonZeroU64>,
    /// Reconcile installed product surfaces against canonical origin/main at
    /// this declared cadence, inside this process.
    #[arg(long, requires = "product_sync_surface")]
    pub product_sync_interval_seconds: Option<NonZeroU64>,
    /// A surface `stado product sync --surface` reconciles on that cadence;
    /// repeat for each declared surface, in the order they run.
    #[arg(long, requires = "product_sync_interval_seconds")]
    pub product_sync_surface: Vec<String>,
    /// Existing reverse-forward SSH destination (user@host); both ends stay loopback.
    #[arg(long, requires_all = ["forward_remote_port", "forward_local_port", "forward_interval_seconds"])]
    pub forward_destination: Option<String>,
    /// Remote loopback port of the existing reverse listener.
    #[arg(long, requires = "forward_destination")]
    pub forward_remote_port: Option<NonZeroU16>,
    /// Local loopback port reached by that listener.
    #[arg(long, requires = "forward_destination")]
    pub forward_local_port: Option<NonZeroU16>,
    /// Reconcile the reverse listener at its own declared cadence.
    #[arg(long, requires = "forward_destination")]
    pub forward_interval_seconds: Option<NonZeroU64>,
    /// Run the web edge's reverse proxy, this program, inside this process.
    #[arg(long, requires = "edge_caddyfile")]
    pub edge_caddy: Option<std::path::PathBuf>,
    /// The Caddyfile `stado web edge` delivers to this host; watched and reloaded.
    #[arg(long, requires = "edge_caddy")]
    pub edge_caddyfile: Option<std::path::PathBuf>,
    /// Run the GitHub pre-check runner installed at this root inside this
    /// process: its launcher starts under passwordless sudo and drops to the
    /// runner's own account.
    #[arg(long, value_name = "ROOT")]
    pub precheck_runner: Option<std::path::PathBuf>,
    /// Collect workstation diagnostics inside this host process.
    #[arg(long)]
    pub watchdog: bool,
    /// Diagnostics destination; the standalone watchdog's default applies when omitted.
    #[arg(long, requires = "watchdog")]
    pub watchdog_bucket: Option<String>,
    /// Seconds between workstation diagnostics collections; required with
    /// --watchdog.
    #[arg(long, requires = "watchdog")]
    pub watchdog_interval_seconds: Option<i64>,
}

impl ServeArgs {
    pub(crate) fn arguments(&self) -> Vec<String> {
        // Attached values retain empty strings and leading hyphens when the
        // emitted declaration is parsed again by clap.
        let mut args = vec![
            "serve".to_string(),
            format!("--gpu-type={}", self.worker.gpu_type),
            format!("--kind={}", self.worker.kind),
        ];
        if let Some(price) = self.worker.vast_price_gpu {
            args.push(format!("--vast-price-gpu={price}"));
        }
        if let Some(seconds) = self.worker.vast_max_duration_s {
            args.push(format!("--vast-max-duration-s={seconds}"));
        }
        if self.run_worker {
            args.push("--worker".to_string());
        }
        if self.disk_cleanup {
            args.push("--disk-cleanup".to_string());
        }
        if let Some(interval) = self.failure_fixer_interval_seconds {
            args.push(format!("--failure-fixer-interval-seconds={interval}"));
        }
        if let Some(pattern) = &self.failure_fixer_command_pattern {
            args.push(format!("--failure-fixer-command-pattern={pattern}"));
        }
        if let Some(interval) = self.release_interval_seconds {
            args.push(format!("--release-interval-seconds={interval}"));
        }
        if let Some(interval) = self.health_interval_seconds {
            args.push(format!("--health-interval-seconds={interval}"));
        }
        if let Some(interval) = self.product_sync_interval_seconds {
            args.push(format!("--product-sync-interval-seconds={interval}"));
        }
        for surface in &self.product_sync_surface {
            args.push(format!("--product-sync-surface={surface}"));
        }
        if let Some(target) = &self.worker.target {
            args.push(format!("--target={target}"));
        }
        if self.worker.auto {
            args.push("--auto".to_string());
        }
        if self.worker.idle_shutdown {
            args.push("--idle-shutdown".to_string());
        }
        if self.worker.vast_auto_list {
            args.push("--vast-auto-list".to_string());
        }
        if let Some(seconds) = self.worker.poll_seconds {
            args.push(format!("--poll-seconds={seconds}"));
        }
        if let Some(seconds) = self.worker.vast_idle_window_s {
            args.push(format!("--vast-idle-window-s={seconds}"));
        }
        if self.resolver {
            args.push("--resolver".to_string());
        }
        if let Some(coordinator) = &self.coordinator {
            args.push(format!("--coordinator={coordinator}"));
        }
        if let Some(mode) = self.control_plane {
            let value = mode
                .to_possible_value()
                .expect("coordinator modes are CLI values");
            args.push(format!("--control-plane={}", value.get_name()));
        }
        if let Some(interval) = self.control_plane_interval_seconds {
            args.push(format!("--control-plane-interval-seconds={interval}"));
        }
        if let Some(destination) = &self.forward_destination {
            args.push(format!("--forward-destination={destination}"));
        }
        if let Some(port) = self.forward_remote_port {
            args.push(format!("--forward-remote-port={port}"));
        }
        if let Some(port) = self.forward_local_port {
            args.push(format!("--forward-local-port={port}"));
        }
        if let Some(interval) = self.forward_interval_seconds {
            args.push(format!("--forward-interval-seconds={interval}"));
        }
        if let Some(caddy) = &self.edge_caddy {
            args.push(format!("--edge-caddy={}", caddy.display()));
        }
        if let Some(caddyfile) = &self.edge_caddyfile {
            args.push(format!("--edge-caddyfile={}", caddyfile.display()));
        }
        if let Some(root) = &self.precheck_runner {
            args.push(format!("--precheck-runner={}", root.display()));
        }
        if self.api {
            args.push("--api".to_string());
        }
        if let Some(bind) = &self.bind {
            args.push(format!("--bind={bind}"));
        }
        if let Some(port) = self.port {
            args.push(format!("--port={port}"));
        }
        if let Some(storage) = &self.api_storage {
            args.push(format!("--api-storage={storage}"));
        }
        if let Some(root) = &self.api_local_store {
            args.push(format!("--api-local-store={}", root.display()));
        }
        if self.watchdog {
            args.push("--watchdog".to_string());
            if let Some(interval) = self.watchdog_interval_seconds {
                args.push(format!("--watchdog-interval-seconds={interval}"));
            }
            if let Some(bucket) = &self.watchdog_bucket {
                args.push(format!("--watchdog-bucket={bucket}"));
            }
        }
        args
    }
}
