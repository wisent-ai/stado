//! Quality gate failures a source install records, so the same committed
//! revision is refused with the recorded failures instead of being exported
//! and checked again.
//!
//! A quality gate (`fmt`, `clippy`) is a function of the committed tree, and
//! the toolchain is part of that tree (`rust-toolchain.toml`), so a revision
//! that failed one fails it again: repeating the install spends an export, a
//! compile and a gigabyte of disk to print the same refusal. A build failure
//! is not recorded, because a build can fail for a reason outside the tree.
//! Every gate runs to its end whatever the one before it said, and every
//! refusal is recorded and returned together: stopping at the first one made
//! each installation find only the next refusal.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, path::PathBuf};

use crate::common::atomic_json;

/// The quality gate that refused a revision, as the install saw it.
pub(in crate::install::recipes::build) struct QualityFailure {
    pub check: Value,
    pub error: String,
}

/// Where a checkout's installs record their quality failures: its build area
/// under Stado's home ([`crate::common::runs::checkout_area`]), never the
/// checkout, which sits in `~/Documents` where no janitor pass reaches.
fn record_path(area: &Path, key: &str) -> PathBuf {
    area.join("install-failures").join(format!("{key}.json"))
}

/// The product, platform, revision and catalog recipe an install builds: a
/// different recipe is a different install even at one revision.
pub(in crate::install::recipes::build) fn key(
    id: &str,
    platform: &str,
    revision: &str,
    recipe: &Value,
) -> String {
    let recipe = recipe.to_string();
    let mut digest = Sha256::new();
    for part in [id, platform, revision, recipe.as_str()] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

/// What a record says when the gate ran and refused the tree. Records written
/// before this value also hold failures to start a gate's program on the
/// host — a missing program, or a script whose shell exited 126 or 127 — which
/// say nothing about the revision, so only a record carrying it refuses an
/// install, and an older record lets the gate run once more.
const GATE_VERDICT: &str = "tree-verdict";

/// Refuse an install whose revision already failed its quality gates, naming
/// every gate the recorded run saw refuse.
pub(in crate::install::recipes::build) fn refuse_recorded(area: &Path, key: &str) -> Result<()> {
    let path = record_path(area, key);
    let body = match fs::read(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("reading the recorded install failure {}", path.display())
            })
        }
    };
    let recorded: Value = serde_json::from_slice(&body)
        .with_context(|| format!("reading the recorded install failure {}", path.display()))?;
    if recorded["kind"].as_str() != Some(GATE_VERDICT) {
        return Ok(());
    }
    let field = |name: &str| match recorded[name].as_str() {
        Some(value) => value.to_string(),
        None => format!("(the record {} names no {name})", path.display()),
    };
    // A record written before every gate ran to its end holds one `check`
    // and its `error`; a later one holds all of them under `failures`.
    let failures: Vec<String> = match recorded["failures"].as_array() {
        Some(failures) => failures
            .iter()
            .map(|failure| format!("quality {}: {}", failure["check"], failure["error"]))
            .collect(),
        None => vec![format!("quality {}: {}", recorded["check"], field("error"))],
    };
    bail!(
        "{} {} already failed its quality gates at {}, every gate run to its end:\n{}\nbuild \
         evidence: {}. The same revision fails the same gates; commit a repair and install that \
         revision",
        field("product"),
        field("revision"),
        field("recorded_at"),
        failures.join("\n"),
        field("evidence"),
    )
}

/// Record every quality gate a revision failed.
pub(in crate::install::recipes::build) fn record(
    area: &Path,
    key: &str,
    install: (&str, &str, &Path),
    failures: &[QualityFailure],
) -> Result<()> {
    let (product, revision, evidence) = install;
    let path = record_path(area, key);
    let parent = path
        .parent()
        .context("install failure record has no parent")?;
    fs::create_dir_all(parent)?;
    let failures: Vec<Value> = failures
        .iter()
        .map(|failure| json!({ "check": failure.check, "error": failure.error }))
        .collect();
    atomic_json(
        &path,
        &json!({
            "kind": GATE_VERDICT,
            "product": product,
            "revision": revision,
            "failures": failures,
            "evidence": evidence.display().to_string(),
            "recorded_at": chrono::Utc::now().to_rfc3339(),
        }),
    )
}

/// Run every quality check of a release platform to its end; the checks that
/// refused the tree are recorded together against `key` in the checkout's
/// build area and returned as one error naming each with its evidence.
pub(in crate::install::recipes::build) fn gate(
    area: &Path,
    key: &str,
    install: (&str, &str, &Path),
    quality: &Value,
    mut run: impl FnMut(&Value) -> Result<()>,
) -> Result<()> {
    let (product, _, evidence) = install;
    let mut failures = Vec::new();
    let mut host_faults = Vec::new();
    for check in quality
        .as_array()
        .context("release quality must be an array")?
    {
        let Err(error) = run(check) else {
            continue;
        };
        // A step whose program could not be started says nothing about the
        // revision: the host lacked the program. Recording it would refuse
        // every later attempt at the same revision with "the same revision
        // fails the same gates" after the host is repaired. A script the shell
        // ran but whose own program it could not find or run is the same host
        // fault one level down: POSIX gives the shell's exit status for those
        // as 127 and 126.
        let host_lacked_program = error
            .root_cause()
            .downcast_ref::<std::io::Error>()
            .is_some()
            || error
                .root_cause()
                .downcast_ref::<crate::common::CommandFailed>()
                .is_some_and(|failed| matches!(failed.code, Some(126 | 127)));
        let failure = QualityFailure {
            check: check["name"].clone(),
            error: format!("{error:#}"),
        };
        match host_lacked_program {
            true => host_faults.push(failure),
            false => failures.push(failure),
        }
    }
    if !failures.is_empty() {
        record(area, key, install, &failures)?;
    }
    let said: Vec<String> = failures
        .iter()
        .map(|failure| {
            format!(
                "quality {} refused the revision: {}",
                failure.check, failure.error
            )
        })
        .chain(host_faults.iter().map(|failure| {
            format!(
                "quality {} could not run on this host: {}",
                failure.check, failure.error
            )
        }))
        .collect();
    if said.is_empty() {
        return Ok(());
    }
    bail!(
        "{product} quality failed, every gate run to its end; build evidence: {}:\n{}",
        evidence.display(),
        said.join("\n")
    )
}
