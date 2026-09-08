use serde_json::{json, Value};

use crate::cli::host::checks::probes::print_json;

/// The `--json` document of [`super::backup_audit`].
pub(super) fn print_audit_document(
    audit: &crate::deploy::host_backup_audit::BackupAudit,
    reclaim_twins: bool,
    apply: bool,
) {
    let classes: serde_json::Map<String, Value> = audit
        .classes
        .iter()
        .map(|(class, totals)| {
            (
                class.clone(),
                json!({"objects": totals.objects, "bytes": totals.bytes}),
            )
        })
        .collect();
    let render_object = |object: &crate::deploy::host_backup_audit::ObjectComparison| {
        json!({
            "path": object.path,
            // These names identify the two fixed physical roots. They
            // deliberately do not claim which one is authoritative.
            "local_storage": {
                "state": object.primary.state,
                "bytes": object.primary.bytes,
                "sha256": object.primary.sha256,
            },
            "local_backup": {
                "state": object.backup.state,
                "bytes": object.backup.bytes,
                "sha256": object.backup.sha256,
            },
            "metadata": {
                "local_storage": {
                    "state": object.primary_metadata.state,
                    "bytes": object.primary_metadata.bytes,
                    "sha256": object.primary_metadata.sha256,
                },
                "local_backup": {
                    "state": object.backup_metadata.state,
                    "bytes": object.backup_metadata.bytes,
                    "sha256": object.backup_metadata.sha256,
                },
            },
        })
    };
    let objects = audit.objects.iter().map(render_object).collect::<Vec<_>>();
    let inventory_objects = audit
        .inventory_objects
        .iter()
        .map(render_object)
        .collect::<Vec<_>>();
    print_json(&json!({
        "host": audit.host,
        "complete": audit.complete,
        "unavailable": audit.unavailable,
        "classes": classes,
        "objects": objects,
        "inventory_objects": inventory_objects,
        "namespace_inventory": {
            "complete": audit.namespace_inventory_complete,
            "local_storage": audit.namespaces.get("local_storage").cloned().unwrap_or_default(),
            "local_backup": audit.namespaces.get("local_backup").cloned().unwrap_or_default(),
        },
        "reclaimable_bytes": audit.reclaimable_bytes(),
        "retained_bytes": audit.retained_bytes(),
        "reclaim": {
            "requested": reclaim_twins,
            "applied": reclaim_twins && apply,
            "complete": audit.reclaim_complete,
            "deleted_objects": audit.deleted.objects,
            "deleted_bytes": audit.deleted.bytes,
            "would_delete_objects": audit.would_delete.objects,
            "would_delete_bytes": audit.would_delete.bytes,
            "delete_failed_objects": audit.delete_failed.objects,
            "delete_failed_bytes": audit.delete_failed.bytes,
            "pruned_directories": audit.pruned_directories,
        },
        "free_kb_before": audit.free_kb_before,
        "free_kb_after": audit.free_kb_after,
    }));
}
