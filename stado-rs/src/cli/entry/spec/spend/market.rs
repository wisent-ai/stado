//! Offering the fleet's idle capacity on a marketplace. The marketplace is an
//! adapter named by `--provider`; the operations are the product's own.
//! `--provider stado` trades with another Stado fleet: this host runs a
//! worker against the peer fleet's queue while our own queue is idle, and
//! the peer is declared with `stado market peer`.

use clap::Subcommand;

/// A marketplace Stado carries an adapter for. `--provider` takes any seller
/// the capability catalog's `market` family declares: an implemented one
/// becomes this value, and a declared seller without an adapter is refused
/// naming the catalog's own note, so adding a marketplace is a catalog row
/// and an adapter, and the commands read the family instead of a list here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarketProvider {
    /// Vast.ai: the machine is listed through its host API with the key in
    /// the Skarbiec item `vast`, field `api_key`.
    Vast,
    /// Another Stado fleet, named by `--peer`: while our queue is idle this
    /// host runs a Stado worker against the peer's queue, reached with the
    /// bearer in the item playing the peer's declared credential role.
    Stado,
}

impl std::str::FromStr for MarketProvider {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        use crate::capabilities::{CapabilityKind, CapabilitySupport, ProviderId, CAPABILITIES};
        let family = CAPABILITIES
            .iter()
            .find(|capability| capability.id == CapabilityKind::Market)
            .ok_or("the capability catalog declares no market family")?;
        let sellers: Vec<&str> = family
            .providers
            .iter()
            .map(|row| row.provider.as_str())
            .collect();
        let provider = crate::capabilities::provider(raw)
            .filter(|provider| family.providers.iter().any(|row| row.provider == *provider))
            .ok_or_else(|| {
                format!(
                    "{raw:?} is not a seller of the market family; it declares {}",
                    sellers.join(", ")
                )
            })?;
        let row = family
            .providers
            .iter()
            .find(|row| row.provider == provider)
            .ok_or("the market family lost its own row")?;
        match (provider, row.support) {
            (ProviderId::Vast, CapabilitySupport::Implemented) => Ok(Self::Vast),
            (ProviderId::Stado, CapabilitySupport::Implemented) => Ok(Self::Stado),
            (_, support) => Err(format!(
                "{} is declared {} in the market family ({}); Stado carries no adapter for it yet, \
                 see `stado capabilities market`",
                provider.as_str(),
                support.as_str(),
                row.note
            )),
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum MarketCommands {
    /// List this fleet's machine on the marketplace at the given prices.
    /// With --provider stado: open this host to the peer again at that
    /// price, for the auto-list loop that holds the peer on this host.
    List {
        /// The marketplace to list on.
        #[arg(long)]
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
        #[arg(long)]
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
        #[arg(long)]
        provider: MarketProvider,
        /// The declared peer fleet (--provider stado).
        #[arg(long)]
        peer: Option<String>,
        /// Print the marketplace's answer as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Every place a fleet host can earn, from the declared list of sources
    /// (rental marketplaces, inference, training and media networks, proof,
    /// storage and bandwidth networks): what each pays for, its unit and
    /// currency, the page that says how a host joins, and whether Stado
    /// carries an adapter. --check asks each page whether it still answers
    /// and exits non-zero naming each that does not.
    Sources {
        #[arg(long)]
        check: bool,
        #[arg(long)]
        json: bool,
    },
    /// Payouts a source made in a token, kept with the rate each is valued
    /// at: `record` keeps one as the source's own statement states it,
    /// `list` prints them with their US-dollar value. A payout with no
    /// declared rate is kept and printed as unpriced, never at a guess.
    #[command(subcommand)]
    Payout(PayoutCommands),
    /// Whether a host's inference deployment is worth more than renting its
    /// board: the tokens it served per measured hour, priced at an outside
    /// model's per-token prices, beside the median hourly price renters pay
    /// for the board model on the marketplace, from the newest `stado market
    /// measure` reading. Names the larger, and every figure it could not read.
    ServeOrRent {
        /// The marketplace whose rental price is compared.
        #[arg(long)]
        provider: MarketProvider,
        /// Taken only with --provider stado, which is refused.
        #[arg(long)]
        peer: Option<String>,
        /// The registry host serving the deployment.
        #[arg(long)]
        target: String,
        /// The outside model the traffic would otherwise buy, exactly as
        /// OpenRouter lists it (e.g. thedrummer/cydonia-24b-v4.1).
        #[arg(long = "outside-model")]
        outside_model: String,
        /// The board model as the marketplace spells it.
        #[arg(long)]
        gpu: String,
        /// Hours back from now to measure.
        #[arg(long = "since-hours", default_value_t = 24)]
        since_hours: u32,
        #[arg(long)]
        json: bool,
    },
    /// What the marketplace asks for a board model right now: the number of
    /// rentable one-board on-demand offers and what a renter pays per hour
    /// (lowest, median, highest), read from the public offer search, which
    /// needs no credential. Each reading is kept in the fleet store with its
    /// time and source, so what to sell is decided against figures.
    Measure {
        /// The marketplace to read.
        #[arg(long)]
        provider: MarketProvider,
        /// Taken only with --provider stado, which is refused.
        #[arg(long)]
        peer: Option<String>,
        /// The board model as the marketplace spells it (e.g. "RTX PRO 6000 WS").
        #[arg(long)]
        gpu: String,
        #[arg(long)]
        json: bool,
    },
    /// Announce to the marketplace that this host goes down for maintenance,
    /// so every renter is told to save their work before it does.
    ///
    /// Vast notifies each renter with an active contract; a machine taken
    /// down under a contract without this lowers its reliability score.
    /// Prints how many renters were told. Run it before any release, restart
    /// or deployment that stops a listed host.
    Maintenance {
        /// The marketplace to tell.
        #[arg(long)]
        provider: MarketProvider,
        /// Taken only with --provider stado, which is refused.
        #[arg(long)]
        peer: Option<String>,
        /// Minutes from now until the host goes down.
        #[arg(long = "in-minutes", default_value_t = 0)]
        in_minutes: u32,
        /// How long the host stays down, in hours.
        #[arg(long)]
        hours: f64,
        /// Why: power, internet, disk, gpu, software or other.
        #[arg(long)]
        category: String,
        #[arg(long)]
        json: bool,
    },
    /// What a registry host earned on the marketplace, day by day, as the
    /// marketplace itself states it, beside what the same days of the host
    /// cost (`stado cost host`) and the margin.
    ///
    /// Vast.ai states income per machine and day (GPU, storage, upload and
    /// download, gross); a day it states no row for is printed as not
    /// stated, never as zero. Each run reads the marketplace and keeps every
    /// stated day in the fleet store (`market/earnings/<provider>/<host>/
    /// <day>.json`), so the ledger outlives the marketplace's own window.
    Earnings {
        /// The marketplace to read.
        #[arg(long)]
        provider: MarketProvider,
        /// Taken only with --provider stado, which is refused.
        #[arg(long)]
        peer: Option<String>,
        /// The registry host whose machine is read.
        #[arg(long)]
        target: String,
        /// Whole days back from today to read.
        #[arg(long = "since-days", default_value_t = 7)]
        since_days: u32,
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
        #[arg(long)]
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
        #[arg(long)]
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
        #[arg(long)]
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
    /// A network's own node run on a fleet host while the fleet leaves the
    /// host idle: declare which declared service earns from which source and
    /// what it holds, offer or withdraw it, and run the loop that starts it
    /// on an idle host and stops it when fleet work appears.
    #[command(subcommand)]
    Node(NodeCommands),
}

/// One network node as `stado market node` declares and runs it.
#[derive(Subcommand)]
pub(crate) enum NodeCommands {
    /// Declare a node, withdrawn until `edit --listed true` offers it.
    /// Refused when the name is taken, the source is not one `stado market
    /// sources` declares, or the host declares no such service.
    Add {
        /// The node's name in this fleet: lowercase letters, digits and `-`.
        name: String,
        /// The source it earns from, by its id in `stado market sources`.
        #[arg(long)]
        source: String,
        /// The registry host its service is declared on.
        #[arg(long)]
        target: String,
        /// The service, as `stado service declare` named it, installed from
        /// the network's official release.
        #[arg(long)]
        service: String,
        /// Cores the node holds while it runs.
        #[arg(long = "cpu-cores")]
        cpu_cores: i64,
        /// GiB of memory it holds.
        #[arg(long = "ram-gb")]
        ram_gb: f64,
        /// GiB of board memory it holds.
        #[arg(long = "vram-gb")]
        vram_gb: i64,
        #[arg(long)]
        json: bool,
    },
    /// Change a declared node; only the named fields change. `--listed
    /// true` offers it, `--listed false` withdraws it, and a running follow
    /// loop stops it on its next poll.
    Edit {
        name: String,
        #[arg(long)]
        listed: Option<bool>,
        #[arg(long = "cpu-cores")]
        cpu_cores: Option<i64>,
        #[arg(long = "ram-gb")]
        ram_gb: Option<f64>,
        #[arg(long = "vram-gb")]
        vram_gb: Option<i64>,
        #[arg(long)]
        json: bool,
    },
    /// Retire a node. Refused while its follow loop has it running.
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Every declared node with the loop's last transition.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One declared node with the loop's last transition.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Run the node while its host is idle: after --idle-window-s idle
    /// seconds, with the node offered, take its capacity reservation and
    /// start its service; stop it and release the reservation the moment
    /// fleet work appears for the host or it is withdrawn.
    Follow {
        name: String,
        /// Idle seconds before the node starts.
        #[arg(long)]
        idle_window_s: i64,
        /// Seconds between queue polls; required unless --once.
        #[arg(long)]
        poll_interval_s: Option<u64>,
        /// Print the decisions without starting or stopping anything.
        #[arg(long)]
        dry_run: bool,
        /// One poll and exit; needs --dry-run.
        #[arg(long)]
        once: bool,
    },
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

#[derive(Subcommand, Debug)]
pub(crate) enum PayoutCommands {
    /// Keep one payout a source stated.
    Record {
        /// The source's id in `stado market sources`.
        #[arg(long)]
        source: String,
        /// The registry host the payout is for.
        #[arg(long)]
        host: String,
        /// When it was paid, RFC 3339.
        #[arg(long = "paid-at")]
        paid_at: String,
        /// The token it was paid in.
        #[arg(long)]
        token: String,
        #[arg(long)]
        amount: f64,
        /// The US dollars one unit was worth, with --as-of and --rate-source.
        #[arg(long = "usd-per-unit")]
        usd_per_unit: Option<f64>,
        /// The date of that rate.
        #[arg(long = "as-of")]
        as_of: Option<String>,
        /// The published table the rate was read from.
        #[arg(long = "rate-source")]
        rate_source: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print every kept payout, or one host's, with its US-dollar value.
    List {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
}
