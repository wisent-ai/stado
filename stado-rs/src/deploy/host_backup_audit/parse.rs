//! Fold the remote program's `STADO_*` marker lines into the reading.

use std::collections::BTreeSet;

use super::{
    BackupAudit, ObjectComparison, ObjectIdentity, ABSENT, DIFFERS, SAME_SIZE_UNPROVEN, TWIN,
};

/// Parse the remote program's output.
pub fn parse_output(stdout: &str, host: &str) -> BackupAudit {
    let mut audit = BackupAudit {
        host: host.to_string(),
        ..BackupAudit::default()
    };
    let mut namespace_roots_complete = BTreeSet::new();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("STADO_BACKUP_AUDIT") => {
                let (Some(class), Some(size), Some(path)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                if !matches!(class, TWIN | DIFFERS | ABSENT | SAME_SIZE_UNPROVEN) {
                    continue;
                }
                let bytes = size.trim().parse::<u64>().unwrap_or_default();
                let totals = audit.classes.entry(class.to_string()).or_default();
                totals.objects += 1;
                totals.bytes += bytes;
                let examples = audit.examples.entry(class.to_string()).or_default();
                examples.push((bytes, path.to_string()));
                examples.sort_by_key(|(bytes, _)| std::cmp::Reverse(*bytes));
                examples.truncate(5);
            }
            Some("STADO_BACKUP_OBJECT") => {
                let (
                    Some(path),
                    Some(primary_state),
                    Some(primary_bytes),
                    Some(primary_sha256),
                    Some(backup_state),
                    Some(backup_bytes),
                    Some(backup_sha256),
                    Some(primary_metadata_state),
                    Some(primary_metadata_bytes),
                    Some(primary_metadata_sha256),
                    Some(backup_metadata_state),
                    Some(backup_metadata_bytes),
                    Some(backup_metadata_sha256),
                ) = (
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                )
                else {
                    continue;
                };
                let identity = |state: &str, bytes: &str, sha256: &str| ObjectIdentity {
                    state: state.to_string(),
                    bytes: bytes.parse().ok(),
                    sha256: (!sha256.is_empty()).then(|| sha256.to_string()),
                };
                audit.objects.push(ObjectComparison {
                    path: path.to_string(),
                    primary: identity(primary_state, primary_bytes, primary_sha256),
                    backup: identity(backup_state, backup_bytes, backup_sha256),
                    primary_metadata: identity(
                        primary_metadata_state,
                        primary_metadata_bytes,
                        primary_metadata_sha256,
                    ),
                    backup_metadata: identity(
                        backup_metadata_state,
                        backup_metadata_bytes,
                        backup_metadata_sha256,
                    ),
                });
            }
            Some("STADO_BACKUP_INVENTORY_OBJECT") => {
                let (
                    Some(encoded_path),
                    Some(primary_state),
                    Some(primary_bytes),
                    Some(primary_sha256),
                    Some(backup_state),
                    Some(backup_bytes),
                    Some(backup_sha256),
                    Some(primary_metadata_state),
                    Some(primary_metadata_bytes),
                    Some(primary_metadata_sha256),
                    Some(backup_metadata_state),
                    Some(backup_metadata_bytes),
                    Some(backup_metadata_sha256),
                ) = (
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                    fields.next(),
                )
                else {
                    continue;
                };
                let Ok(path_bytes) = hex::decode(encoded_path) else {
                    continue;
                };
                let Ok(path) = String::from_utf8(path_bytes) else {
                    continue;
                };
                let identity = |state: &str, bytes: &str, sha256: &str| ObjectIdentity {
                    state: state.to_string(),
                    bytes: bytes.parse().ok(),
                    sha256: (!sha256.is_empty()).then(|| sha256.to_string()),
                };
                audit.inventory_objects.push(ObjectComparison {
                    path,
                    primary: identity(primary_state, primary_bytes, primary_sha256),
                    backup: identity(backup_state, backup_bytes, backup_sha256),
                    primary_metadata: identity(
                        primary_metadata_state,
                        primary_metadata_bytes,
                        primary_metadata_sha256,
                    ),
                    backup_metadata: identity(
                        backup_metadata_state,
                        backup_metadata_bytes,
                        backup_metadata_sha256,
                    ),
                });
            }
            Some("STADO_BACKUP_NAMESPACE") => {
                let (Some(root), Some(encoded)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let Ok(bytes) = hex::decode(encoded) else {
                    continue;
                };
                let Ok(namespace) = String::from_utf8(bytes) else {
                    continue;
                };
                audit
                    .namespaces
                    .entry(root.to_string())
                    .or_default()
                    .push(namespace);
            }
            Some("STADO_BACKUP_NAMESPACES_END") => {
                if let Some(root) = fields.next() {
                    namespace_roots_complete.insert(root.to_string());
                }
            }
            Some("STADO_BACKUP_NAMESPACES_ERROR") => {
                let root = fields.next().unwrap_or("unknown");
                let detail = fields.next().unwrap_or("namespace inventory failed");
                audit.unavailable = Some(format!("{root}: {detail}"));
            }
            Some("STADO_BACKUP_AUDIT_UNAVAILABLE") => {
                audit.unavailable = fields.next().map(str::to_string);
            }
            Some("STADO_BACKUP_AUDIT_END") => audit.complete = true,
            Some("STADO_BACKUP_RECLAIM") => {
                let (Some(outcome), Some(size), Some(_path)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                let bytes = size.trim().parse::<u64>().unwrap_or_default();
                let totals = match outcome {
                    "deleted" => &mut audit.deleted,
                    "would_delete" => &mut audit.would_delete,
                    "delete_failed" => &mut audit.delete_failed,
                    _ => continue,
                };
                totals.objects += 1;
                totals.bytes += bytes;
            }
            Some("STADO_BACKUP_RECLAIM_END") => audit.reclaim_complete = true,
            Some("STADO_BACKUP_PRUNED") => {
                audit.pruned_directories = fields
                    .next()
                    .and_then(|count| count.trim().parse::<i64>().ok())
                    .unwrap_or_default();
            }
            Some("STADO_BACKUP_FREE") => {
                let (Some(phase), Some(blocks)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let blocks = blocks.trim().parse::<i64>().ok();
                match phase {
                    "before" => audit.free_kb_before = blocks,
                    "after" => audit.free_kb_after = blocks,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    for namespaces in audit.namespaces.values_mut() {
        namespaces.sort();
        namespaces.dedup();
    }
    audit.namespace_inventory_complete = ["local_storage", "local_backup"]
        .iter()
        .all(|root| namespace_roots_complete.contains(*root));
    audit
}
