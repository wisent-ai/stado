//! Renting our own idle GPU out on the Vast.ai marketplace.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum VastCommands {
    /// List the configured Vast.ai machine on the marketplace.
    /// Requires stado-vast/api_key in Skarbiec and WC_VAST_MACHINE_ID unless
    /// the machine can be discovered automatically.
    List {
        /// Per-GPU-hour rental price USD (default 0.50).
        #[arg(long, default_value_t = 0.50)]
        price_gpu: f64,
        /// Per-GB-month disk price USD (default 0.05).
        #[arg(long, default_value_t = 0.05)]
        price_disk: f64,
        /// Optional minimum interruptible-bid price floor.
        #[arg(long)]
        price_min_bid: Option<f64>,
    },
    /// Remove every offer for our Vast.ai machine, blocking new renters.
    /// Existing rentals are not terminated. Requires stado-vast/api_key in
    /// Skarbiec and a resolvable machine id.
    Unlist,
    /// Show Vast.ai's current view of our machine (rentals, listed).
    Status,
    /// One-shot snapshot of the Vast bridge + wisent-compute state.
    Monitor {
        /// Logical Stado queue namespace (default wisent-compute).
        #[arg(long, default_value = "wisent-compute")]
        bucket: String,
    },
    /// Daemon: list on Vast.ai when wisent-compute is idle, unlist when work appears.
    #[command(name = "auto-list")]
    AutoList {
        /// Wisent-compute must be idle this many seconds before listing.
        #[arg(long, default_value_t = 300)]
        idle_window_s: i64,
        /// Polling interval against configured Stado queue storage.
        #[arg(long, default_value_t = 10)]
        poll_interval_s: i64,
        /// Per-GPU-hour rental price USD when we list.
        #[arg(long, default_value_t = 0.50)]
        price_gpu: f64,
        /// Cap the max rental length any Vast renter can buy from
        /// this offer (default 3600s = 1h). 0 to leave open-ended.
        #[arg(long, default_value_t = 3600)]
        max_duration_s: i64,
        /// Print the toggle decisions without calling the Vast API.
        #[arg(long)]
        dry_run: bool,
    },
}
