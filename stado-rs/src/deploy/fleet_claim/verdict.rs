//! What the three sources add up to: whether anything can claim, and the one
//! headline and one `--json` section that say so.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::queue::capacity::{self, Publication};

use super::{wait_words, HostVerdict, OldestWait};

/// Whether anything in this fleet can claim anything in this queue, and when
/// not, why not, host by host.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetClaim {
    /// How many jobs sit in `queue/`.
    pub queued: usize,
    /// The longest-waiting queued job, or `None` for an empty queue.
    pub oldest: Option<OldestWait>,
    /// Every `kind=local` registry target, in registry order.
    pub hosts: Vec<HostVerdict>,
    /// Registry names of the local hosts whose newest capacity publication is
    /// inside the staleness horizon. The number `stado overview` prints where
    /// it used to print a count of declared workers.
    pub publishing: Vec<String>,
    /// Consumer ids publishing capacity that no declared local target
    /// claims: cloud dispatchers, marketplace workers, and hosts the
    /// registry has not adopted. A fresh one of these is why this report
    /// refuses to say nothing can claim — it cannot see what that publisher
    /// is able to take, and asserting a stall it cannot prove is worse than
    /// saying less.
    pub unattributed: Vec<String>,
    /// consumer_id -> publication for every row in the store, stale rows
    /// included. Retained so a caller that also needs the live capacity
    /// rows — `stado overview` needs them for its worker list — reads the
    /// prefix once, through this reader, and can never disagree with the
    /// verdict about which rows are fresh.
    pub(super) publications: BTreeMap<String, Publication>,
    pub(super) now: DateTime<Utc>,
}

impl FleetClaim {
    /// {consumer_id: payload} for every publication inside the staleness
    /// horizon — what [`capacity::read_consumer_capacity`] would return, with
    /// nothing deleted on the way.
    pub fn live_consumers(&self) -> BTreeMap<String, Value> {
        self.publications
            .iter()
            .filter(|(_, row)| !row.stale(self.now))
            .map(|(id, row)| (id.clone(), row.payload.clone()))
            .collect()
    }

    /// At least one host could claim at least one queued job.
    ///
    /// True for an empty fleet-wide unknown: a fresh publisher this report
    /// cannot attribute to a declared host counts as a claimant, because
    /// "nothing can claim" is a strong statement and this reader must only
    /// make it when it can see every publisher that exists.
    pub fn claimable(&self) -> bool {
        self.hosts.iter().any(|host| host.claiming)
            || self.unattributed.iter().any(|id| {
                self.publications
                    .get(id)
                    .is_some_and(|row| !row.stale(self.now))
            })
    }

    /// Work is queued and nothing in the fleet can take it. The one condition
    /// worth interrupting an operator over.
    pub fn stuck(&self) -> bool {
        self.queued > 0 && !self.claimable()
    }

    /// The verdict as an operator reads it: a headline that sizes the stall,
    /// then one line per host that cannot claim, in the host's own words.
    ///
    /// Empty unless [`Self::stuck`]. One renderer for both surfaces, so
    /// `stado overview` and `stado status` can never print two different
    /// explanations of one stuck queue.
    pub fn lines(&self) -> Vec<String> {
        if !self.stuck() {
            return Vec::new();
        }
        let mut lines = vec![format!(
            "nothing can claim the queue: {} queued, {}; {} of {} local hosts publish capacity newer than {}s",
            self.queued,
            self.oldest_words(),
            self.publishing.len(),
            self.hosts.len(),
            capacity::CAPACITY_STALE_SECONDS,
        )];
        if self.hosts.is_empty() {
            lines.push("  cannot claim: the registry declares no kind=local host".to_string());
        }
        for host in &self.hosts {
            if host.claiming {
                continue;
            }
            let words: Vec<String> = host
                .blockers
                .iter()
                .map(|blocker| blocker.rendered())
                .collect();
            lines.push(format!(
                "  cannot claim: {} {}",
                host.host,
                words.join(", ")
            ));
        }
        lines
    }

    /// `oldest 2c4a47aa waiting 121h 38m`, or the empty-queue phrasing.
    fn oldest_words(&self) -> String {
        match &self.oldest {
            None => "nothing waiting".to_string(),
            Some(job) => match job.age_seconds {
                None => format!("oldest {} waiting an unreadable time", job.job_id),
                Some(age) => format!("oldest {} waiting {}", job.job_id, wait_words(age)),
            },
        }
    }

    /// The `--json` section.
    pub fn to_report(&self) -> Value {
        json!({
            "claimable": self.claimable(),
            "stuck": self.stuck(),
            "queued": self.queued,
            "oldest_queued": self.oldest.as_ref().map(|job| json!({
                "job_id": job.job_id,
                "age_seconds": job.age_seconds,
                "waited": job.age_seconds.map(wait_words),
            })),
            "stale_horizon_seconds": capacity::CAPACITY_STALE_SECONDS,
            "publishing": self.publishing,
            "unattributed_publishers": self.unattributed,
            "hosts": self.hosts.iter().map(|host| json!({
                "host": host.host,
                "claiming": host.claiming,
                "blockers": host.blockers.iter().map(|blocker| json!({
                    "word": blocker.word,
                    "detail": blocker.detail,
                })).collect::<Vec<Value>>(),
            })).collect::<Vec<Value>>(),
        })
    }
}
