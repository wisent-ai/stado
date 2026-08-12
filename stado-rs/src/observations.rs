//! When did anyone last look? The age of a fact, kept in the data model
//! instead of in whichever command happened to check.
//!
//! The service directory declared `stado-object-api` active on a laptop. That
//! declaration was structurally valid for twelve days: `config validate`
//! passed, `registry validate` passed, `doctor` passed, and the schema had
//! nothing to say about it because nothing in the schema was wrong. The lid
//! was closed. Every consumer routed through a forward with no upstream, and
//! the worker on the always-on Mac refused 29,616 times to claim work whose
//! diagnostics it could not upload.
//!
//! The defect was not that the fleet believed something false. It is that the
//! model could not express the sentence "nobody has looked since". `active` was
//! stored as a declaration, declarations do not decay, and so a statement
//! entered twelve days earlier read exactly like a statement confirmed a second
//! ago. There was no field that could have been wrong, which is why no
//! validator could have caught it.
//!
//! So an observation is a separate kind of record from a declaration, and it
//! carries four things a declaration does not:
//!
//!   fact     what was being checked, named the same way by every checker, so
//!            two commands looking at one thing produce one row and not two
//!   vantage  who looked. Reachability has no fleet-wide answer -- a loopback
//!            endpoint is true from its own host and false from everywhere
//!            else -- so an observation without a vantage is not a smaller
//!            observation, it is an unusable one
//!   state    `observed`, `unreachable`, or `unverified`
//!   at       when. This is the field the outage needed and did not have
//!
//! Three states, never two. `unreachable` means someone looked and it was not
//! there. `unverified` means the look did not happen: host down, helper not
//! installed, channel refused. Collapsing those into one `false` is how a fleet
//! learns to ignore its own reports -- an uninstalled probe starts rendering as
//! an outage, operators learn that red does not mean red, and the one real
//! outage in the pile reads like the rest of the noise. Twelve days is how long
//! that takes.
//!
//! And `Never` is the fourth answer, the one that has to stay distinct from all
//! three: this fleet has no record of anyone ever checking this. Reading that
//! as `observed` is the original bug. Reading it as `unreachable` invents an
//! outage. It is neither; it is an admission, and the only honest rendering of
//! it is the word `never`.
//!
//! Freshness is therefore a property of the record and not of the reader. A
//! `Fresh` observation is one made inside the caller's TTL and may be acted on.
//! `Stale` still carries the observation -- the last thing anyone saw is worth
//! showing, clearly marked as history -- but it must never be treated as the
//! present. [`DEFAULT_TTL`] is one hour because a laptop lid closes in a
//! second and an hour is how long the fleet is willing to be wrong about it.
//!
//! Storage is `~/.stado/observations.json`, owner-only, written through a
//! temporary file in the same directory and a rename, the same discipline
//! `cli::directory::write_forward_marker` uses for forward markers: a reader
//! must never see half a file, and a reader here is a routing decision.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// How long an observation speaks for the present.
///
/// One hour. A closed lid, a killed process and a revoked forward all happen
/// in under a second, so no TTL makes a stored observation equal to a live
/// probe; this is the window the fleet accepts being wrong in, chosen so that
/// a routine sweep keeps every fact green and a fleet nobody is sweeping goes
/// visibly amber within the working hour rather than silently in twelve days.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// Something answered at the declared place, from the vantage that was asked.
pub const OBSERVED: &str = "observed";
/// Someone looked and nothing was there. This is a failure.
pub const UNREACHABLE: &str = "unreachable";
/// The look could not happen. Deliberately not [`UNREACHABLE`]: "I did not
/// look" and "I looked and it is gone" send an operator to two different
/// machines.
pub const UNVERIFIED: &str = "unverified";

/// One look, by one machine, at one fact, at one moment.
///
/// Every field is a `String` because this record crosses a file, a helper
/// script's stdout and two CLI surfaces, and each place it is narrowed to an
/// enum is a place an unrecognised state gets flattened into a known one. The
/// states this tree writes are [`OBSERVED`], [`UNREACHABLE`] and
/// [`UNVERIFIED`]; a state written by something newer is carried through
/// verbatim rather than rounded down to the nearest word we already know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// What was checked, in a form every checker spells identically.
    /// Services use [`service_fact`]; the `<kind>:<subject>` shape is the
    /// convention so two kinds of fact cannot collide in one namespace.
    pub fact: String,
    /// The host that did the looking, by registry target name. Not optional
    /// and not defaultable: an observation whose vantage is unknown cannot be
    /// compared against the next one, so it is not evidence of anything.
    pub vantage: String,
    /// [`OBSERVED`], [`UNREACHABLE`], [`UNVERIFIED`], or a word from a newer
    /// writer, passed through.
    pub state: String,
    /// Why, in the operating system's own words where there are any. The
    /// difference between "connection refused" and "timed out" is the
    /// difference between a dead process and a dead route.
    pub detail: String,
    /// RFC 3339, UTC. The field the outage needed.
    pub at: String,
}

impl Observation {
    /// Stamp a look taken right now.
    pub fn now(
        fact: impl Into<String>,
        vantage: impl Into<String>,
        state: &str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            fact: fact.into(),
            vantage: vantage.into(),
            state: state.to_string(),
            detail: detail.into(),
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        }
    }

    /// The moment of the look, or `None` when the stamp is unreadable.
    ///
    /// An unreadable stamp is not treated as absent anywhere in this module.
    /// A row that exists proves somebody looked; only its age is in doubt, and
    /// the safe reading of an unknown age is "old".
    fn moment(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.at)
            .ok()
            .map(|stamp| stamp.with_timezone(&Utc))
    }
}

/// How old the fleet's knowledge of one fact is.
///
/// `Stale` carries the observation rather than discarding it because the last
/// thing anyone saw is the most useful thing an operator can be told after
/// "this is out of date" -- but it is handed back in a variant that cannot be
/// mistaken for the present by a caller pattern-matching on it.
#[derive(Debug, Clone)]
pub enum Freshness {
    /// Observed inside the TTL. Safe to act on.
    Fresh(Observation),
    /// The most recent observation, older than the TTL. History, not state.
    Stale(Observation),
    /// No machine has ever recorded a look at this fact. Not a failure, and
    /// not a pass; the absence of evidence, which is what the twelve days were.
    Never,
}

/// The canonical fact name for "is this service reachable".
///
/// One spelling, shared by the writer and by every reader, because a fact
/// recorded under `service:brama@mini` and looked up as `brama` is a fact with
/// no reader -- the exact shape of failure this whole change is against.
pub fn service_fact(name: &str, host: &str) -> String {
    format!("service:{name}@{host}")
}

/// `~/.stado/observations.json`.
fn path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        io::Error::other("HOME is not set, so there is nowhere to keep observations")
    })?;
    Ok(home.join(".stado").join("observations.json"))
}

/// Every observation this machine has on file, oldest file state included.
///
/// A missing file, an unreadable one, a truncated one, and a row whose shape
/// is not an observation all yield nothing rather than an error. A machine
/// that has never observed anything must read as [`Freshness::Never`], and
/// `Never` is a legitimate answer, not a fault: making the absence of the file
/// fail would mean every fresh host reported an error instead of the truth,
/// and the first thing an operator does with an error on a read path is stop
/// reading it.
///
/// Rows are decoded one at a time so a single malformed entry -- an older
/// writer, a hand edit -- costs only itself. The surviving rows are still
/// evidence, and dropping all of them to punish one is how a fleet loses the
/// record it is about to need.
pub fn load() -> Vec<Observation> {
    let Ok(path) = path() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&body) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|row| serde_json::from_value::<Observation>(row).ok())
        .collect()
}

/// Merge `observations` into the file: one row per `(fact, vantage)`, newest
/// kept.
///
/// Merging rather than appending is what keeps this a record of the present
/// instead of a log. A sweep runs on a timer; an append-only file would grow
/// without bound and force every reader to scan it to answer one question,
/// which is a reader that eventually stops being written. One row per pair is
/// the smallest thing that still answers "what does each vantage currently
/// say", and two vantages disagreeing about one fact is information, so the
/// vantage is part of the key and not a field that overwrites.
///
/// Newest wins by timestamp, not by arrival: a delayed sweep result must not
/// overwrite a fresher one just because it landed second. A row already on
/// file with an unreadable stamp loses to anything readable, since a row that
/// cannot be dated cannot be defended as current.
///
/// Written to a temporary file in the same directory and renamed, exactly as
/// `cli::directory::write_forward_marker` writes forward markers: the rename
/// is atomic within a filesystem, so a reader sees the old complete file or
/// the new complete file and never a half of either. Owner-only, because the
/// file states which hosts answered and which did not, and that is a map of
/// where to knock.
pub fn record(observations: &[Observation]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = path()?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} has no parent directory", path.display())))?;
    std::fs::create_dir_all(directory)?;

    let mut merged: BTreeMap<(String, String), Observation> = BTreeMap::new();
    for existing in load() {
        merged.insert((existing.fact.clone(), existing.vantage.clone()), existing);
    }
    for fresh in observations {
        let key = (fresh.fact.clone(), fresh.vantage.clone());
        let keep = match merged.get(&key) {
            Some(held) => match (held.moment(), fresh.moment()) {
                (Some(held_at), Some(fresh_at)) => fresh_at >= held_at,
                // An undateable row on file is superseded by a dated one, and
                // an undateable incoming row still beats nothing readable.
                (None, _) => true,
                (Some(_), None) => false,
            },
            None => true,
        };
        if keep {
            merged.insert(key, fresh.clone());
        }
    }

    let rows: Vec<&Observation> = merged.values().collect();
    let mut body = serde_json::to_vec_pretty(&rows).map_err(io::Error::other)?;
    body.push(b'\n');

    // The pid keeps two concurrent recorders off one another's temporary file.
    // The renames themselves still race and the last one wins whole, so a
    // recorder that loaded before the other finished drops those rows until
    // the next sweep writes them again. That is accepted rather than locked
    // against: a lock on this path would let a stuck writer block a reader,
    // and an observation one sweep late renders as an age -- which is exactly
    // the thing this module exists to make visible instead of hiding.
    let staging = directory.join(format!(".observations-{}.json.staging", std::process::id()));
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staging)?;
    handle.write_all(&body)?;
    handle.sync_all()?;
    drop(handle);
    // `mode` above applies only when the open created the file; a temporary
    // left behind by a killed process would otherwise carry its old bits into
    // the rename.
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&staging, &path)?;
    Ok(())
}

/// What the fleet knows about one fact right now, and how old that knowledge
/// is.
///
/// The newest observation across all vantages answers, because the question
/// this serves -- "may I rely on this" -- is answered by the most recent look
/// from anywhere; a caller that needs one specific vantage is asking a
/// different question and should read [`load`] and filter it.
///
/// A row whose stamp will not parse is reported `Stale`, never `Fresh` and
/// never `Never`. Somebody looked, so `Never` would be false; the age is
/// unknown, and unknown age is not freshness.
pub fn freshness(fact: &str, ttl: Duration) -> Freshness {
    freshness_in(&load(), fact, ttl)
}

/// [`freshness`] against records already in hand.
///
/// A table asks this once per row. Re-reading and re-parsing the whole file
/// for each cell would make the cost of showing freshness scale with the size
/// of the fleet, and a column that gets slower the more services you run is a
/// column somebody eventually deletes -- which is how the fact lost its reader
/// the first time.
pub fn freshness_in(records: &[Observation], fact: &str, ttl: Duration) -> Freshness {
    let mut newest: Option<(DateTime<Utc>, &Observation)> = None;
    let mut undated: Option<&Observation> = None;
    for row in records {
        if row.fact != fact {
            continue;
        }
        match row.moment() {
            None => undated = Some(row),
            Some(moment) => {
                if newest.as_ref().is_none_or(|(held, _)| moment >= *held) {
                    newest = Some((moment, row));
                }
            }
        }
    }
    let Some((_, row)) = newest else {
        return match undated {
            Some(row) => Freshness::Stale(row.clone()),
            None => Freshness::Never,
        };
    };
    match age(row) {
        Some(span) if span <= ttl => Freshness::Fresh(row.clone()),
        _ => Freshness::Stale(row.clone()),
    }
}

/// How long ago the look happened, clamped at zero.
///
/// A stamp in the future is clock skew between two hosts, not a prophecy. It
/// reads as `just now` rather than as an enormous negative age, because the
/// alternative is a column that renders a skewed laptop as the freshest thing
/// in the fleet or as gibberish, and neither tells an operator about the skew.
fn age(row: &Observation) -> Option<Duration> {
    let moment = row.moment()?;
    Utc::now()
        .signed_duration_since(moment)
        .to_std()
        .ok()
        .or(Some(Duration::ZERO))
}

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// A duration in the largest unit that still says something: `45s`, `14m`,
/// `3h`, `12d`. Twelve days is the number this fleet has to be able to read at
/// a glance.
fn compact(span: Duration) -> String {
    let seconds = span.as_secs();
    if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h", seconds / HOUR)
    } else {
        format!("{}d", seconds / DAY)
    }
}

/// One column's worth of freshness: `just now`, `14m ago`, `stale (3h)`,
/// `never`.
///
/// `stale` is spelled out as a word rather than shown as a bare age, because
/// an age alone is read as a fact about the service and this is a fact about
/// the fleet's knowledge of it. `never` is the same shape for the same reason:
/// an empty cell reads as "fine" to every operator alive.
pub fn render(freshness: &Freshness) -> String {
    match freshness {
        Freshness::Fresh(row) => match age(row) {
            Some(span) if span.as_secs() < MINUTE => "just now".to_string(),
            Some(span) => format!("{} ago", compact(span)),
            None => "just now".to_string(),
        },
        Freshness::Stale(row) => match age(row) {
            Some(span) => format!("stale ({})", compact(span)),
            None => "stale (undated)".to_string(),
        },
        Freshness::Never => "never".to_string(),
    }
}

/// [`render`] over [`freshness`] at [`DEFAULT_TTL`], for the display paths
/// that all want the same question asked the same way.
pub fn describe(fact: &str) -> String {
    render(&freshness(fact, DEFAULT_TTL))
}

/// [`describe`] against records already in hand, for a table that loads the
/// file once and then asks about every row.
pub fn describe_in(records: &[Observation], fact: &str) -> String {
    render(&freshness_in(records, fact, DEFAULT_TTL))
}
