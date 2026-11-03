//! The disk janitor's cleanup lock a building product operation holds, and
//! how an operation asked to `--wait` takes the janitor's turn instead of
//! being refused for it.

use clap::ArgMatches;

use super::given;
use crate::cli::CmdError;
use crate::providers::local::space_reclaim as janitor;

/// The environment variable a building operation that holds the janitor's
/// lock leaves for every process it starts: its pid, a space, and the holder
/// it was taken as. An `install` runs its recipe's `stado product cargo
/// stage` and quality gates as children; each asked for a hold of its own,
/// and when the janitor had asked for its turn meanwhile (it waits on the
/// install's own hold) every child was refused, so an install never built.
pub const HELD: &str = "STADO_JANITOR_HOLD";

/// The holder an ancestor Stado that holds the janitor's lock names, when
/// this process runs under it: its hold lasts until it exits, which is after
/// this process, so the build directory is already kept from the janitor.
/// A value Stado did not write, or one naming a process that is not a
/// running Stado among this process's ancestors, holds nothing.
async fn held_above() -> Option<String> {
    let value = std::env::var(HELD).ok()?;
    let (pid, holder) = value.split_once(' ')?;
    if pid == std::process::id().to_string() {
        return None;
    }
    match crate::cli::release_loop::allowed::stado_ancestor(pid).await {
        Ok(true) => Some(holder.to_owned()),
        Ok(false) => None,
        Err(cause) => {
            eprintln!("stado: {HELD} names process {pid}, whose ancestry cannot be read ({cause}); this operation takes its own hold");
            None
        }
    }
}

/// The janitor's cleanup lock held in shared mode for the whole of a building
/// operation run here, as every job the agent runs holds it, or `None` for an
/// operation that builds nothing. Which operations build is the product
/// crate's declaration (`stado_product::cli::compiles`): a language server
/// that only answers an editor holds nothing for its life.
///
/// A product installation compiles into a per-workspace build directory the
/// janitor finds tagged as a regenerable cache. Without the hold a pass under
/// disk pressure deleted that directory mid-build, and the install failed
/// copying a binary that no longer existed. A janitor that holds the lock is
/// waited for, said as a lock wait, and the operation runs once the pass
/// lets go. One that has asked for its turn refuses the operation naming the
/// holds it waits on, before anything is exported or compiled, unless the
/// operation was asked to `--wait`: then it waits for the running workloads
/// to let go, runs the janitor's pass itself, and takes its hold after it.
/// The pass it runs walks the build caches before it takes the exclusive
/// lock, and a workload that takes a hold meanwhile turns the pass away and
/// makes it ask for the next turn; so a waiting operation waits for that turn
/// too, and for each one after it, until a pass has run and its own hold is
/// taken. Each round is said on stderr with what the pass reported.
pub(super) async fn janitor_hold(
    matches: &ArgMatches,
) -> Result<Option<janitor::WorkloadLock>, CmdError> {
    let Some((action, arguments)) = matches.subcommand() else {
        return Ok(None);
    };
    if !stado_product::cli::compiles(matches) {
        return Ok(None);
    }
    let operation = given(arguments, "operation")?;
    let mut holder = match operation {
        Some(operation) => format!("stado product {action} {operation}"),
        None => format!("stado product {action}"),
    };
    if let Some(product) = given(arguments, "positional")? {
        holder.push(' ');
        holder.push_str(product);
    }
    if let Some(surface) = given(arguments, "surface")? {
        holder.push_str(" --surface ");
        holder.push_str(surface);
    }
    // A process an operation holding the lock started is held by it: asking
    // again would be refused by the very turn request that waits on the
    // ancestor's hold.
    if let Some(above) = held_above().await {
        eprintln!("{holder}: the disk janitor's cleanup lock is held for it by {above}, the Stado operation that started it");
        return Ok(None);
    }
    let unheld = |error: janitor::JanitorError| {
        CmdError::click(format!(
            "{holder} cannot take the disk janitor's cleanup lock, so a cleanup pass could \
             delete its build directory while it compiles; nothing was exported or built: {error}"
        ))
    };
    // Set before the operation starts anything, so every process it runs
    // inherits the hold.
    let held = |lock: janitor::WorkloadLock| {
        std::env::set_var(HELD, format!("{} {holder}", std::process::id()));
        Ok(Some(lock))
    };
    if let Some(lock) = janitor::wait_workload_lock(&holder).map_err(unheld)? {
        return held(lock);
    }
    let waits = matches!(arguments.try_get_one::<bool>("wait"), Ok(Some(true)));
    if !waits {
        // The turn is transient: it ends when the pass it waits for has run.
        // It is stated as a wait an unchanged retry gets past (exit 69,
        // "retry later"), never as a rule refusing the request, which told
        // the caller that retrying would not help.
        return Err(CmdError::click(format!(
            "{holder} builds into a directory the disk janitor reclaims, and the janitor has \
             asked running work for its turn; nothing was exported or built. It waits on: {}. \
             Run it again once that pass has run, or with --wait to wait for that work, run \
             the janitor's pass and then build",
            janitor::workload_holders()
        ))
        .stating(crate::primitives::failure::FailureCode::RateLimit));
    }
    loop {
        janitor::wait_for_janitor_turn(&holder).map_err(unheld)?;
        let running = janitor::live_workload_count().map_err(unheld)?;
        let report = janitor::run_cleanup_once(
            running,
            janitor::CleanupWriter::Cli { every: None },
            &mut |line: &str| eprintln!("{line}"),
        )
        .await;
        if let Some(lock) = janitor::wait_workload_lock(&holder).map_err(unheld)? {
            return held(lock);
        }
        eprintln!(
            "{holder}: the disk janitor asks for its turn again after the pass it waited for \
             ({report}); it waits for that turn too. It waits on: {}",
            janitor::workload_holders()
        );
    }
}
