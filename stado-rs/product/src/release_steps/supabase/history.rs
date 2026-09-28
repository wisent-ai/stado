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

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// The versions the linked project's history records as applied, read from
/// the CLI's JSON listing (its text table decorates every cell).
pub(super) fn applied(listing: &str) -> Result<BTreeSet<String>> {
    let document: Value = serde_json::from_str(listing)
        .context("supabase migration list did not answer its JSON listing")?;
    let rows = document["migrations"]
        .as_array()
        .context("supabase migration list answered no migrations array")?;
    Ok(rows
        .iter()
        .filter_map(|row| row["remote"].as_str())
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .collect())
}

fn versions(document: &Value, key: &str) -> Vec<String> {
    document[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The repairs the bundle's declarations call for against `history`:
/// versions to record as applied and versions to record as reverted.
pub(super) fn repairs(
    source: &Path,
    history: &BTreeSet<String>,
) -> Result<(Vec<String>, Vec<String>)> {
    let read = |name: &str| -> Result<Option<Value>> {
        let path = source.join("supabase").join(name);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_slice(&text)
            .map(Some)
            .with_context(|| format!("{} is not JSON", path.display()))
    };
    let mut applied = BTreeSet::new();
    let mut reverted = BTreeSet::new();
    if let Some(split) = read("split-migrations.json")? {
        for (part, original) in split["parts"].as_object().into_iter().flatten() {
            if original
                .as_str()
                .is_some_and(|original| history.contains(original))
                && !history.contains(part)
            {
                applied.insert(part.clone());
            }
        }
    }
    if let Some(baseline) = read("baseline.json")? {
        applied.extend(
            versions(&baseline, "applied")
                .into_iter()
                .filter(|version| !history.contains(version)),
        );
        reverted.extend(
            versions(&baseline, "retired")
                .into_iter()
                .filter(|version| history.contains(version)),
        );
    }
    Ok((
        applied.into_iter().collect(),
        reverted.into_iter().collect(),
    ))
}

/// The receipt fields for what was repaired.
pub(super) fn receipt(applied: &[String], reverted: &[String], answers: Vec<Value>) -> Value {
    json!({"applied": applied, "reverted": reverted, "answers": answers})
}
