//! Which live fleet host a release build or delivery job is pinned to.

use std::collections::BTreeMap;

use crate::cli::release_submit::builds::claimability::{claimability, Claimability};
use crate::cli::release_submit::builds::scratch::{published_free_bytes, scratch_verdict};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::ScratchReceipt;

/// The registry and every live capacity publication, read once for all the
/// platforms one submission places. Each placement used to read both again:
/// the capacity read alone is a listing plus one download per publishing
/// host, repeated for every platform of every build.
pub(crate) struct Fleet {
    registry: crate::targets::Registry,
    capacity: BTreeMap<String, serde_json::Value>,
}

impl Fleet {
    pub(crate) async fn read() -> Result<Self, CmdError> {
        let registry = crate::targets::fetch_registry_remote()
            .await
            .map_err(CmdError::from)?;
        let store = JobStorage::new().await.map_err(CmdError::from)?;
        let capacity = crate::queue::capacity::read_consumer_capacity(&store)
            .await
            .map_err(CmdError::from)?;
        Ok(Self { registry, capacity })
    }
}

/// Pin one job to a live host of `platform`.
///
/// `scratch` is what the last build of the product being placed wrote to
/// disk, when a measuring builder has recorded it: a host that publishes less
/// free disk than that above its own low watermark is refused with
/// [`crate::deploy::host_gates::RELEASE_SCRATCH_SHORT`]. Delivery jobs pass
/// `None`; they write an archive, not a build tree.
///
/// Among the hosts that may take the job, the one publishing the most free
/// disk goes first. Name order would send every build of a platform to the
/// first host in the list for as long as that host stays one byte above its
/// low watermark.
///
/// `secret_env` is what the job projects into its environment, as
/// `item#field` references. A host whose publication lists
/// `secret_fields` without one of them cannot resolve it and is unfit.
///
/// `in_flight` counts the builds of the same product each builder is running
/// now (see [`super::history`]). They share one Cargo build directory there,
/// and Cargo admits one at a time, so among hosts otherwise alike the one
/// compiling fewer of them goes first. Delivery jobs pass an empty map.
pub(crate) async fn builder(
    fleet: &Fleet,
    platform: &str,
    pinned: Option<&str>,
    scratch: Option<&ScratchReceipt>,
    secret_env: &BTreeMap<String, String>,
    in_flight: &BTreeMap<String, usize>,
) -> Result<(crate::targets::ComputeTarget, String), CmdError> {
    let (registry, capacity) = (&fleet.registry, &fleet.capacity);
    // Keep each live consumer's own publication, not merely its name: the
    // claimability judgement below is made from it, so no extra host read is
    // needed to know whether a candidate can take the work it would be pinned.
    let mut live_consumers = BTreeMap::new();
    for (consumer, publication) in capacity {
        let identity = consumer.strip_prefix("local-").unwrap_or(consumer);
        if let Some(target) = registry.lookup_self(identity).map_err(CmdError::from)? {
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
        .iter()
        .filter_map(|target| {
            if target.release_platform != platform || pinned.is_some_and(|name| target.name != name)
            {
                return None;
            }
            let (consumer, publication) = live_consumers.get(&target.name)?;
            // An operator's workstation - role `interactive` in the registry -
            // may build, but only after every always-on or burst builder of
            // the platform that can take the job, because a workstation can
            // publish the most free disk while somebody is using it; the claim
            // gate's cpu_busy and ram_headroom_low still hold it back while
            // it is loaded.
            let interactive = target.role.as_deref() == Some("interactive");
            let mut verdict = claimability(publication);
            if let Some(short) = scratch.and_then(|need| scratch_verdict(publication, need)) {
                verdict = match verdict {
                    Claimability::Refusing { mut blockers } => {
                        blockers.push(short);
                        Claimability::Refusing { blockers }
                    }
                    _ => Claimability::Unfit { reason: short },
                };
            }
            if let Some(reason) = missing_secret_field(publication, secret_env) {
                verdict = Claimability::Unfit { reason };
            }
            considered.push((target.name.clone(), verdict.clone()));
            // Busy workers can receive queued builds; their normal claim gate
            // still waits for resources. `cleanup_in_progress` is the same
            // kind of wait: on a volume past the disk-full threshold the
            // janitor takes its turn between workloads, and the next claim
            // follows that turn. Refusing it here left the only linux-amd64
            // builder unplaceable for every product for as long as its root
            // volume held the operator's own data past the threshold, while
            // its work volume had terabytes free. Disk shortage, policy,
            // missing measurements and unexplained refusals are still never
            // treated as a busy queue.
            let waiting_for_resources = match &verdict {
                Claimability::Refusing { blockers }
                    if !blockers.is_empty()
                        && blockers.iter().all(|reason| {
                            matches!(
                                reason.as_str(),
                                "cpu_busy"
                                    | "ram_headroom_low"
                                    | "exclusive_job_running"
                                    | crate::providers::local::disk_cleanup::CLEANUP_IN_PROGRESS
                            )
                        }) =>
                {
                    true
                }
                verdict if verdict.eligible() => false,
                _ => return None,
            };
            let free = published_free_bytes(publication).unwrap_or_default();
            let sharing = in_flight.get(&target.name).copied().unwrap_or_default();
            Some((
                waiting_for_resources,
                sharing,
                interactive,
                free,
                target,
                consumer.clone(),
            ))
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| right.3.cmp(&left.3))
            .then_with(|| left.4.name.cmp(&right.4.name))
    });
    candidates
        .into_iter()
        .next()
        .map(|(_, _, _, _, target, consumer)| (target.clone(), consumer))
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
                store
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
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })
}

/// The first `role#field` the job declares that this host's published
/// `secret_fields` does not allow. A publication without the list comes from
/// an agent older than it and is not judged here; that agent's own
/// claim-time probe still declines what it cannot resolve.
fn missing_secret_field(
    publication: &serde_json::Value,
    secret_env: &BTreeMap<String, String>,
) -> Option<String> {
    let allowed = publication.get("secret_fields")?.as_array()?;
    secret_env
        .values()
        .find(|reference| {
            !allowed
                .iter()
                .any(|field| field.as_str() == Some(reference.as_str()))
        })
        .map(|reference| format!("{reference} is not in its agent.skarbiec.secret_fields"))
}

/// Refuse a platform job that asks for a role no item the `stado` consumer
/// can see plays, before anything is queued.
///
/// [`missing_secret_field`] judges a builder by the `secret_fields` it
/// declares, and a declaration is not a vault: a job whose role no item
/// plays would sit queued while every builder's claim-time probe declines
/// it. The builder resolves a job's secrets as consumer `stado`, so that
/// consumer's metadata listing — ids and tags, no values — decides whether
/// any host could ever claim the job.
pub(crate) async fn refuse_unheld_secret_items(
    product: &str,
    platform: &str,
    secret_env: &BTreeMap<String, String>,
) -> Result<(), CmdError> {
    if secret_env.is_empty() {
        return Ok(());
    }
    let listing_failed = |error: crate::skarbiec::SkarbiecError| {
        CmdError::click(format!(
            "{product}'s {platform} build asks for vault roles and the stado consumer's item \
             listing could not be read, so whether any builder can claim it is unknown: {error}"
        ))
        .stating(error.failure_code())
    };
    let visible = crate::skarbiec::Client::configured()
        .map_err(listing_failed)?
        .list_items()
        .await
        .map_err(listing_failed)?;
    let unheld: Vec<String> = secret_env
        .iter()
        .filter_map(|(env, reference)| {
            let role = reference
                .split_once('#')
                .map_or(reference.as_str(), |(role, _)| role);
            crate::skarbiec::roles::item_for_role(&visible, role)
                .err()
                .map(|refusal| format!("{env}: {refusal}"))
        })
        .collect();
    if unheld.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{product}'s {platform} build asks for roles the stado consumer cannot select, so no \
         builder could ever claim it: {}. Store the secret with `stado credentials item put \
         --host <vault owner> --role <ROLE>` and grant it to stado before building.",
        unheld.join("; ")
    )))
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
        .map_err(CmdError::from)?;
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    let publications = crate::queue::capacity::read_publications(&store)
        .await
        .map_err(CmdError::from)?;
    let mut newest = None;
    for (consumer, publication) in publications {
        let identity = consumer.strip_prefix("local-").unwrap_or(&consumer);
        let matches_target = registry
            .lookup_self(identity)
            .map_err(CmdError::from)?
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
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })
}
