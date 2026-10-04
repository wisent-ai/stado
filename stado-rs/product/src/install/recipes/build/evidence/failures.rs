//! Quality gate failures a source install records, so the same committed
//! revision is refused with the recorded failure instead of being exported and
//! checked again.
//!
//! A quality gate (`fmt`, `clippy`) is a function of the committed tree, and
//! the toolchain is part of that tree (`rust-toolchain.toml`), so a revision
//! that failed one fails it again: repeating the install spends an export, a
//! compile and a gigabyte of disk to print the same refusal. A build failure
//! is not recorded, because a build can fail for a reason outside the tree.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, path::PathBuf};

use crate::common::atomic_json;

/// The quality gate that refused a revision, as the install saw it.
pub(in crate::install::recipes::build) struct QualityFailure {
    pub product: String,
    pub revision: String,
    pub check: Value,
    pub error: String,
}

/// Where one checkout keeps its installs' recorded quality failures.
fn record_path(root: &Path, key: &str) -> PathBuf {
    root.join(".wisent-output/install-failures")
        .join(format!("{key}.json"))
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
/// before this field existed also hold failures to start a gate's program on
/// the host, which say nothing about the revision, so only a record carrying
/// it refuses an install.
const GATE_VERDICT: &str = "gate-verdict";

/// Refuse an install whose revision already failed a quality gate.
pub(in crate::install::recipes::build) fn refuse_recorded(root: &Path, key: &str) -> Result<()> {
    let path = record_path(root, key);
    let Ok(body) = fs::read(&path) else {
        return Ok(());
    };
    let recorded: Value = serde_json::from_slice(&body)
        .with_context(|| format!("reading the recorded install failure {}", path.display()))?;
    if recorded["kind"].as_str() != Some(GATE_VERDICT) {
        return Ok(());
    }
    bail!(
        "{} {} already failed quality {} at {}: {}; build evidence: {}. The same revision fails \
         the same gate; commit a repair and install that revision",
        recorded["product"].as_str().unwrap_or_default(),
        recorded["revision"].as_str().unwrap_or_default(),
        recorded["check"],
        recorded["recorded_at"].as_str().unwrap_or_default(),
        recorded["error"].as_str().unwrap_or_default(),
        recorded["evidence"].as_str().unwrap_or_default(),
    )
}

/// Record the quality gate a revision failed.
pub(in crate::install::recipes::build) fn record(
    root: &Path,
    key: &str,
    failure: &QualityFailure,
    evidence: &Path,
) -> Result<()> {
    let path = record_path(root, key);
    let parent = path
        .parent()
        .context("install failure record has no parent")?;
    fs::create_dir_all(parent)?;
    atomic_json(
        &path,
        &json!({
            "kind": GATE_VERDICT,
            "product": failure.product,
            "revision": failure.revision,
            "check": failure.check,
            "error": failure.error,
            "evidence": evidence.display().to_string(),
            "recorded_at": chrono::Utc::now().to_rfc3339(),
        }),
    )
}

/// Run a release platform's quality checks in order; the first that fails is
/// recorded against `key` in the checkout and returned with its evidence.
pub(in crate::install::recipes::build) fn gate(
    checkout: &Path,
    key: &str,
    install: (&str, &str, &Path),
    quality: &Value,
    mut run: impl FnMut(&Value) -> Result<()>,
) -> Result<()> {
    let (product, revision, evidence) = install;
    for check in quality
        .as_array()
        .context("release quality must be an array")?
    {
        if let Err(error) = run(check) {
            let error = error.context(format!(
                "{product} quality {} failed; build evidence: {}",
                check["name"],
                evidence.display()
            ));
            // A step whose program could not be started says nothing about
            // the revision: the host lacked the program. Recording it would
            // refuse every later attempt at the same revision with "the same
            // revision fails the same gate" after the host is repaired.
            if error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some()
            {
                return Err(error);
            }
            let failure = QualityFailure {
                product: product.to_owned(),
                revision: revision.to_owned(),
                check: check["name"].clone(),
                error: format!("{error:#}"),
            };
            record(checkout, key, &failure, evidence)?;
            return Err(error);
        }
    }
    Ok(())
}
