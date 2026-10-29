//! What a pass may delete on this host, from the ownership its registry
//! entry declares.
//!
//! On a dedicated host every candidate is the fleet's to reclaim. On a
//! shared host (a person's own computer that also runs fleet work) the
//! caches, project build trees, model downloads and Time Machine snapshots in
//! the home are that person's, and a pass deletes only inside Stado's own
//! tree; every other candidate is still measured and reported, so the volume
//! report keeps saying what holds the disk, and kept. A host whose entry
//! cannot be read, or declares an ownership no word names, is treated as
//! shared: whose files they are is then not known.

use std::path::{Path, PathBuf};

use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;
use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::targets::{ComputeTarget, HostOwnership};

/// Stado's own tree under the home.
const STADO_TREE: &str = ".stado";
/// The skip reason a kept candidate outside Stado's tree is counted under.
pub(crate) const NOT_STADO_OWNED: &str = "shared_host_not_stado_owned";

/// `None` when every candidate may be deleted; otherwise the root of the one
/// tree a pass deletes in, the reason for a fail-closed answer recorded on
/// `report`.
pub(crate) fn deletable_root(
    target: Option<&ComputeTarget>,
    home: &Path,
    report: &mut CleanupReport,
) -> Option<PathBuf> {
    let stado = home.join(STADO_TREE);
    let Some(target) = target else {
        report.add_error(
            "ownership",
            &JanitorError::os(
                "this host's registry entry could not be read, so whose files its home holds is \
                 not known; only Stado's own tree is cleaned",
            ),
        );
        return Some(stado);
    };
    match target.ownership() {
        Ok(HostOwnership::Dedicated) => None,
        Ok(HostOwnership::Shared) => Some(stado),
        Err(refused) => {
            report.add_error(
                "ownership",
                &JanitorError::os(&format!("{refused}; only Stado's own tree is cleaned")),
            );
            Some(stado)
        }
    }
}

/// Whether a candidate at `path` may be deleted under `root`.
pub(crate) fn may_delete(root: Option<&Path>, path: &Path) -> bool {
    root.is_none_or(|root| path.starts_with(root))
}
