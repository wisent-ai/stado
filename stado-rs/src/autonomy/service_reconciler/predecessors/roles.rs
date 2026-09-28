//! Whether a declared unit's work already runs inside its product's one
//! process on the same host, asked before the pass repairs that unit.
//!
//! Retiring a role unit does not remove its registry declaration, so on the
//! next pass it reads `missing` and would be reasserted beside the process
//! that took its role over. The pass asks the same question retirement asks,
//! [`service::role_retired`], so a unit is never both retired and repaired
//! on one host, and a unit handed over to a resolver that has not answered
//! yet is not brought back before that resolver has tried.

use crate::deploy::service::{self, ManagedService};
use crate::deploy::Runner;

use super::Replacement;

/// `Some(detail)` when `declared` is a role unit of a replacement on its host
/// and that replacement took the role over: its live process is proven to run
/// an ordinary role, or, for a role that shares its unit's listener, the
/// host's handoff record says the listener was acquired or the unit stepped
/// aside and the resolver has not answered. A listener role is asked even
/// while the replacement is not running, because an acquired listener stays
/// acquired across the replacement's restarts. `None` when the unit is still
/// the only thing doing its work, including when that cannot be established.
pub(in crate::autonomy::service_reconciler) async fn taken_over(
    declared: &ManagedService,
    replacements: &[Replacement],
    runner: &Runner,
) -> Option<String> {
    let unit = declared.unit_id();
    for super::Replacement {
        service: running,
        entry,
        active,
    } in replacements
    {
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
        if !active && !service::listener_role(role) {
            continue;
        }
        let target = crate::deploy::host_channel::canonical_target(&running.host)
            .await
            .ok()?;
        if let Some(proof) = service::role_retired(&target, running, role, !active, runner).await {
            return Some(format!(
                "{unit} is retired on {}: {} took its role over ({proof})",
                running.host, entry.name
            ));
        }
    }
    None
}
