//! What every placement profile needs, decided from the registry and the
//! hosts' own memory publications and nothing else. Pure: no store, no host,
//! no clock but the one passed in, so `stado placement relief` and the
//! autonomy tick read the same plan from the same facts.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{words, HostMemory, ReliefRow, PRESSURE_STICKY_SECONDS, RELOCATION_COOLDOWN_SECONDS};
use crate::placement::{self, PlacementProfile};
use crate::targets::Registry;

/// One host other than the placed one, and why it was or was not chosen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub host: String,
    /// The profile declares this host; a move may go there now. An
    /// undeclared host is a place `placement standby` can prepare.
    pub declared: bool,
    /// `eligible`, or the one reason this host was refused.
    pub verdict: String,
    pub memory: Option<HostMemory>,
}

/// The candidate verdicts. Named once beside the classification words.
pub mod verdicts {
    pub const ELIGIBLE: &str = "eligible";
    pub const NO_PUBLICATION: &str = "no_publication";
    pub const STALE: &str = "stale";
    pub const PRESSURED: &str = "pressured";
    pub const SWAP_OVER: &str = "swap_over";
    pub const UNMEASURED: &str = "unmeasured";
    pub const NO_MORE_HEADROOM: &str = "no_more_headroom_than_source";
}

/// What the pass should attempt for a profile, beside the row that argues it.
#[derive(Debug, Clone)]
pub enum DueAction {
    /// Move the profile to a declared host with more headroom.
    Move,
    /// No declared host has headroom; prepare this registered host for the
    /// profile, so a later tick can move there.
    Standby,
}

#[derive(Debug, Clone)]
pub struct Due {
    pub profile: PlacementProfile,
    pub to_host: String,
    pub action: DueAction,
}

#[derive(Debug, Clone)]
pub struct ReliefOutcome {
    pub row: ReliefRow,
    pub due: Option<Due>,
}

/// Every profile's row. `relocations` is the previous report's map of last
/// relocation instants, for the cooldown; `pressure_seen` is its map of when
/// each host last published memory pressure, for the sticky window.
pub fn plan(
    document: &Value,
    registry: &Registry,
    hosts: &BTreeMap<String, HostMemory>,
    relocations: &BTreeMap<String, String>,
    pressure_seen: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Result<Vec<ReliefOutcome>, String> {
    let profiles = placement::profiles(document)?;
    Ok(profiles
        .into_iter()
        .map(|profile| plan_profile(profile, registry, hosts, relocations, pressure_seen, now))
        .collect())
}

/// Whether this host counts as pressured right now: its own publication says
/// so, or it published pressure inside [`PRESSURE_STICKY_SECONDS`].
fn pressured(
    host: &str,
    memory: &HostMemory,
    pressure_seen: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> bool {
    if memory.pressure_active {
        return true;
    }
    seen_within_window(host, pressure_seen, now).is_some()
}

/// When this host last published pressure, if that was inside the window.
fn seen_within_window(
    host: &str,
    pressure_seen: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Option<String> {
    let last = pressure_seen.get(host)?;
    let seen = DateTime::parse_from_rfc3339(last).ok()?;
    let age = now
        .signed_duration_since(seen.with_timezone(&Utc))
        .num_seconds();
    (age >= i64::default() && age < PRESSURE_STICKY_SECONDS).then(|| last.clone())
}

fn plan_profile(
    profile: PlacementProfile,
    registry: &Registry,
    hosts: &BTreeMap<String, HostMemory>,
    relocations: &BTreeMap<String, String>,
    pressure_seen: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> ReliefOutcome {
    let mut row = ReliefRow {
        profile: profile.name.clone(),
        services: profile.services.clone(),
        placed_on: None,
        classification: String::new(),
        destination: None,
        candidates: Vec::new(),
        detail: String::new(),
        transaction_id: None,
    };
    let settle = |mut row: ReliefRow, classification: &str, detail: String| {
        row.classification = classification.to_string();
        row.detail = detail;
        ReliefOutcome { row, due: None }
    };
    if let Err(refusal) = crate::cli::placement::ensure_profile_lifecycle_mutable(&profile) {
        return settle(row, words::PROFILE_UNMOVABLE, refusal.to_string());
    }
    let placed_on = match crate::cli::placement::placed_host(registry, &profile) {
        Ok(host) => host,
        Err(refusal) => return settle(row, words::PROFILE_UNMOVABLE, refusal),
    };
    row.placed_on = Some(placed_on.clone());
    let Some(source) = hosts.get(&placed_on).filter(|memory| !memory.stale) else {
        let detail = match hosts.get(&placed_on) {
            Some(memory) => format!("{placed_on} last published memory {}", memory.describe()),
            None => format!("{placed_on} has published no capacity"),
        };
        return settle(row, words::EVIDENCE_STALE, detail);
    };
    if !pressured(&placed_on, source, pressure_seen, now) {
        return settle(
            row,
            words::SETTLED,
            format!("{placed_on}: {}", source.describe()),
        );
    }
    let declared = |host: &String| profile.hosts.contains_key(host);
    row.candidates = registry
        .targets
        .iter()
        .filter(|target| target.kind == "local" && target.name != placed_on)
        .map(|target| {
            candidate(
                &target.name,
                declared(&target.name),
                hosts.get(&target.name),
                source,
                seen_within_window(&target.name, pressure_seen, now).is_some(),
            )
        })
        .collect();
    let best = |wanted_declared: bool| {
        row.candidates
            .iter()
            .filter(|candidate| {
                candidate.declared == wanted_declared && candidate.verdict == verdicts::ELIGIBLE
            })
            .max_by(|a, b| {
                let headroom = |candidate: &Candidate| {
                    candidate
                        .memory
                        .as_ref()
                        .and_then(|memory| memory.available_gb)
                        .unwrap_or_default()
                };
                headroom(a).total_cmp(&headroom(b))
            })
            .map(|candidate| candidate.host.clone())
    };
    let Some(to_host) = best(true) else {
        if let Some(standby) = best(false) {
            row.destination = Some(standby.clone());
            row.detail = format!(
                "{placed_on} is over its watermark ({}) and no declared host has more headroom; \
                 {standby} is registered with more and can be prepared to stand by",
                source.describe()
            );
            return ReliefOutcome {
                row,
                due: Some(Due {
                    profile,
                    to_host: standby,
                    action: DueAction::Standby,
                }),
            };
        }
        return settle(
            row,
            words::NO_DESTINATION,
            format!(
                "{placed_on} is over its watermark ({}) and no other host, declared or \
                 registered, has more headroom",
                source.describe()
            ),
        );
    };
    if let Some(last) = relocations.get(&profile.name) {
        let recent = DateTime::parse_from_rfc3339(last)
            .ok()
            .map(|last| {
                now.signed_duration_since(last.with_timezone(&Utc))
                    .num_seconds()
            })
            .is_some_and(|age| age >= i64::default() && age < RELOCATION_COOLDOWN_SECONDS);
        if recent {
            row.destination = Some(to_host);
            return settle(
                row,
                words::MOVED_RECENTLY,
                format!(
                    "{placed_on} is over its watermark ({}), but {} was relocated at {last}, within the {RELOCATION_COOLDOWN_SECONDS}s cooldown",
                    source.describe(),
                    profile.name
                ),
            );
        }
    }
    row.destination = Some(to_host.clone());
    row.detail = format!(
        "{placed_on} is over its watermark ({}); {to_host} has more headroom",
        source.describe()
    );
    ReliefOutcome {
        row,
        due: Some(Due {
            profile,
            to_host,
            action: DueAction::Move,
        }),
    }
}

fn candidate(
    host: &str,
    declared: bool,
    memory: Option<&HostMemory>,
    source: &HostMemory,
    pressured_recently: bool,
) -> Candidate {
    let verdict = match memory {
        None => verdicts::NO_PUBLICATION,
        Some(memory) if memory.stale => verdicts::STALE,
        // A destination is judged over the same window as a source: a host
        // that published pressure minutes ago and reads clear this second is
        // not headroom, it is the same oscillation that hid the source's own
        // pressure from this stage.
        Some(memory) if memory.pressure_active || pressured_recently => verdicts::PRESSURED,
        Some(memory) if memory.swap_pressure_only => verdicts::SWAP_OVER,
        Some(memory) => match (memory.available_gb, source.available_gb) {
            (None, _) => verdicts::UNMEASURED,
            (Some(candidate), Some(placed)) if candidate <= placed => verdicts::NO_MORE_HEADROOM,
            _ => verdicts::ELIGIBLE,
        },
    };
    Candidate {
        host: host.to_string(),
        declared,
        verdict: verdict.to_string(),
        memory: memory.cloned(),
    }
}
