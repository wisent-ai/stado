//! The submit-time gate that refuses a platform no live worker can claim.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use crate::monitor::host_health;
use crate::queue::capacity;
use crate::queue::storage::JobStorage;
use crate::targets::{
    platform_accepts_job, platform_job_os_arch, queue_name, ComputeTarget, Registry,
};

/// One read of the fleet's claim state: which registry targets are
/// broadcasting fresh capacity right now, and the age of every local
/// target's newest health beacon. Those are the two questions a submit-time
/// claimability check asks — "can anything claim this job" and "which
/// machines went quiet, and how long ago" — and they are the same data
/// `stado host gates` reads per host (capacity publications under
/// `capacity/`, beacons under `host_health/`), read fleet-wide once per
/// submit attempt rather than once per platform.
///
/// The check exists because a build job no live worker can claim used to be
/// indistinguishable from a build in progress: the job sat in the queue and
/// the run said `running` for as long as nobody went looking.
pub struct Claimability {
    /// Registry target names with a fresh capacity publication.
    live: BTreeSet<String>,
    /// Local target name -> age of its newest beacon in seconds (`None` =
    /// it has never beaconed).
    beacon_ages: BTreeMap<String, Option<i64>>,
}

impl Claimability {
    /// Snapshot the claim state of the queue store the jobs would be
    /// submitted to.
    pub async fn read(registry: &Registry, store: &JobStorage) -> Result<Self, String> {
        let now = Utc::now();
        let publications = capacity::read_publications(store)
            .await
            .map_err(|exc| format!("reading capacity publications: {exc}"))?;
        let mut live = BTreeSet::new();
        for (consumer, publication) in &publications {
            if publication.stale(now) {
                continue;
            }
            // A local agent publishes as `local-<hostname>`, and the
            // hostname is the machine's own word for itself, not its
            // registry name — resolved to a target through `lookup_self`,
            // the same join release_submit's builder selection makes.
            let identity = consumer.strip_prefix("local-").unwrap_or(consumer);
            if let Some(target) = registry
                .lookup_self(identity)
                .map_err(|exc| exc.to_string())?
            {
                live.insert(target.name.clone());
            }
        }
        let prefix = format!("{}/", host_health::HEALTH_PREFIX);
        let mut newest_beacons: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
        for blob in store
            .list_blobs_with_meta(&prefix)
            .await
            .map_err(|exc| format!("listing {prefix}: {exc}"))?
        {
            let Some(slug) = blob
                .name
                .strip_prefix(&prefix)
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            // The object mtime is the age authority, exactly as
            // `registry beacon-age` reads it: the body's `reported_at` is
            // stamped by the reporting host's own clock.
            let Some(updated) = blob.updated else {
                continue;
            };
            newest_beacons.insert(slug.to_string(), updated);
        }
        let mut beacon_ages = BTreeMap::new();
        for target in registry.local_targets() {
            let observed = host_health::beacon_slugs(target, &target.name)
                .into_iter()
                .find_map(|slug| newest_beacons.get(&slug));
            beacon_ages.insert(
                target.name.clone(),
                observed.map(|stamp| (now - *stamp).num_seconds().max(0)),
            );
        }
        Ok(Self { live, beacon_ages })
    }

    /// Why no live worker can claim a build job for `platform`, or `None`
    /// when at least one can. The match applies the same routing the
    /// claiming agent does (`platform_job_os_arch` at submit,
    /// `platform_accepts_job` at claim), so a platform this check calls
    /// claimable is one a worker accepts. A platform no registry host
    /// declares and a platform whose hosts all went quiet are different
    /// sentences, because they send the operator to different fixes.
    pub fn refusal(&self, registry: &Registry, platform: &str) -> Option<String> {
        let (platform_os, architecture) = platform_job_os_arch(platform)?;
        let candidates: Vec<&ComputeTarget> = registry
            .targets
            .iter()
            .filter(|target| {
                platform_accepts_job(&target.release_platform, platform_os, architecture)
            })
            .collect();
        let queue = queue_name();
        if candidates.is_empty() {
            return Some(format!(
                "no registry host declares {platform}, so no worker can claim \
                 the {queue} queue for it"
            ));
        }
        if candidates
            .iter()
            .any(|target| self.live.contains(&target.name))
        {
            return None;
        }
        let beacons = candidates
            .iter()
            .map(|target| match self.beacon_ages.get(&target.name) {
                Some(Some(age)) => format!(
                    "{} {} ago",
                    target.name,
                    crate::cli::registry::human_age(chrono::TimeDelta::seconds(*age))
                ),
                _ => format!("{} no beacon", target.name),
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "no live {platform} worker claims the {queue} queue (last beacons: {beacons})"
        ))
    }
}
