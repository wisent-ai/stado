//! What this host's object API stands on at the vault, published beside the
//! resolver's state so `resolver status` can read it with the process
//! stopped, and so a `keyring lock repair` the listener ran is read from the
//! product rather than from its log.
//!
//! The listener (`dashboard::listener::auth::vault`) keeps, per bearer key,
//! the consultation holding that key and the last one that failed, and the
//! one declared crypto repair it started when a failure named the process
//! holding the vault's keyring lock. `/api/state.json` serves the same
//! document on the live process; this file is the copy that answers when the
//! process is the thing being diagnosed. Every change the listener makes to
//! the document is written here at once, atomically and best effort like
//! [`super::published::publish`]: a listener that cannot write its
//! diagnostics still serves.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::probe::age_seconds;
use super::published::state_path;

const VAULT_FILE: &str = "resolver-vault.json";

/// What the listener knows about its consultations of the vault under one
/// key: an object namespace, a release publisher item or the host-health
/// role.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct VaultConsultation {
    /// The `czekam` line of the consultation holding this key now.
    pub(crate) open: Option<String>,
    /// When the consultation holding this key began, RFC 3339 UTC: the
    /// `od:` field of `open`, kept apart so a reader can age it without
    /// parsing the line.
    pub(crate) open_since: Option<String>,
    /// The `blad czekania` line of the last consultation of this key that
    /// failed, until one succeeds.
    pub(crate) failed: Option<String>,
}

/// One run of the declared crypto repair the listener started because a
/// consultation's failure named the process holding the vault's keyring
/// lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyringLockRepair {
    /// The vault's own sentence naming the holder.
    pub(crate) holder: String,
    /// When the repair started, RFC 3339 UTC.
    pub(crate) started: String,
    /// How it ended, or `None` while it runs.
    pub(crate) outcome: Option<String>,
}

/// Every key's consultation state and the keyring-lock repair: the document
/// `/api/state.json` serves under `vault` and this file holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct VaultConsultations {
    #[serde(rename = "consultations")]
    pub(crate) keys: BTreeMap<String, VaultConsultation>,
    #[serde(rename = "keyring_lock_repair")]
    pub(crate) repair: Option<KeyringLockRepair>,
}

impl VaultConsultations {
    pub(crate) fn entry(&mut self, key: &str) -> &mut VaultConsultation {
        self.keys.entry(key.to_string()).or_default()
    }

    /// Each key with the line it is held by and the line it last failed
    /// with, and the keyring-lock repair the listener last ran: the
    /// operator's read of what the listener stands on.
    pub(crate) fn state_json(&self) -> Value {
        serde_json::to_value(self).expect("vault consultations serialize")
    }

    /// Write the document beside the resolver's state, atomically and best
    /// effort; the listener calls this after every change it makes.
    pub(crate) fn publish(&self) {
        let Some(path) = vault_path() else { return };
        let Some(parent) = path.parent() else { return };
        let body = match serde_json::to_vec_pretty(self) {
            Ok(body) => body,
            Err(error) => {
                eprintln!("[dashboard] could not serialize its vault consultations: {error}");
                return;
            }
        };
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        let written = std::fs::create_dir_all(parent)
            .and_then(|()| std::fs::write(&temp, &body))
            .and_then(|()| std::fs::rename(&temp, &path));
        if let Err(error) = written {
            eprintln!(
                "{}",
                crate::primitives::staging::failed_with(
                    format!(
                        "[dashboard] could not publish its vault consultations to {}: {error}",
                        path.display()
                    ),
                    &temp,
                )
            );
        }
    }
}

fn vault_path() -> Option<std::path::PathBuf> {
    state_path().map(|path| path.with_file_name(VAULT_FILE))
}

/// The document the listener on this host last published: none when no
/// listener has published one, the document when the file is there, and
/// the file's own failure when it is there and cannot be read.
fn published() -> Result<Option<VaultConsultations>, String> {
    let Some(path) = vault_path() else {
        return Ok(None);
    };
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the object API's vault consultations at {} cannot be read: {error}",
                path.display()
            ))
        }
    };
    serde_json::from_str(&body).map(Some).map_err(|error| {
        format!(
            "the object API's vault consultations at {} are not the document this build \
             writes: {error}",
            path.display()
        )
    })
}

/// What `resolver status` says about the vault: the published document as
/// JSON, the keyring-lock repair as the listener recorded it, and a blocker
/// for each thing that refuses object requests now: a key whose last
/// consultation failed (requests under it are refused with that line while
/// one consults again), a key held longer than `refresh_seconds` (the
/// resolver re-read its directory while that consultation still stood), the
/// keyring-lock repair while it runs, and a document that cannot be read.
/// Read only when the binds are this host's: the file is this host's.
pub(super) struct VaultReport {
    pub(super) document: Value,
    pub(super) repair: Option<KeyringLockRepair>,
    pub(super) blockers: Vec<String>,
}

pub(super) fn report(local: bool, refresh_seconds: u64) -> VaultReport {
    let mut blockers = Vec::new();
    let published = match (local, published()) {
        (false, _) => None,
        (true, Ok(published)) => published,
        (true, Err(unreadable)) => {
            blockers.push(unreadable);
            None
        }
    };
    let Some(published) = published else {
        return VaultReport {
            document: Value::Null,
            repair: None,
            blockers,
        };
    };
    for (key, consultation) in &published.keys {
        if let Some(failed) = &consultation.failed {
            blockers.push(format!(
                "the object API's last consultation of the vault under {key} failed, so requests \
                 under {key} are refused with that line while one of them consults again: {failed}"
            ));
        }
        if let (Some(open), Some(since)) = (&consultation.open, &consultation.open_since) {
            if let Some(seconds) =
                age_seconds(since).filter(|seconds| *seconds > refresh_seconds as i64)
            {
                blockers.push(format!(
                    "the object API's consultation of the vault under {key} has stood {seconds}s, \
                     longer than the {refresh_seconds}s directory refresh, and every request \
                     under {key} is refused behind it: {open}"
                ));
            }
        }
    }
    if let Some(repair) = &published.repair {
        if repair.outcome.is_none() {
            blockers.push(format!(
                "the object API's keyring lock repair for {} runs since {}: the vault names that \
                 holder, so consultations fail until the lock is released",
                repair.holder, repair.started
            ));
        }
    }
    let mut document = published.state_json();
    if let Some(repair) = &published.repair {
        document["keyring_lock_repair"]["started_seconds_ago"] =
            json!(age_seconds(&repair.started));
    }
    VaultReport {
        document,
        repair: published.repair,
        blockers,
    }
}
