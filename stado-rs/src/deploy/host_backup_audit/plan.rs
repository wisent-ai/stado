//! What the operator asked for, and the program text that carries it: the
//! markers substituted into the remote program, the hashing budgets the two
//! kinds of pass get, and the plan itself.

use super::remote_program::REMOTE_SCRIPT_TEMPLATE;

/// Marker for the namespace the bare-path arm maps into.
const NAMESPACE_MARK: &str = "@NAMESPACE@";
/// Marker for the replica root, relative to the remote home.
const BACKUP_ROOT_MARK: &str = "@BACKUP_ROOT@";
/// Marker for the primary store root, relative to the remote home.
const PRIMARY_ROOT_MARK: &str = "@PRIMARY_ROOT@";
/// Marker for exact qualified object paths, encoded as comma-separated hex.
const OBJECTS_HEX_MARK: &str = "@OBJECTS_HEX@";
/// Marker for namespace names whose backup-visible object metadata is listed.
const INVENTORY_NAMESPACES_HEX_MARK: &str = "@INVENTORY_NAMESPACES_HEX@";

/// How long a READ-ONLY pass may spend hashing, in seconds.
///
/// The fleet channel gives every script 120 seconds, and this replica is
/// 48.5 GiB — far more than `shasum` can read in that window on a host that is
/// also running jobs. So the size comparison, which is one `stat` per file and
/// decides [`ABSENT`](super::ABSENT) and most of [`DIFFERS`](super::DIFFERS)
/// outright, always completes; the
/// hashing that proves a twin runs until this deadline and then stops, leaving
/// the rest honestly labelled. Repeated runs make more of it provable as the
/// twin set shrinks under whatever the operator then reclaims.
const HASH_DEADLINE_SECONDS: u64 = 70;

/// How long a RECLAIM pass may spend hashing, and how long the channel waits
/// for it.
///
/// A reclaim proves every object it deletes inside the same pass, so it has to
/// read both copies of everything it intends to drop — twice 38.47 GiB on this
/// host. Under the read-only budget it would prove almost nothing and delete
/// almost nothing, and the temptation would then be to delete against the
/// previous run's recorded verdict, which is exactly the mistake that turns a
/// replica into data loss. So the pass gets a budget that fits the work.
const RECLAIM_HASH_DEADLINE_SECONDS: u64 = 1500;
/// Wall clock for a reclaim pass on the channel, above its hashing deadline so
/// the program's own deadline is what stops it and the totals still come back.
pub(super) const RECLAIM_TIMEOUT_SECONDS: u64 = 1800;

/// One pass over one host's replica, as the operator asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPlan {
    /// Namespace a bare replica path maps into on the primary side.
    pub namespace: String,
    /// Replica root, relative to the remote login user's `$HOME`.
    pub backup_root: String,
    /// Primary store root, relative to the same `$HOME`.
    pub primary_root: String,
    /// Exact namespace-qualified object paths to compare. Empty scans the
    /// replica as before.
    pub objects: Vec<String>,
    /// Namespaces whose backup-visible object paths and size metadata should be
    /// listed without reading object bodies.
    pub inventory_namespaces: Vec<String>,
    /// Also delete the twins this pass proves.
    pub reclaim: bool,
    /// Actually delete. Without it a reclaim names what it would drop and
    /// drops nothing.
    pub apply: bool,
}

impl AuditPlan {
    /// The hashing budget this pass needs: a reclaim must prove everything it
    /// deletes, a read-only pass may stop early and label the rest.
    fn hash_deadline_seconds(&self) -> u64 {
        if self.reclaim {
            RECLAIM_HASH_DEADLINE_SECONDS
        } else {
            HASH_DEADLINE_SECONDS
        }
    }
}

/// The remote program with this host's roots, namespace, hashing deadline and
/// reclaim mode in place.
pub fn remote_script(plan: &AuditPlan) -> String {
    REMOTE_SCRIPT_TEMPLATE
        .replace(NAMESPACE_MARK, &plan.namespace)
        .replace(BACKUP_ROOT_MARK, &plan.backup_root)
        .replace(PRIMARY_ROOT_MARK, &plan.primary_root)
        .replace(
            OBJECTS_HEX_MARK,
            &plan
                .objects
                .iter()
                .map(hex::encode)
                .collect::<Vec<_>>()
                .join(","),
        )
        .replace(
            INVENTORY_NAMESPACES_HEX_MARK,
            &plan
                .inventory_namespaces
                .iter()
                .map(hex::encode)
                .collect::<Vec<_>>()
                .join(","),
        )
        .replace("@HASH_DEADLINE@", &plan.hash_deadline_seconds().to_string())
        .replace("@RECLAIM@", if plan.reclaim { "yes" } else { "no" })
        // A pass that was not asked to reclaim cannot apply anything, whatever
        // else it was handed.
        .replace(
            "@APPLY@",
            if plan.reclaim && plan.apply {
                "yes"
            } else {
                "no"
            },
        )
}
