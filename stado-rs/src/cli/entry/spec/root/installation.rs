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

    /// List Stado capability families, variants, providers and active selections.
    Capabilities {
        /// Restrict output to one capability family.
        capability: Option<String>,
        /// Emit the versioned machine-readable catalog.
        #[arg(long)]
        json: bool,
    },

    /// One operator snapshot: jobs, active workers, quota, budgets, burn and credits.
    Overview {
        /// Emit the complete machine-readable snapshot.
        #[arg(long)]
        json: bool,
    },

    /// Inventory a dependency's live resources, auth, consumers, storage, and DR coverage.
    #[command(name = "blast-radius")]
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

    /// Apply the disk-full rule on this machine once: at 80% used, delete
    /// everything the fleet put here. For another host,
    /// `stado space reclaim TARGET --stage registry_cleanup` runs this on the
    /// target's own installed Stado. The resident watch is the
    /// `--disk-cleanup` role of `stado serve`.
    #[command(name = "disk-cleanup")]
    DiskCleanup {
        /// Run every cleaner and delete nothing: what a pass at the
        /// threshold would remove now.
        #[arg(long)]
        dry_run: bool,
    },

    /// Preview every directory directly under ~/.stado/work, including job
    /// and run areas. --apply removes them all, even when active.
    Workdirs {
        /// Delete all listed directories; root-level files and links stay
        /// unless --include-files is given.
        #[arg(long)]
        apply: bool,
        /// Also remove the loose files and links directly at the root, so the
        /// scratch root itself ends up empty. Links are unlinked, never
        /// followed.
        #[arg(long = "include-files")]
        include_files: bool,
        /// Emit the machine-readable report.
        #[arg(long)]
        json: bool,
    },

    /// List the waits open on this machine, from the register every Stado
    /// wait keeps (`$STADO_WAITS_DIR` or ~/.stado/waits); a wait whose
    /// process no longer runs is reported ended and removed. `open`
    /// registers a wait for another process; `close` removes one.
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
    /// what was published. A status page reads `show`.
    #[command(name = "public-status", subcommand)]
    PublicStatus(PublicStatusCommands),
}

/// The two halves of the published component status.
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
}

/// Registering a wait a program does not perform in Rust itself.
#[derive(Subcommand)]
pub(crate) enum WaitsAction {
    /// Register for process --pid a wait on --kind (one of the register's
    /// `rodzaj` words: siec, object-api, host, blokada, proces, baza,
    /// terminal, zgoda, sesja), for --what at --where. It ends when that
    /// process does or with `stado waits close <id>`.
    Open {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        what: String,
        #[arg(long = "where")]
        place: String,
        #[arg(long)]
        pid: u32,
        /// Emit the registered wait as JSON.
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
}
