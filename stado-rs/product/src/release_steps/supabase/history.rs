//! What the linked project's migration history says before a push, and the
//! repairs a bundle declares so `db push` applies only what the database has
//! not run.
//!
//! Two declarations, both optional files beside the migrations:
//!
//! - `supabase/split-migrations.json` `{"parts": {"<part>": "<original>"}}`:
//!   a migration the database applied was later split; each part whose
//!   original is in the history is recorded as applied.
//! - `supabase/baseline.json` `{"applied": ["<version>", ...], "retired":
//!   ["<version>", ...]}`: a database whose schema was written before the
//!   product's migrations were release-managed. `applied` names the local
//!   versions that database already holds (recorded as applied when absent
//!   from its history); `retired` names history rows no local file carries
//!   any more (recorded as reverted when present), because `db push` refuses
//!   a history with versions it cannot find locally.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use stado_wait as wait;

#[derive(Deserialize)]
struct Migration {
    version: String,
}

/// Supabase's project-scoped Management API lists applied versions as JSON.
/// The CLI's migration listing is a display table, not a machine interface.
pub(super) fn applied(project: &str, token: &str) -> Result<BTreeSet<String>> {
    // https://supabase.com/docs/reference/api/v1-list-migration-history
    let mut endpoint = reqwest::Url::parse("https://api.supabase.com/v1/projects/")?;
    endpoint
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Supabase management URL cannot hold path segments"))?
        .pop_if_empty()
        .push(project)
        .push("database")
        .push("migrations");
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .with_context(|| format!("GET {endpoint}: creating migration history client"))?;
    let response = wait::request_blocking(client.get(endpoint.clone()).bearer_auth(token))
        .with_context(|| format!("GET {endpoint}: reading applied migration history failed"))?;
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("GET {endpoint}: reading HTTP {status} response"))?;
    if !status.is_success() {
        bail!("GET {endpoint}: HTTP {status}: {body}");
    }
    let rows: Vec<Migration> = serde_json::from_str(&body)
        .with_context(|| format!("GET {endpoint}: HTTP {status}: invalid migration history"))?;
    let mut versions = BTreeSet::new();
    for row in rows {
        if row.version.trim().is_empty() {
            bail!("GET {endpoint}: HTTP {status}: migration history contains a blank version");
        }
        versions.insert(row.version);
    }
    Ok(versions)
}

#[derive(Deserialize)]
struct Split {
    parts: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Baseline {
    applied: Vec<String>,
    retired: Vec<String>,
}

/// Validated bundle declarations, loaded before contacting the provider.
pub(super) struct Declarations {
    split: Option<Split>,
    baseline: Option<Baseline>,
}

fn read<T: DeserializeOwned>(source: &Path, name: &str) -> Result<Option<T>> {
    let path = source.join("supabase").join(name);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .with_context(|| format!("invalid migration declaration {}", path.display()))
}

fn version(source: &Path, name: &str, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!(
            "invalid migration declaration {}: {field} contains a blank version",
            source.join("supabase").join(name).display()
        );
    }
    Ok(())
}

impl Declarations {
    pub(super) fn load(source: &Path) -> Result<Self> {
        let split: Option<Split> = read(source, "split-migrations.json")?;
        let baseline: Option<Baseline> = read(source, "baseline.json")?;
        if let Some(split) = &split {
            for (part, original) in &split.parts {
                version(source, "split-migrations.json", "parts key", part)?;
                version(source, "split-migrations.json", "parts value", original)?;
            }
        }
        if let Some(baseline) = &baseline {
            for (field, versions) in [
                ("applied", &baseline.applied),
                ("retired", &baseline.retired),
            ] {
                for value in versions {
                    version(source, "baseline.json", field, value)?;
                }
            }
        }
        Ok(Self { split, baseline })
    }

    /// Versions to record as applied and versions to record as reverted.
    pub(super) fn repairs(&self, history: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
        let mut applied = BTreeSet::new();
        let mut reverted = BTreeSet::new();
        if let Some(split) = &self.split {
            for (part, original) in &split.parts {
                if history.contains(original) && !history.contains(part) {
                    applied.insert(part.clone());
                }
            }
        }
        if let Some(baseline) = &self.baseline {
            applied.extend(
                baseline
                    .applied
                    .iter()
                    .filter(|v| !history.contains(*v))
                    .cloned(),
            );
            reverted.extend(
                baseline
                    .retired
                    .iter()
                    .filter(|v| history.contains(*v))
                    .cloned(),
            );
        }
        (
            applied.into_iter().collect(),
            reverted.into_iter().collect(),
        )
    }
}

/// The receipt fields for what was repaired.
pub(super) fn receipt(applied: &[String], reverted: &[String], answers: Vec<Value>) -> Value {
    json!({"applied": applied, "reverted": reverted, "answers": answers})
}
