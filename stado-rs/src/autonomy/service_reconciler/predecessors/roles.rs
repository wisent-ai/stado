//! Whether a declared unit's work already runs inside its product's one
//! process on the same host, asked before the pass repairs that unit.
//!
//! Retiring a role unit does not remove its registry declaration, so on the
//! next pass it reads `missing` and would be reasserted beside the process
//! that took its role over. The pass asks the same question retirement asks,
//! [`service::role_taken_over`], so a unit is never both retired and repaired
//! on one host.

use crate::deploy::service::{self, ManagedService};
use crate::deploy::Runner;

use super::Replacement;

/// `Some(detail)` when `declared` is a role unit of a replacement running on
/// its host and that replacement's live process is proven to run the role;
/// `None` when the unit is still the only thing doing its work, including
/// when that cannot be established.
pub(in crate::autonomy::service_reconciler) async fn taken_over(
    declared: &ManagedService,
    replacements: &[Replacement],
    runner: &Runner,
) -> Option<String> {
    let unit = declared.unit_id();
    for (running, entry) in replacements {
        if running.host != declared.host {
            continue;
        }
        let Some(role) = entry
            .role_units
            .iter()
            .find(|role| role.unit == unit || role.unit == declared.name)
        else {
            continue;
        };
        let target = crate::deploy::host_channel::canonical_target(&running.host)
            .await
            .ok()?;
        if let Ok(None) = service::role_taken_over(&target, running, &role.flag, runner).await {
            return Some(format!(
                "{unit} is retired on {}: {} runs its role ({}) inside {}",
                running.host,
                entry.name,
                role.flag,
                running.unit_id()
            ));
        }
    }
    None
}
