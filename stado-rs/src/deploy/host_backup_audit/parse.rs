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
                let (Some(class), Some(size), Some(_)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    audit.unavailable = Some(format!(
                        "the audit printed a class row that does not read: {line}"
                    ));
                    continue;
                };
                if !matches!(class, TWIN | DIFFERS | ABSENT | SAME_SIZE_UNPROVEN) {
                    audit.unavailable = Some(format!(
                        "the audit printed a class this Stado does not know: {line}"
                    ));
                    continue;
                }
                // A size that does not read was counted as zero bytes, so a
                // class could look smaller than it is; the audit is marked
                // unavailable with the line instead.
                let Ok(bytes) = size.trim().parse::<u64>() else {
                    audit.unavailable = Some(format!(
                        "the audit printed a size that does not read: {line}"
                    ));
                    continue;
                };
                let totals = audit.classes.entry(class.to_string()).or_default();
                totals.objects += 1;
                totals.bytes += bytes;
            }
            Some("STADO_BACKUP_OBJECT") => {
                // A requested object the reader cannot read leaves the audit
                // incomplete; dropped, the operator asked about an object and
                // was told nothing.
                let Some(path) = fields.next() else {
                    audit.unavailable = Some(format!(
                        "the audit printed an object row that does not read: {line}"
                    ));
                    continue;
                };
                match object_identities(path.to_string(), &mut fields, line) {
                    Ok(object) => audit.objects.push(object),
                    Err(detail) => audit.unavailable = Some(detail),
                }
            }
            Some("STADO_BACKUP_INVENTORY_OBJECT") => {
                // A row the audit printed and this reader cannot read leaves
                // the inventory incomplete; dropped, the audit compared fewer
                // objects and reported the rest matched.
                let Some(path) = fields
                    .next()
                    .and_then(|encoded| hex::decode(encoded).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                else {
                    audit.unavailable = Some(format!(
                        "the audit printed an object path that is not hex-encoded UTF-8: {line}"
                    ));
                    continue;
                };
                match object_identities(path, &mut fields, line) {
                    Ok(object) => audit.inventory_objects.push(object),
                    Err(detail) => audit.unavailable = Some(detail),
                }
            }
            Some("STADO_BACKUP_NAMESPACE") => {
                let (Some(root), Some(encoded)) = (fields.next(), fields.next()) else {
                    audit.unavailable = Some(format!(
                        "the audit printed a namespace row that does not read: {line}"
                    ));
                    continue;
                };
                let Some(namespace) = hex::decode(encoded)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                else {
                    audit.unavailable = Some(format!(
                        "the audit printed a namespace that is not hex-encoded UTF-8: {line}"
                    ));
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
                // The probe names the root and its failure; a line that
                // carries neither is said as such rather than as root
                // "unknown".
                audit.unavailable = Some(match (fields.next(), fields.next()) {
                    (Some(root), Some(detail)) => format!("{root}: {detail}"),
                    (Some(root), None) => {
                        format!("{root}: namespace inventory failed without a reason")
                    }
                    (None, _) => {
                        "namespace inventory failed, and the probe named no root".to_string()
                    }
                });
            }
            Some("STADO_BACKUP_AUDIT_UNAVAILABLE") => {
                audit.unavailable = fields.next().map(str::to_string);
            }
            Some("STADO_BACKUP_AUDIT_END") => audit.complete = true,
            Some("STADO_BACKUP_RECLAIM") => {
                let (Some(outcome), Some(size), Some(_path)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    audit.unavailable = Some(format!(
                        "the reclaim printed a row that does not read: {line}"
                    ));
                    continue;
                };
                let Ok(bytes) = size.trim().parse::<u64>() else {
                    audit.unavailable = Some(format!(
                        "the reclaim printed a size that does not read: {line}"
                    ));
                    continue;
                };
                let totals = match outcome {
                    "deleted" => &mut audit.deleted,
                    "would_delete" => &mut audit.would_delete,
                    "delete_failed" => &mut audit.delete_failed,
                    _ => {
                        audit.unavailable = Some(format!(
                            "the reclaim printed an outcome this Stado does not know: {line}"
                        ));
                        continue;
                    }
                };
                totals.objects += 1;
                totals.bytes += bytes;
            }
            Some("STADO_BACKUP_RECLAIM_END") => audit.reclaim_complete = true,
            Some("STADO_BACKUP_PRUNED") => {
                match fields.next().map(|count| count.trim().parse::<i64>()) {
                    Some(Ok(count)) => audit.pruned_directories = count,
                    _ => {
                        audit.unavailable = Some(format!(
                            "the prune printed a directory count that does not read: {line}"
                        ))
                    }
                }
            }
            Some("STADO_BACKUP_FREE") => {
                // The free space the pass read on each side of its deletions;
                // a reading that does not read is said, never shown as unread.
                let reading = match (fields.next(), fields.next()) {
                    (Some(phase), Some(blocks)) => {
                        crate::deploy::host_channel::marker_number::<i64>(
                            blocks,
                            "free block count",
                        )
                        .map(|blocks| (phase, blocks))
                    }
                    _ => Err("it names no phase or block count".to_string()),
                };
                match reading {
                    Ok(("before", blocks)) => audit.free_kb_before = blocks,
                    Ok(("after", blocks)) => audit.free_kb_after = blocks,
                    Ok(_) => {
                        audit.unavailable = Some(format!(
                            "the audit printed a free-space phase this Stado does not know: {line}"
                        ))
                    }
                    Err(detail) => {
                        audit.unavailable = Some(format!(
                            "the audit printed a free-space row where {detail}: {line}"
                        ))
                    }
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

/// The four identities of one object row, in the order the program prints
/// them after the path: state, byte count and digest of the primary, of the
/// backup and of their two metadata files. A row with fewer fields, or a byte
/// count that is not a number, is refused quoting the line; an empty or `-`
/// byte count is an object the program found no size for.
fn object_identities(
    path: String,
    fields: &mut std::str::Split<'_, char>,
    line: &str,
) -> Result<ObjectComparison, String> {
    let mut identity = || -> Result<ObjectIdentity, String> {
        let (Some(state), Some(bytes), Some(sha256)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!(
                "the audit printed an object row that does not read: {line}"
            ));
        };
        Ok(ObjectIdentity {
            state: state.to_string(),
            bytes: crate::deploy::host_channel::marker_number(bytes, "byte count").map_err(
                |detail| format!("the audit printed an object row where {detail}: {line}"),
            )?,
            sha256: (!sha256.is_empty()).then(|| sha256.to_string()),
        })
    };
    Ok(ObjectComparison {
        path,
        primary: identity()?,
        backup: identity()?,
        primary_metadata: identity()?,
        backup_metadata: identity()?,
    })
}
