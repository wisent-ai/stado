//! Store-side ownership index: the half of the cross-check that answers
//! "does anything still claim this VM?" from the store alone — the `running/`
//! job documents and the un-released `provider-leases/` blobs — so the fleet
//! side only has to say which VMs exist.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use futures::StreamExt;

use crate::cli::CmdError;
use crate::queue::leases::{LeaseError, LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::migrations::BULK_WORKERS;
use crate::queue::JobStorage;

/// `JobStorage::list_jobs` / `list_paths` take `oldest_first = 0` for "no
/// bound" (Python `limit=None`); naming the sentinel keeps the call sites
/// honest about what zero means there.
const UNBOUNDED: usize = usize::MIN;

/// The blob prefix `queue/leases.rs::ProviderLeaseStore::path` writes under.
/// Already carried by `queue/copy.rs::CANONICAL_PREFIXES`; these commands
/// only read it.
const LEASE_PREFIX: &str = "provider-leases/";

/// Suffix of a lease blob name (`provider-leases/{job_id}.json`).
const LEASE_SUFFIX: &str = ".json";

/// Everything in the store that can legitimately hold an agent VM, keyed by
/// the reference string the holder itself recorded.
pub(super) struct Holders {
    /// reference-as-recorded -> human reasons ("job 1a2b3c4d", "lease ...").
    reasons: BTreeMap<String, Vec<String>>,
    /// reference-as-recorded -> the owning job's `gpu_type`.
    gpu_types: BTreeMap<String, String>,
}

impl Holders {
    pub(super) async fn build(store: &JobStorage) -> Result<Self, CmdError> {
        let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut gpu_types: BTreeMap<String, String> = BTreeMap::new();

        // Running job documents. `instance_ref` is written by the dispatcher
        // as the provider ref and by local agents as "local@<hostname>";
        // both forms are matched in `Holders::keys`.
        for job in store.list_jobs("running", UNBOUNDED).await? {
            let Some(reference) = job.instance_ref.filter(|value| !value.is_empty()) else {
                continue;
            };
            reasons
                .entry(reference.clone())
                .or_default()
                .push(format!("job {}", job.job_id));
            if !job.gpu_type.is_empty() {
                gpu_types.entry(reference).or_insert(job.gpu_type);
            }
        }

        // Provider leases. `queue/leases.rs::ProviderLeaseStore` has no bulk
        // listing, so the job ids come from the blob names and each lease is
        // then loaded through the public `load` (which owns decoding and the
        // size bound).
        let lease_store = ProviderLeaseStore::new(store.clone());
        let job_ids: Vec<String> = store
            .list_paths(LEASE_PREFIX, UNBOUNDED)
            .await?
            .iter()
            .filter_map(|path| lease_job_id(path))
            .collect();
        // A lease that cannot be read is not a lease that can be ignored:
        // it may be the only record of who owns a VM, so the whole
        // inventory fails rather than authorizing a deletion on a partial
        // ownership picture.
        let loaded: Vec<Option<ProviderLease>> = futures::stream::iter(&job_ids)
            .map(|job_id| lease_store.load(job_id))
            .buffered(BULK_WORKERS)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<Option<ProviderLease>>, _>>()
            .map_err(|err: LeaseError| CmdError::click(err.to_string()))?;
        for lease in loaded.into_iter().flatten() {
            if lease.provider_resource_id.is_empty() || !lease_holds_resource(&lease) {
                continue;
            }
            reasons
                .entry(lease.provider_resource_id.clone())
                .or_default()
                .push(format!("lease {} ({})", lease.job_id, lease.state));
        }

        Ok(Holders { reasons, gpu_types })
    }

    /// Every string a holder may have recorded for one VM: the provider's
    /// own `name@zone`, the `local@<hostname>` form an agent stamps onto the
    /// job it claims (both are checked by
    /// `monitor/monitor.rs::reap_dead_agents`), and the bare VM name a lease
    /// records as its `provider_resource_id`.
    fn keys(reference: &str, vm_name: &str) -> Vec<String> {
        vec![
            reference.to_string(),
            format!("local@{vm_name}"),
            vm_name.to_string(),
        ]
    }

    pub(super) fn holders_for(&self, reference: &str, vm_name: &str) -> Vec<String> {
        let mut found: Vec<String> = Self::keys(reference, vm_name)
            .iter()
            .filter_map(|key| self.reasons.get(key))
            .flatten()
            .cloned()
            .collect();
        found.sort();
        found.dedup();
        found
    }

    pub(super) fn gpu_type_for(&self, reference: &str, vm_name: &str) -> Option<String> {
        Self::keys(reference, vm_name)
            .iter()
            .find_map(|key| self.gpu_types.get(key))
            .cloned()
    }
}

/// `provider-leases/{job_id}.json` -> `job_id`.
fn lease_job_id(path: &str) -> Option<String> {
    let job_id = path
        .strip_prefix(LEASE_PREFIX)?
        .strip_suffix(LEASE_SUFFIX)?;
    (!job_id.is_empty()).then(|| job_id.to_string())
}

/// Whether a lease still holds its provider resource: not released, and its
/// resource TTL has not lapsed.
///
/// An absent or unparseable `resource_expires_at` counts as HELD. The reaper
/// must never delete a VM on the strength of a timestamp it could not read.
fn lease_holds_resource(lease: &ProviderLease) -> bool {
    if lease.state == LeaseState::Released.as_str() {
        return false;
    }
    // Python `datetime.fromisoformat(value.replace("Z", "+00:00"))`, the
    // same normalization `queue/leases.rs::parse_timestamp` applies.
    match DateTime::parse_from_rfc3339(&lease.resource_expires_at.replace('Z', "+00:00")) {
        Ok(expires) => Utc::now() < expires.with_timezone(&Utc),
        Err(_) => true,
    }
}
