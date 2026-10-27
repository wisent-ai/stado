//! The ghost-VM delete the running-jobs pass performs after it wins a
//! requeue: find the reaped host's full ref, from the tick's cache or a fresh
//! listing, and kill the trainer that would otherwise keep writing.

use std::collections::BTreeMap;

use crate::providers::Provider;

use super::super::log;

/// Best-effort delete of the VM named `hostname`.
///
/// A requeue that moves a job from running back to the queue leaves the
/// prior agent's process running on the VM unless the VM is deleted, and a
/// second dispatch of the same job would then write beside it. This deletes
/// the VM the requeue gave up on.
///
/// Looks up the full <name>@<zone> ref from `vm_cache` (a map
/// {hostname: full_ref} built by the caller, or none when the caller built
/// none) and lists the fleet fresh on a miss, so a VM a transient listing
/// left out is still found. Never raises; returns True iff a delete call
/// returned cleanly. Idempotent and safe on a VM that is truly gone (returns
/// False).
pub(super) async fn safe_delete_vm_by_hostname(
    provider: &dyn Provider,
    hostname: &str,
    vm_cache: Option<&BTreeMap<String, String>>,
) -> bool {
    let mut full_ref = vm_cache.and_then(|cache| cache.get(hostname)).cloned();
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
