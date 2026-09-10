//! `stado host backup-audit` — classify a local replica against the store
//! it mirrors, and reclaim the twins when asked.

pub(in crate::cli::host) mod classes;
pub(in crate::cli::host) mod document;
pub(in crate::cli::host) mod listing;

use crate::cli::CmdError;

use crate::cli::host::files::storage::{BACKUP_ROOT, PRIMARY_ROOT};
use crate::cli::host::machine::users::credentials::credential_host;

/// Classify a host's local replica against the store it mirrors, and reclaim
/// the twins when asked.
///
/// The first time a tree on this fleet was assumed to be duplicate data it
/// turned out to be the only copy of 9.58 GiB, so classifying is the default
/// and it deletes nothing. A reclaim proves and deletes inside ONE pass: every
/// object it unlinks was hashed on both sides moments earlier by that same
/// pass. It never reads a verdict from a previous run, which is the shape that
/// turns a replica into data loss when addresses move between the audit and
/// the deletion — and they did move on this host, twice, in one evening.
pub async fn backup_audit(
    target: &str,
    object_uris: &[String],
    inventory_namespaces: &[String],
    reclaim_twins: bool,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    if (!object_uris.is_empty() || !inventory_namespaces.is_empty()) && (reclaim_twins || apply) {
        return Err(CmdError::click(
            "exact object inspection is read-only and cannot reclaim backup objects",
        ));
    }
    let _credential_authority = credential_host(target).await?;
    let namespace = crate::config::wc_stado_storage_namespace();
    if namespace.trim().is_empty() && object_uris.is_empty() && inventory_namespaces.is_empty() {
        return Err(CmdError::click(
            "this control plane has no storage.stado.namespace, so a replica path cannot be \
             resolved to a primary address",
        ));
    }
    let objects = object_uris
        .iter()
        .map(|uri| {
            crate::remote::object_store::ObjectRef::parse(uri)
                .map(|object| object.storage_path())
                .map_err(|error| CmdError::click(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for namespace in inventory_namespaces {
        crate::remote::object_store::ObjectRef::new(namespace, "inventory")
            .map_err(|error| CmdError::click(error.to_string()))?;
    }
    let inventory_namespaces = inventory_namespaces.to_vec();
    let plan = crate::deploy::host_backup_audit::AuditPlan {
        namespace: namespace.to_string(),
        backup_root: BACKUP_ROOT.to_string(),
        primary_root: PRIMARY_ROOT.to_string(),
        objects,
        inventory_namespaces,
        reclaim: reclaim_twins,
        apply,
    };
    let runner = crate::deploy::production_runner();
    let (_, audit) = crate::deploy::host_backup_audit::audit_host(target, &plan, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let gib = |bytes: u64| bytes as f64 / 1024.0 / 1024.0 / 1024.0;

    if json {
        document::print_audit_document(&audit, reclaim_twins, apply);
        return Ok(());
    }
    if !audit.inventory_objects.is_empty() {
        listing::print_inventory_objects(&audit);
        if !audit.complete {
            return Err(CmdError::click(audit.unavailable.unwrap_or_else(|| {
                "namespace inventory did not complete".to_string()
            })));
        }
        return Ok(());
    }
    if !audit.objects.is_empty() {
        listing::print_exact_objects(&audit);
        if !audit.complete {
            return Err(CmdError::click(
                "the host did not complete exact backup-object inspection",
            ));
        }
        return Ok(());
    }
    classes::print_classes(&audit, reclaim_twins, apply, &gib);
    Ok(())
}
