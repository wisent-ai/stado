//! Offering the fleet's idle GPU on a compute marketplace. The marketplace is
//! an adapter named by `--provider`; the operations are the product's own.

use clap::{Subcommand, ValueEnum};

/// The marketplaces a listing can be placed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum MarketProvider {
    /// Vast.ai: the machine is listed through its host API with the key in
    /// the Skarbiec item `vast`, field `api_key`.
    Vast,
}

#[derive(Subcommand)]
pub(crate) enum MarketCommands {
    /// List this fleet's GPU machine on the marketplace at the given prices.
    List {
        /// The marketplace to list on.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// Per-GPU-hour rental price in USD.
        #[arg(long)]
        price_gpu: f64,
        /// Per-GB-month disk price in USD.
        #[arg(long)]
        price_disk: f64,
        /// Optional minimum interruptible-bid price floor.
        #[arg(long)]
        price_min_bid: Option<f64>,
        /// Print the marketplace's answer as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw every offer for this fleet's machine, blocking new renters.
    /// Existing rentals are not terminated.
    Unlist {
        /// The marketplace to withdraw from.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// Print the marketplace's answer as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show the marketplace's current view of this fleet's machine: rentals
    /// and whether it is listed.
    Status {
        /// The marketplace to ask.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// Print the marketplace's answer as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Whether this machine can earn on the marketplace, and which
    /// provisioning step is missing when it cannot: the Skarbiec channel this
    /// host holds, the vault that would declare the credential, and the
    /// marketplace's own answer to it. Exits non-zero unless the marketplace
    /// accepts the credential.
    Readiness {
        /// The marketplace to check.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// Ask this host's vault instead of the one serving Skarbiec.
        #[arg(long)]
        vault_host: Option<String>,
        /// Do not read the vault; report the channel and the marketplace only.
        #[arg(long)]
        no_vault_check: bool,
        /// Print the stado.vast-readiness.v1 document.
        #[arg(long)]
        json: bool,
    },
    /// One snapshot of the marketplace's view beside what the queue holds.
    Monitor {
        /// The marketplace to ask.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// Logical Stado queue namespace; the configured bucket when omitted.
        #[arg(long)]
        bucket: Option<String>,
        /// Print the snapshot as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Keep the listing in step with the queue: list when the queue has been
    /// idle for the window, withdraw when work appears.
    #[command(name = "auto-list")]
    AutoList {
        /// The marketplace to list on.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// The queue must be idle this many seconds before listing.
        #[arg(long)]
        idle_window_s: i64,
        /// Seconds between polls of the configured Stado queue storage.
        /// Required unless --once.
        #[arg(long)]
        poll_interval_s: Option<u64>,
        /// Per-GPU-hour rental price in USD when the machine is listed.
        #[arg(long)]
        price_gpu: f64,
        /// Cap the longest rental a renter can buy from this offer, in
        /// seconds; 0 leaves it open-ended.
        #[arg(long)]
        max_duration_s: i64,
        /// Print the toggle decisions without calling the marketplace. Needs
        /// no credential: the decisions come from the queue.
        #[arg(long)]
        dry_run: bool,
        /// Evaluate one poll and exit, instead of looping. With --dry-run
        /// this is the one-shot preview a graphical surface can run.
        #[arg(long)]
        once: bool,
    },
}
