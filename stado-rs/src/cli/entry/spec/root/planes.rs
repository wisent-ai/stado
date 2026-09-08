//! The long-running control planes: the scheduling tick, the API listener,
//! and the two packaged combinations of them. The third block of
//! `stado --help`.

use clap::Subcommand;

/// The third block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum PlaneCommands {
    /// Run the provider-neutral scheduling tick locally.
    ///
    /// Reads cadence and identity from the named coordinator entry. Queue,
    /// registry, capacity, and schedule state use the configured Stado
    /// storage backend.
    Coordinator {
        /// Coordinator name or host heuristic (default: active=true entry).
        #[arg(long)]
        target: Option<String>,
        /// Run a single scheduling tick and exit (cron-friendly).
        #[arg(long)]
        once: bool,
    },

    /// Run the Stado API listener for the wisent-compute queue.
    ///
    /// Serves native operator actions and the authenticated object, release,
    /// machine, service, host-health and enrollment routes over loopback
    /// HTTP. It serves no HTML page; the operator workspace is Stado
    /// Desktop.
    ///
    /// With --enrollment-only the listener serves nothing but the three
    /// enrollment routes, which is the only shape safe to publish.
    Dashboard {
        /// Bind address. Default WC_DASHBOARD_BIND or 127.0.0.1.
        #[arg(long)]
        bind: Option<String>,
        /// Port. Default WC_DASHBOARD_PORT or 8765.
        #[arg(long)]
        port: Option<i64>,
        /// Serve ONLY GET /join.sh, GET /api/fleet/invite/key and
        /// POST /api/fleet/join; answer 404 to every other path and method.
        /// Publish this listener through a tunnel, never the full dashboard.
        #[arg(long)]
        enrollment_only: bool,
    },

    /// Run a device-local API listener, scheduler, and worker.
    #[command(name = "local-control-plane", hide = true)]
    LocalControlPlane {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 8765)]
        port: i64,
        #[arg(long, default_value_t = 15)]
        interval: i64,
    },

    /// Run a cloud-hosted coordinator and API listener.
    #[command(name = "cloud-control-plane", hide = true)]
    CloudControlPlane {
        #[arg(long, default_value = "localhost")]
        bind: String,
        #[arg(long, default_value_t = 8080)]
        port: i64,
        #[arg(long, default_value_t = 30)]
        interval: i64,
    },
}
