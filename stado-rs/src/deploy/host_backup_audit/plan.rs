//! What the operator asked for, and the program text that carries it: the
//! markers substituted into the remote program and the plan itself.

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

/// The remote program with this host's roots, namespace and reclaim mode in
/// place.
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
