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
/// Something answered at the declared place and it is the wrong program: the
/// port is held by a launchd job other than the one declared to serve it.
///
/// Deliberately not [`OBSERVED`]: an answer was the whole of that word's
/// evidence, which is how a declaration naming a port another service had
/// taken stays green: a gateway declared on one loopback port while it
/// serves another and an unrelated job holds the declared one has every
/// probe read `HTTP 404` as an answer and report [`OBSERVED`] for hours.
/// Deliberately not [`UNREACHABLE`] either,
/// because the socket is alive and restarting the declared service repairs
/// nothing — the declaration is what is wrong. This is a failure.
pub const MISOWNED: &str = "misowned";
/// A host the directory declares a standby for a service answered on its
/// standby address: a second copy is serving beside the active host. A
/// standby is by definition not serving, so this is a failure; for Skarbiec it
/// is a second vault taking writes the owner never sees.
pub const STANDBY_SERVING: &str = "standby_serving";

/// One look, by one machine, at one fact, at one moment.
///
/// Every field is a `String` because this record crosses a file, a helper
/// script's stdout and two CLI surfaces, and each place it is narrowed to an
/// enum is a place an unrecognised state gets flattened into a known one. The
/// states this tree writes are [`OBSERVED`], [`UNREACHABLE`], [`UNVERIFIED`]
/// and [`MISOWNED`]; a state written by something newer is carried through
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
    /// [`OBSERVED`], [`UNREACHABLE`], [`UNVERIFIED`], [`MISOWNED`], or a word
    /// from a newer writer, passed through.
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
