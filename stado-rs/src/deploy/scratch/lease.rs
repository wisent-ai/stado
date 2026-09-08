//! The record that makes a disposable target reapable.
//!
//! `host user create` had no lifetime, and the module that added deletion says
//! why in its own words: "an account provisioned for a one-off task could only
//! be removed by hand over ad-hoc SSH — which is how a test account outlived
//! its purpose on a managed mac". A lease exists so that outliving is bounded
//! by a stamp on the host rather than by someone remembering.
//!
//! The record lives ON the host, in the login account's home, for one reason:
//! whoever reaps it needs only the host. A run that crashes, a laptop that
//! goes away, a store that cannot be reached — none of them can strand an
//! account, because the fact that it exists is written where the machine
//! itself can be asked.

use std::path::PathBuf;
use std::sync::LazyLock;

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use super::declaration::Duration;
use crate::deploy::DeployError;

/// The record contract. A record from another schema is reported, never
/// silently reinterpreted.
pub const RECORD_SCHEMA: &str = "stado.scratch-lease.v1";

/// Where the records live on the host, relative to the login account's home.
pub const HOST_LEASE_DIR: &str = ".stado/scratch";

/// Where the emitted registry documents live on the caller's machine.
pub const LOCAL_ROOT_DIR: &str = ".stado/scratch";

static NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9-]{0,30}$").expect("static regex compiles"));

/// One disposable target's whole story: what it is, on what, since when, and
/// until when.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ScratchLease {
    pub schema: String,
    pub name: String,
    pub username: String,
    pub profile: String,
    pub target: String,
    pub created_at: String,
    pub expires_at: String,
    /// The machine and account that asked, so an operator reading a stranded
    /// lease knows which run to go and look at.
    pub requested_by: String,
}

impl ScratchLease {
    /// A fresh lease, stamped from the caller's clock in UTC.
    pub fn new(
        name: &str,
        profile: &str,
        target: &str,
        ttl: Duration,
    ) -> Result<Self, DeployError> {
        let created = Utc::now();
        let span = TimeDelta::try_minutes(ttl.minutes())
            .ok_or_else(|| DeployError("lease lifetime does not fit a timestamp".to_string()))?;
        let expires = created
            .checked_add_signed(span)
            .ok_or_else(|| DeployError("lease lifetime does not fit a timestamp".to_string()))?;
        Ok(Self {
            schema: RECORD_SCHEMA.to_string(),
            name: name.to_string(),
            username: name.to_string(),
            profile: profile.to_string(),
            target: target.to_string(),
            created_at: stamp(created),
            expires_at: stamp(expires),
            requested_by: requested_by(),
        })
    }

    /// Whether this lease's time is up, measured against `now`. An unparseable
    /// stamp counts as expired: a record nobody can date is a leak, and the
    /// reaper is the only thing that removes leaks.
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        match self.expiry() {
            Some(expiry) => expiry <= now,
            None => true,
        }
    }

    /// Seconds left, negative once the lease is over, `None` when the stamp
    /// cannot be read.
    pub fn seconds_remaining(&self, now: DateTime<Utc>) -> Option<i64> {
        self.expiry()
            .map(|expiry| expiry.signed_duration_since(now).num_seconds())
    }

    fn expiry(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.expires_at)
            .ok()
            .map(|parsed| parsed.with_timezone(&Utc))
    }

    /// The record's path in a login account's home on the host.
    pub fn record_path(&self, home: &str) -> String {
        record_path(home, &self.name)
    }
}

/// One record's path, for a name that may not have a parsed record yet.
pub fn record_path(home: &str, name: &str) -> String {
    format!(
        "{}/{HOST_LEASE_DIR}/{name}.json",
        home.trim_end_matches('/')
    )
}

/// The directory the records live in on the host.
pub fn record_dir(home: &str) -> String {
    format!("{}/{HOST_LEASE_DIR}", home.trim_end_matches('/'))
}

/// The caller-side directory holding one lease's emitted registry.
pub fn local_root(name: &str) -> PathBuf {
    crate::config_file::expand_tilde(&format!("~/{LOCAL_ROOT_DIR}/{name}"))
}

/// The name rule, which is also the account name rule: a scratch target and
/// its login are one identity, so there is no mapping to get wrong.
pub fn validate_name(name: &str) -> Result<(), DeployError> {
    if NAME_RE.is_match(name) {
        return Ok(());
    }
    Err(DeployError(format!(
        "scratch names are lowercase [a-z0-9-] beginning with a letter; '{name}' is not"
    )))
}

/// A fresh name nobody has to choose: the prefix an operator can grep for,
/// plus enough randomness that two runs on one host never collide.
pub fn generate_name() -> String {
    let random = uuid::Uuid::new_v4().simple().to_string();
    let suffix: String = random.chars().take(NAME_SUFFIX_LENGTH.into()).collect();
    format!("scratch-{suffix}")
}

/// Who asked, in the form `machine/account`.
fn requested_by() -> String {
    let machine = crate::providers::vast::system_hostname();
    let account = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    format!("{machine}/{account}")
}

/// The moment now, in the stamp form every record and report uses.
pub fn now_stamp() -> String {
    stamp(Utc::now())
}

/// One RFC 3339 second-resolution UTC stamp, the form every other Stado
/// record uses.
fn stamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Hex characters of randomness in a generated name. Long enough that a
/// collision on one host is not a thing that happens, short enough that the
/// whole account name stays inside the portable username limit.
const NAME_SUFFIX_LENGTH: u8 = 6;
