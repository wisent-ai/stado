//! Offering the fleet's idle capacity on a marketplace. The marketplace is an
//! adapter named by `--provider`; the operations are the product's own.
//! `--provider stado` trades with another Stado fleet: this host runs a
//! worker against the peer fleet's queue while our own queue is idle, and
//! the peer is declared with `stado market peer`.

use clap::{Subcommand, ValueEnum};

/// The marketplaces a listing can be placed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum MarketProvider {
    /// Vast.ai: the machine is listed through its host API with the key in
    /// the Skarbiec item `vast`, field `api_key`.
    Vast,
    /// Another Stado fleet, named by `--peer`: while our queue is idle this
    /// host runs a Stado worker against the peer's queue, reached with the
    /// bearer in the item playing the peer's declared credential role.
    Stado,
}

#[derive(Subcommand)]
pub(crate) enum MarketCommands {
    /// List this fleet's machine on the marketplace at the given prices.
    /// With --provider stado: open this host to the peer again at that
    /// price, for the auto-list loop that holds the peer on this host.
    List {
        /// The marketplace to list on.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// The declared peer fleet; required with --provider stado and
        /// refused with any other provider.
        #[arg(long)]
        peer: Option<String>,
        /// Per-GPU-hour rental price in USD.
        #[arg(long)]
        price_gpu: f64,
        /// Per-GB-month disk price in USD; required with --provider vast.
        #[arg(long)]
        price_disk: Option<f64>,
        /// Optional minimum interruptible-bid price floor (--provider vast).
        #[arg(long)]
        price_min_bid: Option<f64>,
        /// Print the marketplace's answer as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw every offer for this fleet's machine, blocking new renters.
    /// Existing rentals are not terminated. With --provider stado the
    /// auto-list loop stops lending at its next poll.
    Unlist {
        /// The marketplace to withdraw from.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// The declared peer fleet; required with --provider stado.
        #[arg(long)]
        peer: Option<String>,
        /// Print the marketplace's answer as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show the marketplace's current view of this fleet's machine: rentals
    /// and whether it is listed. With --provider stado and --peer, what this
    /// host lent that peer; without --peer, what peer fleets ran in our
    /// queue.
    Status {
        /// The marketplace to ask.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// The declared peer fleet (--provider stado).
        #[arg(long)]
        peer: Option<String>,
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
        /// The declared peer fleet; required with --provider stado.
        #[arg(long)]
        peer: Option<String>,
        /// Ask this host's vault instead of the one serving Skarbiec.
        #[arg(long)]
        vault_host: Option<String>,
        /// Do not read the vault; report the channel and the marketplace only.
        #[arg(long)]
        no_vault_check: bool,
        /// Print the readiness document (stado.vast-readiness.v1 or
        /// stado.peer-readiness.v1).
        #[arg(long)]
        json: bool,
    },
    /// One snapshot of the marketplace's view beside what the queue holds.
    Monitor {
        /// The marketplace to ask.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// The declared peer fleet; required with --provider stado.
        #[arg(long)]
        peer: Option<String>,
        /// Logical Stado queue namespace; the configured bucket when omitted.
        #[arg(long)]
        bucket: Option<String>,
        /// Print the snapshot as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Keep the listing in step with the queue: list when the queue has been
    /// idle for the window, withdraw when work appears.
    Follow {
        /// The marketplace to list on.
        #[arg(long, value_enum)]
        provider: MarketProvider,
        /// The declared peer fleet; required with --provider stado.
        #[arg(long)]
        peer: Option<String>,
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
        /// Per-GB-month disk price in USD when the machine is listed;
        /// required with --provider vast.
        #[arg(long)]
        price_disk: Option<f64>,
        /// Cap the longest rental a renter can buy from this offer, in
        /// seconds (with --provider stado: the longest one lending session
        /// runs); 0 leaves it open-ended.
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
    /// The Stado fleets this fleet trades with: declare, change, read and
    /// retire a peer.
    #[command(subcommand)]
    Peer(PeerCommands),
}

/// One peer fleet as `stado market peer` declares it.
#[derive(Subcommand)]
pub(crate) enum PeerCommands {
    /// Declare a peer fleet: where its queue's object API answers, which
    /// namespace holds its queue, and which credential role holds the bearer
    /// it issued to us. Refused when the name is already declared.
    Add {
        /// The peer's name in this fleet: lowercase letters, digits and `-`.
        name: String,
        /// The peer's object API origin: HTTPS, or HTTP on loopback or a
        /// tailnet address.
        #[arg(long)]
        origin: String,
        /// The object namespace that holds the peer's queue.
        #[arg(long)]
        namespace: String,
        /// The role of the item whose `token` field is the peer's bearer.
        #[arg(long)]
        credential_role: String,
        /// PEM authority that signs the peer's origin when it is private.
        #[arg(long)]
        ca_file: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Change a declared peer; only the named fields change.
    Edit {
        name: String,
        #[arg(long)]
        origin: Option<String>,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        credential_role: Option<String>,
        /// Replace the PEM authority for the peer's origin.
        #[arg(long, conflicts_with = "clear_ca_file")]
        ca_file: Option<String>,
        /// Trust the peer's origin through the system roots again.
        #[arg(long)]
        clear_ca_file: bool,
        #[arg(long)]
        json: bool,
    },
    /// Retire a declared peer. Refused while an auto-list loop on this host
    /// holds the peer.
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Every declared peer.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One declared peer, with this host's offer to it.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
}
