//! `stado host local`: the halves of host verbs that run on the host itself,
//! started by the operator side over the host channel with the host's own
//! installed Stado. Each is the object its operator verb acts on, with the
//! verb that runs here: a lock to run under, the object API's readers, the
//! release store's repair, the backup's audit, a port the host hands out and
//! the storage-root transaction's phases, steps and worker.

use clap::Subcommand;

/// The host-side halves of `stado host` verbs.
#[derive(Subcommand)]
pub(crate) enum HostLocalCommands {
    /// Run PROGRAM on this host under an exclusive, non-blocking flock of
    /// LOCK; the fixed recovery programs' lock holder.
    Run {
        lock: String,
        #[arg(last = true)]
        program: Vec<String>,
    },
    /// The host half of the object-API recovery: its readers of this host's
    /// config, launchd definitions, route and Skarbiec release state.
    #[command(subcommand)]
    Api(crate::cli::host::ObjectApiLocalCommands),
    /// Return one release-catalog coordinate in this host's local stores to
    /// the managed account; the host half of the release-store repair.
    Repair {
        #[arg(long)]
        config: String,
        #[arg(long)]
        product: String,
    },
    /// The host half of `stado credentials backup audit`: classify this host's
    /// replica against its primary store and, with --reclaim true --apply true,
    /// delete the twins this pass proved. Prints the marker lines the
    /// operator side reads.
    Audit {
        #[arg(long)]
        backup: std::path::PathBuf,
        #[arg(long)]
        primary: std::path::PathBuf,
        #[arg(long)]
        namespace: String,
        /// yes/no (any of clap's boolish spellings)
        #[arg(long, action = clap::ArgAction::Set, value_parser = clap::builder::BoolishValueParser::new())]
        reclaim: bool,
        #[arg(long, action = clap::ArgAction::Set, value_parser = clap::builder::BoolishValueParser::new())]
        apply: bool,
        /// Exact object paths, each hex-encoded, comma separated
        #[arg(long, default_value = "")]
        objects_hex: String,
        /// Namespaces to inventory, each hex-encoded, comma separated
        #[arg(long, default_value = "")]
        inventory_namespaces_hex: String,
    },
    /// A port nobody chooses: binds a free loopback port the host's own
    /// system hands out, releases it and prints its number, so `service
    /// ensure` records a catalog service's port and `service directory
    /// consumer add` a resolver adapter's in the registry.
    Port,
    /// The storage-root transaction (`stado repair`'s storage-root step) as
    /// it runs on this host: one receipt phase, one host step, or the
    /// resident worker.
    #[command(subcommand)]
    Reconcile(HostLocalReconcileCommands),
}

/// The storage-root transaction's host-side programs.
#[derive(Subcommand)]
pub(crate) enum HostLocalReconcileCommands {
    /// One phase on this host's two local storage roots. Prints the marker
    /// line the transaction worker reads; the owner token and the inherited
    /// lock descriptor arrive in STADO_RECONCILE_OWNER_TOKEN and
    /// STADO_RECONCILE_LOCK_FD.
    Phase {
        #[arg(long)]
        phase: String,
        #[arg(long)]
        transaction: String,
    },
    /// The host steps outside the receipt phases: launching the resident
    /// worker, unit files, listener and object API observations.
    #[command(subcommand)]
    Step(crate::deploy::host_storage_reconcile_host::HostCommands),
    /// The resident transaction worker.
    Worker {
        target: String,
        #[arg(long)]
        target_config: String,
        #[arg(long)]
        transaction: String,
        /// The resident action; the worker refuses one that does not parse as
        /// a `host_storage_reconcile::ResidentAction`.
        #[arg(long)]
        phase: String,
        #[arg(long)]
        source_revision: String,
        #[arg(long)]
        tool_sha256: String,
        #[arg(long)]
        runner_gate: String,
    },
}
