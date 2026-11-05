//! A job on owned hardware priced from the energy its board drew above its
//! idle draw in the hours it ran, as the host's beacon measured them
//! (`hours`), at the host's declared tariff (`targets[].power_tariff`).
//!
//! The cost report is in US dollars: a tariff in another currency is
//! converted at the `usd_rate` it declares ([`PowerTariff::usd_per_kwh`]),
//! and one with no declared rate leaves the job unpriced with that cause,
//! because a rate written here would be nobody's statement.
//!
//! [`PowerTariff::usd_per_kwh`]: crate::targets::PowerTariff::usd_per_kwh

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, DurationRound, Utc};
use serde_json::Value;

use crate::models::Job;
use crate::queue::JobStorage;

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
    /// The host's tariff, or why it has none; each hour is priced at the
    /// band covering it, in US dollars.
    tariff: Result<crate::targets::PowerTariff, String>,
    /// board UUID -> hour start -> kWh drawn above the board's idle draw in
    /// that hour, or why it cannot be told.
    boards: BTreeMap<String, BTreeMap<DateTime<Utc>, Result<f64, String>>>,
}

impl Host {
    fn usd_per_kwh_at(&self, name: &str, at: DateTime<Utc>) -> Result<f64, String> {
        match &self.tariff {
            Ok(tariff) => tariff
                .usd_per_kwh_at(at)
                .map_err(|cause| format!("{name}: {cause}")),
            Err(cause) => Err(cause.clone()),
        }
    }
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
            let tariff = target
                .power_tariff()
                .ok_or_else(|| format!("{} declares no power_tariff", target.name));
            let slugs = crate::monitor::host_health::beacon_slugs(target, &target.name);
            let hours = slugs
                .iter()
                .find_map(|slug| beacons.get(slug))
                .and_then(|beacon| beacon.get("hours"))
                .and_then(Value::as_object);
            // A job is charged only what its board drew above its idle draw
            // over the beacon window, because the board draws that whether
            // or not anything runs.
            let idle = hours
                .map(crate::deploy::host_hours::idle_watts)
                .unwrap_or_default();
            let mut boards: BTreeMap<String, BTreeMap<DateTime<Utc>, Result<f64, String>>> =
                BTreeMap::new();
            for (start, hour) in hours.into_iter().flatten() {
                let Ok(start) = DateTime::parse_from_rfc3339(start) else {
                    continue;
                };
                for (uuid, board) in hour["boards"].as_object().into_iter().flatten() {
                    let above = crate::deploy::host_hours::kwh_above_idle(uuid, board, &idle)
                        .ok_or_else(|| {
                            format!(
                                "{}'s board {uuid} has no measured idle draw in its beacon, so \
                                 the energy work added on it in the hour {start} cannot be told \
                                 from the power it draws anyway",
                                target.name
                            )
                        });
                    boards
                        .entry(uuid.clone())
                        .or_default()
                        .insert(start.with_timezone(&Utc), above);
                }
            }
            for slug in &slugs {
                names.insert(slug.clone(), target.name.clone());
            }
            hosts.insert(target.name.clone(), Host { tariff, boards });
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

    /// What `job` cost on its board: each hour it ran contributes the energy
    /// the board drew above its idle draw in that hour, split among the jobs of `sharing` that ran on the
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
            let kwh = hours
                .get(&hour)
                .ok_or_else(|| {
                    format!(
                        "{host}'s beacon holds no measurement of board {board} for the hour {hour}"
                    )
                })?
                .clone()?;
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
                cost += kwh * (own / all) * known.usd_per_kwh_at(host, hour)?;
            }
            hour = next;
        }
        Ok(cost)
    }

    /// What one hour of `host` costs, for a placement that is about to use
    /// it now: [`Self::hourly_at`] for the current hour.
    pub fn hourly(&self, identity: &str) -> Priced {
        self.hourly_at(identity, Utc::now())
    }

    /// What the hour starting at `at` of `host` costs: the mean energy per
    /// hour its busiest board drew above its idle draw over the hours its
    /// beacon measured, at the tariff band covering `at`, in US dollars. The
    /// busiest board, because a job placed on the host will keep a board
    /// busy; the mean of measured hours only, because an hour not measured
    /// is not an hour that cost nothing. A board whose idle draw is
    /// unmeasured is left out, and the host is unpriced when every board is.
    pub fn hourly_at(&self, identity: &str, at: DateTime<Utc>) -> Priced {
        let (host, known) = self.host(identity)?;
        let price = known.usd_per_kwh_at(host, at)?;
        known
            .boards
            .values()
            .filter_map(|hours| {
                let above: Vec<f64> = hours.values().filter_map(|kwh| kwh.clone().ok()).collect();
                (!above.is_empty()).then(|| above.iter().sum::<f64>() / above.len() as f64)
            })
            .max_by(f64::total_cmp)
            .map(|kwh| kwh * price)
            .ok_or_else(|| {
                format!("{host}'s beacon measured no hour of any board whose idle draw is known")
            })
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
