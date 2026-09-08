//! The host ledger document itself: where it lives, how it is read, and how
//! it is durably committed.

pub(crate) mod attempt;
pub(crate) mod identity;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::deploy::service::ImageIdentity;

use attempt::RevisitAttempt;
use identity::AttemptOutcome;

pub(crate) const REVISIT_SCHEMA: u32 = 1;

/// The host ledger and its lock, inside the release `state_dir`.
///
/// The `@` is load-bearing: this directory also holds `<product>.json` and
/// `<product>-proxy.json`, and `release_control::identifier` admits no `@`, so
/// no product coordinate can ever name the same file.
const LEDGER_FILE: &str = "@unit-images.revisit.json";
pub(in crate::release_unit_image) const LOCK_STEM: &str = "@unit-images.revisit";

/// Every restart this host has spent on unit images, keyed by launchd label.
///
/// One document per host, never per product: which image a pid executes is a
/// property of the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitLedger {
    pub schema_version: u32,
    pub host: String,
    #[serde(default)]
    pub attempts: BTreeMap<String, RevisitAttempt>,
}

impl RevisitLedger {
    fn new(host: &str) -> Self {
        Self {
            schema_version: REVISIT_SCHEMA,
            host: host.to_string(),
            attempts: BTreeMap::new(),
        }
    }

    /// The attempt that bars `unit`, if one does.
    pub(in crate::release_unit_image) fn barring(
        &self,
        unit: &str,
        running: &ImageIdentity,
        declared: &ImageIdentity,
    ) -> Option<&RevisitAttempt> {
        self.attempts
            .get(unit)
            .filter(|attempt| attempt.bars(running, declared))
    }
}

fn ledger_path(state_dir: &str) -> PathBuf {
    Path::new(state_dir).join(LEDGER_FILE)
}

/// Read the host ledger, or start an empty one.
///
/// An absent file is an empty ledger. A file that cannot be parsed, belongs to
/// another host or schema, or carries an outcome outside
/// [`AttemptOutcome`]'s closed vocabulary is an error and NOT an empty ledger:
/// reading a ledger as empty because it could not be understood is how a
/// bounded remedy becomes an unbounded one.
pub(in crate::release_unit_image) fn load_ledger(
    state_dir: &str,
    host: &str,
) -> Result<RevisitLedger, String> {
    let path = ledger_path(state_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RevisitLedger::new(host))
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let ledger: RevisitLedger = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not a revisit ledger: {error}", path.display()))?;
    if ledger.schema_version != REVISIT_SCHEMA {
        return Err(format!(
            "{} carries schema {} and this build reads {REVISIT_SCHEMA}",
            path.display(),
            ledger.schema_version
        ));
    }
    if ledger.host != host {
        return Err(format!(
            "{} belongs to {} and this machine is {host}",
            path.display(),
            ledger.host
        ));
    }
    for (unit, attempt) in &ledger.attempts {
        if AttemptOutcome::parse(&attempt.outcome).is_none() {
            return Err(format!(
                "{} carries unknown outcome {:?} for unit {:?}",
                path.display(),
                attempt.outcome,
                unit
            ));
        }
    }
    Ok(ledger)
}

/// Durably commit the ledger through the release agent's one JSON writer:
/// same-directory unique staging, `create_new`, `write_all`, `sync_all`, then
/// rename. In particular, the `Attempting` safety boundary is not considered
/// written until those bytes are synced and committed.
pub(in crate::release_unit_image) fn save_ledger(
    state_dir: &str,
    ledger: &RevisitLedger,
) -> Result<(), String> {
    crate::release_agent::atomic_json(&ledger_path(state_dir), ledger)
}
