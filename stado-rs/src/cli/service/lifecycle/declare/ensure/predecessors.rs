//! After a catalog product's unit is ensured, the units it replaced are
//! retired on the same host, so asserting the product is also what takes its
//! predecessors down.

use super::*;

/// Retire every unit `entry` lists in `retired_units` on `target` and say
/// what happened to each on stderr, leaving the command's JSON contract
/// unchanged. A unit that could not be retired is the command's failure: the
/// replacement runs, but its predecessor may run beside it.
pub(super) async fn retire_after_ensure(
    target: &crate::targets::ComputeTarget,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let Some(entry) = entry else { return Ok(()) };
    let mut failed = Vec::new();
    for retirement in service::retire_catalog_predecessors(target, entry, runner).await {
        eprintln!(
            "{}: {} replaced {}: {} ({})",
            target.name, entry.name, retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}: {} is running, but the units it replaced could not all be retired: {}",
        target.name,
        entry.name,
        failed.join("; ")
    )))
}
