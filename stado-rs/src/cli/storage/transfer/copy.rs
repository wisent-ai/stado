//! `stado storage copy` and `stado storage backup`, and the plan and report
//! they print.

use crate::cli::storage::*;

// ---- copy ----

#[derive(Args, Debug)]
pub struct StorageCopyArgs {
    #[command(flatten)]
    ends: EndpointArgs,

    /// Restrict the copy to this prefix. Repeatable. Omit to copy the whole
    /// canonical prefix set.
    #[arg(long = "prefix")]
    prefix: Vec<String>,
    /// Print the per-prefix plan and copy nothing.
    #[arg(long)]
    dry_run: bool,
    /// Objects copied in parallel.
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: NonZeroUsize,
}

#[derive(Args, Debug)]
pub struct StorageBackupArgs {
    /// Restrict the backup to this prefix. Repeatable. Omit to copy the
    /// complete canonical state set.
    #[arg(long = "prefix")]
    prefix: Vec<String>,
    /// Print the plan and write nothing.
    #[arg(long)]
    dry_run: bool,
    /// Objects copied in parallel.
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: NonZeroUsize,
}

pub(in crate::cli::storage) async fn run(args: &StorageCopyArgs) -> Result<(), CmdError> {
    copy_between(
        args.ends.source(),
        args.ends.destination(),
        CopyOptions {
            prefixes: args.prefix.clone(),
            concurrency: args.concurrency.get(),
        },
        args.dry_run,
        true,
    )
    .await
}

pub(in crate::cli::storage) async fn backup(args: &StorageBackupArgs) -> Result<(), CmdError> {
    let destination = Endpoint::configured_backup().ok_or_else(|| {
        CmdError::click(
            "no disaster-recovery store is configured; set WC_BACKUP_STORAGE_BACKEND and its locator",
        )
    })?;
    copy_between(
        Endpoint::configured_primary(),
        destination,
        CopyOptions {
            prefixes: args.prefix.clone(),
            concurrency: args.concurrency.get(),
        },
        args.dry_run,
        true,
    )
    .await
}

pub(crate) async fn copy_between(
    from: Endpoint,
    to: Endpoint,
    options: CopyOptions,
    dry_run: bool,
    warn_live: bool,
) -> Result<(), CmdError> {
    if from.describe() == to.describe() {
        return Err(CmdError::click(format!(
            "source and destination are the same store ({}); nothing to copy",
            from.describe()
        )));
    }
    // A copy moves bytes; it must never move the address they live at. The
    // object API is addressed by bare ecosystem keys and every bucket or
    // directory by namespace-qualified store paths, so crossing the two
    // rewrites every name in the set. That is not a hypothetical: it put
    // 9.6 GiB at `ecosystem/probierz/ecosystem/probierz/` in the store the
    // object API serves on charless-mac-mini, and bare `artifacts/`,
    // `status/` and `runs/` trees in that host's backup beside their
    // correctly-qualified twins. Both copies reported success.
    if from.keys_are_namespace_qualified() != to.keys_are_namespace_qualified() {
        let (qualified, bare) = if from.keys_are_namespace_qualified() {
            (from.describe(), to.describe())
        } else {
            (to.describe(), from.describe())
        };
        return Err(CmdError::click(format!(
            "{qualified} names objects by their namespace-qualified store path and {bare} names \
             them by bare ecosystem key, so copying between the two would re-address every \
             object: keys gain a second `ecosystem/<namespace>/` in one direction and lose the \
             one they have in the other. Copy to a store of the same kind, or address the same \
             store through one endpoint on both sides."
        )));
    }

    let source = from.build().await?;
    let destination = to.build().await?;

    println!("{} -> {}", from.describe(), to.describe());
    if dry_run {
        let plan = copy::plan(&source, &destination, &options).await?;
        print_plan(&plan);
        if warn_live {
            print_split_brain_warning();
        }
        return Ok(());
    }

    let report = copy::copy(&source, &destination, &options).await?;
    print_report(&report);
    if warn_live {
        print_split_brain_warning();
    }
    if !report.is_clean() {
        return Err(CmdError::click(format!(
            "{} object(s) failed to copy; the resume sentinel at {} was left at the last \
             clean prefix, so re-running continues from there",
            report.failed(),
            copy::SENTINEL_PATH
        )));
    }
    Ok(())
}

fn print_plan(plan: &CopyPlan) {
    println!("DRY RUN — nothing is written.");
    if !plan.resumed_from.is_empty() {
        println!(
            "Resume sentinel {} stops at {:?}; a real run would fast-forward past every \
             prefix up to and including it.",
            copy::SENTINEL_PATH,
            plan.resumed_from
        );
    }
    let rows: Vec<Vec<String>> = plan
        .prefixes
        .iter()
        .map(|row| {
            vec![
                row.prefix.clone(),
                row.source_objects.to_string(),
                row.already_at_destination.to_string(),
                if row.fast_forward {
                    "fast-forward".to_string()
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    print_table(&["PREFIX", "AT SOURCE", "AT DESTINATION", "RESUME"], &rows);
    let total: usize = plan.prefixes.iter().map(|row| row.source_objects).sum();
    println!(
        "\n{total} source object(s) across {} prefix(es).",
        plan.prefixes.len()
    );
}

fn print_report(report: &CopyReport) {
    if !report.resumed_from.is_empty() {
        println!(
            "Resumed after {:?} (sentinel {}).",
            report.resumed_from,
            copy::SENTINEL_PATH
        );
    }
    let rows: Vec<Vec<String>> = report
        .prefixes
        .iter()
        .map(|prefix| {
            vec![
                prefix.prefix.clone(),
                prefix.copied().to_string(),
                prefix.repaired().to_string(),
                prefix.skipped().to_string(),
                prefix.vanished().to_string(),
                prefix.failed().to_string(),
                prefix.bytes().to_string(),
            ]
        })
        .collect();
    print_table(
        &[
            "PREFIX",
            "COPIED",
            "META-FIXED",
            "SKIPPED",
            "VANISHED",
            "FAILED",
            "BYTES",
        ],
        &rows,
    );
    println!("\n{} byte(s) written.", report.bytes());

    let vanished: Vec<&str> = report
        .prefixes
        .iter()
        .flat_map(|prefix| prefix.objects.iter())
        .filter(|object| object.outcome == Outcome::Vanished)
        .map(|object| object.name.as_str())
        .collect();
    if !vanished.is_empty() {
        println!(
            "{} object(s) disappeared from the source mid-copy — the queue is LIVE.",
            vanished.len()
        );
    }

    if report.is_clean() {
        return;
    }
    let failed = report.failed();
    println!("\n{failed} failure(s):");
    for prefix in &report.prefixes {
        if let Some(error) = &prefix.listing_error {
            println!("  {}: {error}", prefix.prefix);
        }
        for object in prefix.failures() {
            if let Outcome::Failed(reason) = &object.outcome {
                println!("  {}: {reason}", object.name);
            }
        }
    }
}

/// The hazard the whole migration turns on. `deploy/MIGRATE_TO_STADO.md`
/// documents it as the reason the copy step is gated on a drained fleet.
fn print_split_brain_warning() {
    println!(
        "\nWARNING: copying a LIVE queue produces split-brain — a job claimed from the old \
         store, written to the new one, and reaped from neither. Drain the fleet first \
         (stop the coordinator tick and every agent, then confirm there are no queued and \
         no running jobs) and copy again immediately before the cutover. \
         See deploy/MIGRATE_TO_STADO.md."
    );
}
