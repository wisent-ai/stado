//! A job on owned hardware priced from the energy its board drew in the hours
//! it ran, as the host's beacon measured them (`hours`), at the host's
//! declared tariff (`targets[].power_tariff`).
//!
//! The cost report is in US dollars, so only a tariff stated in USD prices a
//! job here; a tariff in another currency leaves the job unpriced with that
//! cause, because no exchange rate is declared anywhere Stado reads, and a
//! rate written here would be nobody's statement.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, DurationRound, Utc};
use serde_json::Value;

use crate::models::Job;
use crate::queue::JobStorage;

/// Joules in one kilowatt-hour: 1000 W for 3600 s (SI).
const JOULES_PER_KWH: f64 = 3_600_000.0;

/// Per host: its USD price per kWh (or why it has none) and, per board and
/// hour start, the energy the board drew in that hour; and every identity
/// the host is known by (its registry name, its hostnames, the slugs its
/// beacon is stored under), since a job names its host by the worker's
/// hostname (`local@<hostname>`) and an offer by its consumer id.
pub struct Energy {
    hosts: BTreeMap<String, Host>,
    names: BTreeMap<String, String>,
}

struct Host {
    price_per_kwh: Result<f64, String>,
    /// board UUID -> hour start -> kWh drawn in that hour.
    boards: BTreeMap<String, BTreeMap<DateTime<Utc>, f64>>,
}

/// A job's cost in USD, or why it is unpriced.
pub type Priced = Result<f64, String>;

impl Energy {
    /// Every registered host's tariff and the hours its newest beacon carries.
    /// A registry or beacon store that cannot be read answers that cause, so
    /// no owned job is priced at a guess.
    pub async fn read(store: &JobStorage) -> Result<Self, String> {
        let document = crate::cli::registry::fetch_document()
            .await
            .map_err(|error| format!("the registry cannot be read: {error}"))?;
        let registry = crate::targets::load_registry_from_value(&document)
            .map_err(|error| format!("the registry does not load: {error}"))?;
        let beacons = crate::cli::registry::beacon_bodies(store)
            .await
            .map_err(|error| format!("the host beacons cannot be read: {error}"))?;
        let mut hosts = BTreeMap::new();
        let mut names = BTreeMap::new();
        for target in &registry.targets {
            // The report is in US dollars: a tariff in another currency is
            // converted at the rate the registry declares for it, and one
            // with no declared rate is said, never priced at a guess.
            let price_per_kwh = match target.power_tariff() {
                Some(tariff) => tariff
                    .usd_per_kwh()
                    .map_err(|cause| format!("{}: {cause}", target.name)),
                None => Err(format!("{} declares no power_tariff", target.name)),
            };
            let slugs = crate::monitor::host_health::beacon_slugs(target, &target.name);
            let hours = slugs
                .iter()
                .find_map(|slug| beacons.get(slug))
                .and_then(|beacon| beacon.get("hours"))
                .and_then(Value::as_object);
            let mut boards: BTreeMap<String, BTreeMap<DateTime<Utc>, f64>> = BTreeMap::new();
            for (start, hour) in hours.into_iter().flatten() {
                let Ok(start) = DateTime::parse_from_rfc3339(start) else {
                    continue;
                };
                for (uuid, board) in hour["boards"].as_object().into_iter().flatten() {
                    let kwh = board["watt_seconds"].as_f64().unwrap_or(0.0) / JOULES_PER_KWH;
                    boards
                        .entry(uuid.clone())
                        .or_default()
                        .insert(start.with_timezone(&Utc), kwh);
                }
            }
            for slug in &slugs {
                names.insert(slug.clone(), target.name.clone());
            }
            hosts.insert(
                target.name.clone(),
                Host {
                    price_per_kwh,
                    boards,
                },
            );
        }
        Ok(Self { hosts, names })
    }

    /// The host `identity` names, as the registry knows it.
    fn host(&self, identity: &str) -> Result<(&str, &Host), String> {
        let normalized = crate::targets::normalize_hostname(identity);
        let name = self
            .names
            .get(&normalized)
            .or_else(|| {
                normalized
                    .split_once('.')
                    .and_then(|(label, _)| self.names.get(label))
            })
            .ok_or_else(|| format!("{identity} names no host the registry holds"))?;
        let host = self
            .hosts
            .get(name)
            .ok_or_else(|| format!("{identity} names no host the registry holds"))?;
        Ok((name.as_str(), host))
    }

    /// What `job` cost on its board: each hour it ran contributes the board's
    /// energy in that hour, split among the jobs of `sharing` that ran on the
    /// same host and board by the share of the hour each covered, at the
    /// host's USD tariff. Unpriced, with the cause, when the job names no host
    /// or board, the host has no USD tariff, or any hour it ran was not
    /// measured on that board: one missing hour would make the sum short.
    pub fn price(&self, job: &Job, sharing: &[Job]) -> Priced {
        let host = job
            .instance_ref
            .as_deref()
            .and_then(|reference| reference.strip_prefix(crate::machine::AGENT_INSTANCE_PREFIX))
            .ok_or_else(|| format!("job {} names no local host", job.job_id))?;
        let board = job
            .gpu_uuid
            .as_deref()
            .ok_or_else(|| format!("job {} records no board it ran on", job.job_id))?;
        let (host, known) = self.host(host)?;
        let price = known.price_per_kwh.clone()?;
        let hours = known
            .boards
            .get(board)
            .ok_or_else(|| format!("{host}'s beacon measured no hour of board {board}"))?;
        let (started, ended) = span(job)?;
        // The other jobs on the same host and board whose run is known: an
        // hour's energy is the board's, and every job on it took a share.
        let neighbours: Vec<(DateTime<Utc>, DateTime<Utc>)> = sharing
            .iter()
            .filter(|other| other.job_id != job.job_id)
            .filter(|other| {
                other.instance_ref == job.instance_ref && other.gpu_uuid == job.gpu_uuid
            })
            .filter_map(|other| span(other).ok())
            .collect();
        let mut cost = 0.0;
        let mut hour = started
            .duration_trunc(Duration::hours(1))
            .map_err(|error| format!("{started} cannot be cut to its hour: {error}"))?;
        while hour < ended {
            let next = hour + Duration::hours(1);
            let kwh = hours.get(&hour).ok_or_else(|| {
                format!("{host}'s beacon holds no measurement of board {board} for the hour {hour}")
            })?;
            let covered = |from: DateTime<Utc>, to: DateTime<Utc>| {
                (to.min(next) - from.max(hour)).num_milliseconds().max(0) as f64
            };
            let own = covered(started, ended);
            let all = own
                + neighbours
                    .iter()
                    .map(|(from, to)| covered(*from, *to))
                    .sum::<f64>();
            if all > 0.0 {
                cost += kwh * (own / all) * price;
            }
            hour = next;
        }
        Ok(cost)
    }

    /// What one hour of `host` costs, for a placement that is about to use
    /// it: the mean energy per hour of its busiest board over the hours its
    /// beacon measured, at its USD tariff. The busiest board, because a job
    /// placed on the host will keep a board busy; the mean of measured hours
    /// only, because an hour not measured is not an hour that cost nothing.
    pub fn hourly(&self, identity: &str) -> Priced {
        let (host, known) = self.host(identity)?;
        let price = known.price_per_kwh.clone()?;
        known
            .boards
            .values()
            .filter(|hours| !hours.is_empty())
            .map(|hours| hours.values().sum::<f64>() / hours.len() as f64)
            .max_by(f64::total_cmp)
            .map(|kwh| kwh * price)
            .ok_or_else(|| format!("{host}'s beacon measured no hour of any board"))
    }
}

/// When the job started and ended, as its record states them.
fn span(job: &Job) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let parse = |what: &str, value: Option<&str>| {
        let value = value.ok_or_else(|| format!("job {} records no {what}", job.job_id))?;
        DateTime::parse_from_rfc3339(value)
            .map(|at| at.with_timezone(&Utc))
            .map_err(|error| {
                format!(
                    "job {}'s {what} {value:?} is not a time: {error}",
                    job.job_id
                )
            })
    };
    let started = parse("start", job.started_at.as_deref())?;
    let ended = parse(
        "end",
        job.completed_at.as_deref().or(job.failed_at.as_deref()),
    )?;
    Ok((started, ended))
}
