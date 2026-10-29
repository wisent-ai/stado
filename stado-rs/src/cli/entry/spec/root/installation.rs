//! What this installation is, what it holds, what it costs, and the upkeep it
//! runs on the machine in front of you: the first block of `stado --help`.

use clap::Subcommand;

use crate::cli::work::autonomy;
use crate::cli::*;

/// The first block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum InstallationCommands {
    /// Show the CLI first-use walkthrough or import an existing registry-v2 file.
    Onboarding {
        /// Discard recorded progress and evidence, then show the walkthrough again.
        #[arg(long)]
        reset: bool,
        /// Additively adopt this registry-v2 JSON file into the canonical registry.
        #[arg(long = "import-registry")]
        import_registry: Option<String>,
        /// Emit the typed import receipt. Requires --import-registry.
        #[arg(long, requires = "import_registry")]
        json: bool,
    },

    /// List Stado capability families, variants, providers and active selections;
    /// resolve the alternative that answers a product's need, or select a family's order.
    #[command(args_conflicts_with_subcommands = true)]
    Capabilities {
        /// Restrict output to one capability family.
        capability: Option<String>,
        /// Emit the versioned machine-readable catalog.
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<CapabilityAction>,
    },

    /// One operator snapshot: jobs, active workers, quota, budgets, burn and credits.
    Overview {
        /// Emit the complete machine-readable snapshot.
        #[arg(long)]
        json: bool,
    },

    /// Inventory a dependency's live resources, auth, consumers, storage, and DR coverage.
    #[command(name = "impact")]
    BlastRadius(blast_radius::BlastRadiusArgs),

    /// Inventory, plan, execute, verify, and restore resource operations.
    #[command(subcommand)]
    Resources(resources::ResourcesCommands),
    /// Inspect and control autonomous placement and resource reconciliation.
    #[command(subcommand)]
    Optimize(autonomy::OptimizeCommands),

    /// Inspect or refresh cross-cloud costs, grants, burn, and credit balances.
    #[command(subcommand)]
    Billing(BillingCommands),

    /// Sign a cloud operator in and repair the Stado role contract;
    /// `--provider` names the cloud.
    #[command(subcommand)]
    Cloud(azure::CloudCommands),

    /// Route public hostnames through a tunnel provider's ingress and DNS
    /// with Stado-held credentials; `--provider` names the provider.
    #[command(subcommand)]
    Tunnel(cloudflare::TunnelCommands),

    /// List the waits open on this machine, from the register every Stado
    /// wait keeps (`$STADO_WAITS_DIR` or ~/.stado/waits); a wait whose
    /// process no longer runs is reported ended and removed. `open`
    /// registers a wait for another process; `close` removes one; `cycles`
    /// names every lock-wait cycle standing on this machine.
    Waits {
        #[command(subcommand)]
        action: Option<WaitsAction>,
        /// Emit the machine-readable listing.
        #[arg(long)]
        json: bool,
    },

    /// Whether the components people rely on work: `check` asks every
    /// component the catalog declares under `public_status` once and
    /// publishes state, since when and the last failed check; `show` reads
    /// what was published; `serve` is the public status page that reads it.
    #[command(name = "availability", subcommand)]
    PublicStatus(PublicStatusCommands),
}

/// What a product does with a family of alternatives: ask which one answers,
/// or choose the order they are tried in.
#[derive(Subcommand)]
pub(crate) enum CapabilityAction {
    /// The first alternative of FAMILY, in the selected order, proven to
    /// answer CONSUMER, with every one before it that did not; exits with the
    /// infra_down code and names each alternative's product, route and cause
    /// when none answers. A proof stands for the refresh interval this host's
    /// resolver declares; `--failed` proves the named alternative again now.
    Resolve {
        /// The family: inference, secrets, browser-automation, mail or transcripts.
        family: String,
        /// The product asking, as the service directory names its consumer.
        #[arg(long)]
        consumer: String,
        /// An alternative that refused the caller after it was handed out;
        /// proven again before it is handed out once more. Repeat for several.
        #[arg(long = "failed")]
        failed: Vec<String>,
        /// Print the answer, or the refusal's details, as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Write FAMILY's order to `capabilities.<family>.order` in the config
    /// file; each name must be one of the family's alternatives, once.
    Select {
        /// The family whose order is chosen.
        family: String,
        /// The alternatives in the order they are tried, comma-separated or repeated.
        #[arg(long, value_delimiter = ',', required = true)]
        order: Vec<String>,
    },
}

/// The published component status: asked, read, and served as a page.
#[derive(Subcommand)]
pub(crate) enum PublicStatusCommands {
    /// Ask every declared component once, from this machine, and publish
    /// what each answered beside what the last check recorded.
    Check {
        /// Emit the published document as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The published state of every declared component: since when, and the
    /// last failed check with the answer that failed it.
    Show {
        /// Emit the published document as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The public status page on --bind (`host:port`), as its own unit on a
    /// host apart from the products it reports on: every request reads the
    /// published document again, and one that cannot be read is answered
    /// 503 naming why. `/` is the page, `/status.json` the document.
    Serve {
        /// The address to listen on, `host:port`.
        #[arg(long)]
        bind: String,
    },
}

/// Registering a wait a program does not perform in Rust itself.
#[derive(Subcommand)]
pub(crate) enum WaitsAction {
    /// Register for process --pid a wait on --kind (one of the register's
    /// `rodzaj` words: siec, object-api, host, blokada, proces, baza,
    /// terminal, zgoda, sesja), for --what at --where. It ends when that
    /// process does or with `stado waits close <id>`. With --awaits
    /// `<kind>:<id>` (lock, host, service, revision, unit or port) the wait
    /// is an edge of the wait-for graph, and one that would close a cycle is
    /// refused naming every link instead of being registered.
    Open {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        what: String,
        #[arg(long = "where")]
        place: String,
        #[arg(long)]
        pid: u32,
        /// The resource the wait awaits, `<kind>:<id>`.
        #[arg(long)]
        awaits: Option<String>,
        /// Emit the registered wait as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record that process --pid holds --resource `<kind>:<id>` (lock,
    /// host, service, revision, unit or port) until `stado waits release
    /// <id>` or until that process ends: a wait on that resource is then a
    /// wait on that process.
    Hold {
        #[arg(long)]
        resource: String,
        #[arg(long)]
        pid: u32,
        /// Emit the holding as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove the holding <id>: its resource is released.
    Release {
        id: String,
        /// Emit the answer as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Every resource a running process holds, as recorded beside the
    /// register; the holdings of processes that ended are removed.
    Holds {
        /// Emit the holdings as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove the wait <id> from the register: it ended.
    Close {
        id: String,
        /// Emit the answer as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Every cycle standing among this machine's waits: a process waiting
    /// on a resource held, directly or through other waits, by a process
    /// whose completion needs it. Each link names the process, its command,
    /// the resource it waits on and who holds that resource. Fails with the
    /// refusal status when any cycle stands.
    Cycles {
        /// Emit the cycles as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Every operation that completed a wait on this machine with the
    /// longest completed wait on it, longest first: the measurement each
    /// later wait on that operation is due by, and the one place to see
    /// which operations take long here. With --over SECONDS only those
    /// whose longest wait exceeded it. It only reads; nothing changes.
    Measured {
        /// Only operations whose longest completed wait exceeded this many
        /// seconds.
        #[arg(long)]
        over: Option<u64>,
        /// Emit the measurements as JSON.
        #[arg(long)]
        json: bool,
    },
}
