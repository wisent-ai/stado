//! The ghost-VM delete the running-jobs pass performs after it wins a
//! requeue: find the reaped host's full ref, from the tick's cache or a fresh
//! listing, and kill the trainer that would otherwise keep writing.

use std::collections::BTreeMap;

use crate::providers::Provider;

use super::super::log;

/// Best-effort delete of a GCE VM named `hostname`.
///
/// The requeue paths in check_running_jobs (orphan + VM-gone) previously
/// moved running -> queue without calling provider.delete_instance, so
/// the prior agent's training subprocess kept running on the
/// supposedly-gone VM and producing duplicate writes against the same
/// gs://wisent-compute/ckpts/<run>/ path. Confirmed live 2026-05-18 for
/// job 724084db:
/// 4 concurrent trainers (workstation + 3 GCP VMs) all
/// racing on the same ckpt prefix because each transient "VM missing
/// from fleet listing" requeue spawned a new dispatch without
/// terminating the old subprocess.
///
/// Looks up the full <name>@<zone> ref from `vm_cache` (a dict
/// {hostname: full_ref} built by the caller) and falls back to a fresh
/// list-and-search on cache miss (handles the exact transient-miss case
/// that caused the ghost-trainer bug). Never raises; returns True iff
/// a delete call returned cleanly. Idempotent and safe on a VM that is
/// truly gone (returns False).
pub(super) async fn safe_delete_vm_by_hostname(
    provider: &dyn Provider,
    hostname: &str,
    vm_cache: &BTreeMap<String, String>,
) -> bool {
    let mut full_ref = vm_cache.get(hostname).cloned();
    if full_ref.is_none() {
        match provider.list_running_instance_refs_with_age().await {
            Ok(refs) => {
                for (r, _age) in refs {
                    if r.split('@').next() == Some(hostname) {
                        full_ref = Some(r);
                        break;
                    }
                }
            }
            Err(e) => {
                log(&format!(
                    "safe_delete: fresh list failed for {hostname}: {e:?}"
                ));
                return false;
            }
        }
    }
    let Some(full_ref) = full_ref else {
        return false;
    };
    match provider.delete_instance(&full_ref).await {
        Ok(()) => {
            log(&format!("safe_delete: killed ghost VM {full_ref}"));
            true
        }
        Err(e) => {
            log(&format!("safe_delete({full_ref}) failed: {e:?}"));
            false
        }
    }
}
