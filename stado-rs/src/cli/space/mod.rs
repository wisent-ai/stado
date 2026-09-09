use clap::Subcommand;
use serde_json::{json, Map, Value};

use super::{host, CmdError};

mod coverage;
mod ops;
mod read;
pub mod watermark;

use ops::{reclaim, relocate, remove_file, retire_file};
use read::{print_json, report};

#[derive(Subcommand)]
pub enum SpaceCommands {
    /// Read disk, memory, inventory, build caches, and janitor state as one report.
    Report {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Read or set the memory watermarks TARGET is measured against.
    ///
    /// With no write flag this prints the declaration in force. With one it
    /// rewrites `targets[].memory_reclaim` through the canonical registry's
    /// compare-and-swap, validating the whole document first.
    Watermark(watermark::WatermarkArgs),
    /// Reclaim only fleet-declared stages, previewing unless --apply is present.
    Reclaim {
        target: String,
        /// Select a declared stage; repeat to select several. Omit for all stages.
        #[arg(long = "stage")]
        stages: Vec<String>,
        /// Report what the selected stages would remove and write no audit record.
        #[arg(long)]
        dry_run: bool,
        /// Remove what the selected stages name. Requires --reason.
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        /// Why the space is being reclaimed; recorded on the target beside its state.
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Perform one guarded single-file space operation.
    File {
        #[command(subcommand)]
        command: SpaceFileCommands,
    },
    /// Relocate object-store keys on the host that holds their bytes.
    Relocate {
        target: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        from_prefix: String,
        #[arg(long, default_value = "")]
        to_prefix: String,
        #[arg(long)]
        store_root: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum SpaceFileCommands {
    /// Remove one regular file from a Stado-managed area of TARGET.
    Remove {
        target: String,
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Archive one obsolete executable or launchd declaration without deleting its bytes.
    Retire {
        target: String,
        path: String,
        #[arg(long)]
        product: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        transaction: Option<String>,
        #[arg(long)]
        expected_sha256: Option<String>,
        #[arg(long)]
        expected_size: Option<u64>,
        #[arg(long)]
        expected_mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Device-local primitive used by the target-resolving retire command.
    #[command(name = "retire-local", hide = true)]
    RetireLocal {
        path: String,
        #[arg(long)]
        product: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        transaction: Option<String>,
        #[arg(long)]
        expected_sha256: Option<String>,
        #[arg(long)]
        expected_size: Option<u64>,
        #[arg(long)]
        expected_mode: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: SpaceCommands) -> Result<(), CmdError> {
    match command {
        SpaceCommands::Report { target, json } => report(&target, json).await,
        SpaceCommands::Watermark(args) => watermark::dispatch(args).await,
        SpaceCommands::Reclaim {
            target,
            stages,
            dry_run: _,
            apply,
            reason,
            json,
        } => reclaim(&target, &stages, apply, reason.as_deref(), json).await,
        SpaceCommands::File { command } => match command {
            SpaceFileCommands::Remove { target, path, json } => {
                remove_file(&target, &path, json).await
            }
            SpaceFileCommands::Retire {
                target,
                path,
                product,
                dry_run,
                transaction,
                expected_sha256,
                expected_size,
                expected_mode,
                json,
            } => {
                retire_file(
                    &target,
                    host::RetireFileRequest {
                        path: &path,
                        product: &product,
                        dry_run,
                        transaction: transaction.as_deref(),
                        expected_sha256: expected_sha256.as_deref(),
                        expected_size,
                        expected_mode: expected_mode.as_deref(),
                    },
                    json,
                )
                .await
            }
            SpaceFileCommands::RetireLocal {
                path,
                product,
                dry_run,
                transaction,
                expected_sha256,
                expected_size,
                expected_mode,
                json,
            } => host::retire_file_local(
                host::RetireFileRequest {
                    path: &path,
                    product: &product,
                    dry_run,
                    transaction: transaction.as_deref(),
                    expected_sha256: expected_sha256.as_deref(),
                    expected_size,
                    expected_mode: expected_mode.as_deref(),
                },
                json,
            ),
        },
        SpaceCommands::Relocate {
            target,
            namespace,
            from_prefix,
            to_prefix,
            store_root,
            dry_run: _,
            apply,
            limit,
            json,
        } => {
            relocate(
                &target,
                &crate::deploy::host_object_relocate::RelocatePlan {
                    namespace,
                    from: from_prefix,
                    to: to_prefix,
                    store_root,
                    apply,
                    limit,
                },
                json,
            )
            .await
        }
    }
}
