//! Which live fleet host a release build or delivery job is pinned to.

use std::collections::BTreeMap;

use crate::cli::release_submit::builds::claimability::{claimability, Claimability};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

pub(crate) async fn builder(
    platform: &str,
    pinned: Option<&str>,
) -> Result<(crate::targets::ComputeTarget, String), CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let capacity = crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // Keep each live consumer's own publication, not merely its name: the
    // claimability judgement below is made from it, so no extra host read is
    // needed to know whether a candidate can take the work it would be pinned.
    let mut live_consumers = BTreeMap::new();
    for (consumer, publication) in &capacity {
        let identity = consumer.strip_prefix("local-").unwrap_or(consumer);
        if let Some(target) = registry
            .lookup_self(identity)
            .map_err(|error| CmdError::click(error.to_string()))?
        {
            live_consumers
                .entry(target.name.clone())
                .or_insert_with(|| (consumer.clone(), publication.clone()));
        }
    }
    let declared_for_platform = registry
        .targets
        .iter()
        .filter(|target| {
            target.release_platform == platform && pinned.is_none_or(|name| target.name == name)
        })
        .count();
    let mut considered: Vec<(String, Claimability)> = Vec::new();
    let mut candidates: Vec<_> = registry
        .targets
        .into_iter()
        .filter_map(|target| {
            if target.release_platform != platform || pinned.is_some_and(|name| target.name != name)
            {
                return None;
            }
            let (consumer, publication) = live_consumers.get(&target.name)?;
            let verdict = claimability(publication);
            considered.push((target.name.clone(), verdict.clone()));
            // Busy workers can receive queued builds; their normal claim gate
            // still waits for resources. Disk, policy, missing measurements,
            // and unexplained refusals must never be treated as a busy queue.
            let waiting_for_resources = match &verdict {
                Claimability::Refusing { blockers }
                    if !blockers.is_empty()
                        && blockers.iter().all(|reason| {
                            matches!(
                                reason.as_str(),
                                "cpu_busy" | "ram_headroom_low" | "exclusive_job_running"
                            )
                        }) =>
                {
                    true
                }
                verdict if verdict.eligible() => false,
                _ => return None,
            };
            Some((waiting_for_resources, target, consumer.clone()))
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    candidates
        .into_iter()
        .next()
        .map(|(_, target, consumer)| (target, consumer))
        .ok_or_else(|| {
            // Name the store this looked in. Builders are selected from capacity
            // publications, not from the registry's platform declaration, so a host
            // that declares the platform and publishes to a different store is
            // invisible here. This message blamed a builder that had been running
            // for seven hours, because the operator machine's queue store was a
            // private loopback resolver and the fleet publishes to a tailnet
            // address, both under namespace `probierz`.
            let store = crate::config::wc_stado_storage_url();
            let store = if store.is_empty() {
                "the configured queue store".to_string()
            } else {
                store.to_string()
            };
            // Every host that was considered and what its own publication said,
            // because "no builder is available" without a reason cost an hour of
            // nobody knowing why a pinned job never started.
            let verdicts = if considered.is_empty() {
                String::from("no declared target of that platform is publishing capacity")
            } else {
                considered
                    .iter()
                    .map(|(host, verdict)| format!("{host} {}", verdict.describe()))
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let selection = match pinned {
                Some(name) => format!("{platform} on saved builder {name}"),
                None => platform.to_owned(),
            };
            CmdError::click(format!(
                "no live fleet builder can CLAIM release_platform {selection}; capacity read \
             from {store} namespace {:?} listed {} live consumer(s) and the registry \
             declares {} target(s) for that platform. Considered: {verdicts}. A host that \
             publishes capacity but claims nothing cannot build: read \
             `stado host gates <host>` for the full verdict.",
                crate::config::wc_stado_storage_namespace(),
                live_consumers.len(),
                declared_for_platform,
            ))
        })
}

/// Resolve the consumer id last published by one exact registry target.
///
/// Delivery jobs already name their target. Requiring fresh general capacity
/// here would discard that placement when a long-running job or disk-pressure
/// gate ages its publication. New queue plans still use [`builder`] and require
/// fresh capacity without a policy or disk refusal.
pub(crate) async fn target_consumer(target_name: &str) -> Result<String, CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let publications = crate::queue::capacity::read_publications(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut newest = None;
    for (consumer, publication) in publications {
        let identity = consumer.strip_prefix("local-").unwrap_or(&consumer);
        let matches_target = registry
            .lookup_self(identity)
            .map_err(|error| CmdError::click(error.to_string()))?
            .is_some_and(|target| target.name == target_name);
        if !matches_target {
            continue;
        }
        let replace = newest
            .as_ref()
            .is_none_or(|(_, stamp)| publication.stamp > *stamp);
        if replace {
            newest = Some((consumer, publication.stamp));
        }
    }
    newest.map(|(consumer, _)| consumer).ok_or_else(|| {
        CmdError::click(format!(
            "recorded target {target_name} has no retained capacity publication, so its consumer \
             identity is unknown; see stado host gates {target_name}"
        ))
    })
}
