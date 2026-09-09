//! What this installation is, what it holds, what it costs, and the upkeep it
//! runs on the machine in front of you: the first block of `stado --help`.

use clap::Subcommand;

use crate::cli::*;

/// The first block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum InstallationCommands {
    /// Print the installed crate data root for desktop provisioning.
    #[command(name = "package-root", hide = true)]
    PackageRoot,

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
    Optimize(autonomy_cmd::OptimizeCommands),

    /// Inspect or refresh cross-cloud costs, grants, burn, and credit balances.
    #[command(subcommand)]
    Billing(BillingCommands),

    /// Authenticate an Azure operator and repair the Stado RBAC contract.
    #[command(subcommand)]
    Azure(azure::AzureCommands),

    /// Configure Cloudflare Tunnel ingress and DNS through Stado-held credentials.
    #[command(subcommand)]
    Cloudflare(cloudflare::CloudflareCommands),

    /// Search and deterministically analyze Gmail messages without modifying them.
    #[command(subcommand)]
    Mail(MailCommands),

    /// Run registry-authorized cleanup for this local target.
    #[command(name = "disk-cleanup")]
    DiskCleanup {
        /// Run one interval-gated cleanup check (default).
        #[arg(long)]
        once: bool,
        /// Continuously check at the canonical policy interval.
        #[arg(long)]
        watch: bool,
        /// Run one bounded enforcing pass toward the declared target even when
        /// the host is already above its low watermark.
        #[arg(long)]
        to_target: bool,
        /// Plan a pass and delete nothing: same policy, same scan, an
        /// `enforce` policy pinned to the janitor's own report mode.
        #[arg(long)]
        dry_run: bool,
    },

    /// Install the registry-controlled cleanup watch on this Mac.
    #[command(name = "install-disk-cleanup")]
    InstallDiskCleanup,

    /// Report, and with --apply remove, the scratch working directories under
    /// `~/.stado/work` that no owner declares.
    Workdirs {
        /// Remove them. Without this the command only reports.
        #[arg(long)]
        apply: bool,
        /// Emit the machine-readable report.
        #[arg(long)]
        json: bool,
    },
}
