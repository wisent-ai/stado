//! The record itself: the four things a look produces, the words a state may
//! carry, and the one spelling of a fact name that every checker shares.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// Something answered at the declared place, from the vantage that was asked.
pub const OBSERVED: &str = "observed";
/// Someone looked and nothing was there. This is a failure.
pub const UNREACHABLE: &str = "unreachable";
/// The look could not happen. Deliberately not [`UNREACHABLE`]: "I did not
/// look" and "I looked and it is gone" send an operator to two different
/// machines.
pub const UNVERIFIED: &str = "unverified";
/// The endpoint answered, but its listener is not the declared program.
///
/// This ownership failure is neither a successful observation nor an
/// unreachable socket. Restarting the declared program does not establish
/// ownership of a port held by another unit.
pub const MISOWNED: &str = "misowned";
/// A declared standby is answering beside the active service.
/// This is an ownership failure, not proof that the active endpoint works.
pub const STANDBY_SERVING: &str = "standby_serving";

/// One look, by one machine, at one fact, at one moment.
///
/// String fields preserve outcomes written by newer versions. Current
/// outcomes include [`OBSERVED`], [`UNREACHABLE`], [`UNVERIFIED`], [`MISOWNED`]
/// and [`STANDBY_SERVING`]; an unknown outcome is never coerced to one of them.
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
    /// A shared outcome constant, or a newer writer's value preserved verbatim.
    pub state: String,
    /// Why, in the operating system's own words where there are any. The
    /// difference between "connection refused" and "timed out" is the
    /// difference between a dead process and a dead route.
    pub detail: String,
    /// When the observation was recorded, in RFC 3339 UTC.
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
    pub(super) fn moment(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.at)
            .ok()
            .map(|stamp| stamp.with_timezone(&Utc))
    }
}

/// The canonical fact name for "is this service reachable".
///
/// One spelling, shared by the writer and by every reader, because a fact
/// recorded under `service:brama@mini` and looked up as `brama` is a fact with
/// no reader -- the exact shape of failure this whole change is against.
pub fn service_fact(name: &str, host: &str) -> String {
    format!("service:{name}@{host}")
}
