//! The join: the three sources read once each, and the wait that sizes the
//! stall derived from the queue listing they share.

use chrono::{DateTime, Utc};

use crate::deploy::{host_gates, DeployError};
use crate::models::Job;
use crate::queue::capacity::{self, Publication};
use crate::queue::JobStorage;
use crate::targets::Registry;

use super::blockers::host_blockers;
use super::{FleetClaim, HostVerdict, OldestWait};

/// Read the verdict.
///
/// One listing plus one body per capacity row, one queue listing, and one
/// beacon read per silent host that declares a queue agent. No ssh, so this
/// stays answerable while every host in the fleet is wedged — which is
/// exactly when it is asked.
pub async fn read_fleet_claim(
    store: &JobStorage,
    registry: &Registry,
    now: DateTime<Utc>,
) -> Result<FleetClaim, DeployError> {
    let publications = capacity::read_publications(store)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let queued = store
        .list_jobs("queue", 0)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;

    let mut hosts: Vec<HostVerdict> = Vec::new();
    let mut publishing: Vec<String> = Vec::new();
    let mut attributed: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        let mut mine: Option<&Publication> = None;
        for (consumer_id, row) in &publications {
            if host_gates::resolves_to(registry, target, consumer_id)? {
                attributed.push(consumer_id.clone());
                mine = Some(row);
                break;
            }
        }
        if mine.is_some_and(|row| !row.stale(now)) {
            publishing.push(target.name.clone());
        }
        let blockers = host_blockers(store, registry, target, mine, &queued, now).await?;
        hosts.push(HostVerdict {
            host: target.name.clone(),
            claiming: blockers.is_empty(),
            blockers,
        });
    }

    let unattributed = publications
        .keys()
        .filter(|id| !attributed.contains(id))
        .cloned()
        .collect();

    Ok(FleetClaim {
        queued: queued.len(),
        oldest: oldest_wait(&queued, now),
        hosts,
        publishing,
        unattributed,
        publications,
        now,
    })
}

/// The longest-waiting queued job. A job whose `created_at` will not parse
/// sorts last rather than out: it is still queued, and dropping it would
/// shrink the count the headline prints.
fn oldest_wait(queued: &[Job], now: DateTime<Utc>) -> Option<OldestWait> {
    queued
        .iter()
        .map(|job| OldestWait {
            job_id: job.job_id.clone(),
            age_seconds: DateTime::parse_from_rfc3339(&job.created_at)
                .ok()
                .map(|created| (now - created.with_timezone(&Utc)).num_seconds()),
        })
        .max_by_key(|wait| wait.age_seconds.unwrap_or(i64::MIN))
}
