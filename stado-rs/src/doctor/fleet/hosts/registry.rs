//! The canonical registry, and whether it can place this host at all.

use crate::doctor::Check;
use crate::providers;
use crate::targets;

// ---------------------------------------------------------------------------
// 8. Registry
// ---------------------------------------------------------------------------

pub(in crate::doctor) const REGISTRY_ID: &str = "registry";
pub(in crate::doctor) const REGISTRY_TITLE: &str = "Registry";
pub(in crate::doctor) const REGISTRY_REMEDY: &str =
    "`stado registry pull` shows what the canonical registry says and `stado registry self` \
     resolves this host; add or rename the entry, then `stado registry validate` and \
     `stado registry push`";

/// The canonical registry must be reachable, must parse, and must know
/// either this host or an active coordinator. An unreachable registry is
/// an error rather than an empty one — [`targets::fetch_registry_remote`]
/// draws that distinction because "the store is down" and "you were
/// removed from the fleet" demand opposite responses.
pub(in crate::doctor) async fn check_registry() -> Check {
    let registry = match targets::fetch_registry_remote().await {
        Ok(registry) => registry,
        Err(err) => {
            return Check::fail(
                REGISTRY_ID,
                REGISTRY_TITLE,
                err.to_string(),
                REGISTRY_REMEDY,
            )
        }
    };
    let hostname = providers::vast::system_hostname();
    let coordinators: Vec<&str> = registry
        .coordinators
        .iter()
        .filter(|coordinator| coordinator.active)
        .map(|coordinator| coordinator.name.as_str())
        .collect();
    let shape = format!(
        "{} target(s), {} coordinator(s)",
        registry.targets.len(),
        registry.coordinators.len()
    );

    match registry.lookup_self(&hostname) {
        Err(err) => Check::fail(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!("parsed ({shape}) but this host's identity is not resolvable: {err}"),
            REGISTRY_REMEDY,
        ),
        Ok(Some(target)) => Check::pass(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!(
                "reachable, parsed ({shape}); {hostname} is target {:?} of kind {}",
                target.name, target.kind
            ),
            REGISTRY_REMEDY,
        ),
        Ok(None) if !coordinators.is_empty() => Check::pass(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!(
                "reachable, parsed ({shape}); {hostname} is not a target, active \
                 coordinator(s): {}",
                coordinators.join(",")
            ),
            REGISTRY_REMEDY,
        ),
        Ok(None) => Check::fail(
            REGISTRY_ID,
            REGISTRY_TITLE,
            format!(
                "reachable and parsed ({shape}) but names neither {hostname} nor any active \
                 coordinator; a daemon started here fails its identity lookup and exits on \
                 every respawn"
            ),
            REGISTRY_REMEDY,
        ),
    }
}
