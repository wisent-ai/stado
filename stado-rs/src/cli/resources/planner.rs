//! Canonical plan creation, hashing, loading, and dependency ordering.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::queue::copy::Endpoint;

use super::model::{
    Action, Condition, Finding, Intent, InventorySnapshot, OperationScope, Plan, SCHEMA_VERSION,
};
use super::CmdError;

pub fn new_plan(
    intent: Intent,
    scope: OperationScope,
    inventory: InventorySnapshot,
    findings: Vec<Finding>,
    actions: Vec<Action>,
) -> Result<Plan, CmdError> {
    let now = Utc::now();
    let prefix = match intent {
        Intent::RationalizationCleanup => "rationalize",
        Intent::Shutdown => "shutdown",
    };
    let plan = Plan {
        schema_version: SCHEMA_VERSION,
        operation_id: format!(
            "{prefix}-{}-{}",
            now.format("%Y%m%dT%H%M%SZ"),
            Uuid::new_v4().simple()
        ),
        intent,
        created_at: timestamp(now),
        expires_at: timestamp(now + Duration::days(true as i64)),
        stado_version: env!("CARGO_PKG_VERSION").to_string(),
        scope,
        configuration_fingerprint: configuration_fingerprint()?,
        inventory,
        findings,
        actions,
    };
    plan.validate()?;
    topological_order(&plan)?;
    Ok(plan)
}

pub fn write_plan(plan: &Plan, path: &Path) -> Result<String, CmdError> {
    plan.validate()?;
    topological_order(plan)?;
    let bytes = plan.canonical_bytes()?;
    atomic_write(path, &bytes)?;
    plan.sha256()
}

pub fn read_plan(path: &Path, expected_hash: &str, intent: Intent) -> Result<Plan, CmdError> {
    let bytes = fs::read(path)?;
    let plan: Plan = serde_json::from_slice(&bytes)?;
    plan.validate()?;
    if plan.intent != intent {
        return Err(CmdError::usage(format!(
            "plan intent is {:?}, but this command accepts {:?}",
            plan.intent, intent
        )));
    }
    if plan.canonical_bytes()? != bytes {
        return Err(CmdError::click(
            "plan is not canonical Stado JSON; regenerate it instead of editing it",
        ));
    }
    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected_hash) {
        return Err(CmdError::click(format!(
            "plan hash mismatch: expected {expected_hash}, actual {actual}"
        )));
    }
    let expires = DateTime::parse_from_rfc3339(&plan.expires_at)
        .map_err(|error| CmdError::click(format!("invalid plan expiry: {error}")))?;
    if expires.with_timezone(&Utc) <= Utc::now() {
        return Err(CmdError::click(format!(
            "plan expired at {}; generate a fresh inventory and plan",
            plan.expires_at
        )));
    }
    if configuration_fingerprint()? != plan.configuration_fingerprint {
        return Err(CmdError::click(
            "Stado configuration changed after planning; generate a fresh plan",
        ));
    }
    topological_order(&plan)?;
    Ok(plan)
}

pub fn topological_order(plan: &Plan) -> Result<Vec<&Action>, CmdError> {
    let by_id: BTreeMap<&str, &Action> = plan
        .actions
        .iter()
        .map(|action| (action.id.as_str(), action))
        .collect();
    let mut remaining: BTreeSet<&str> = by_id.keys().copied().collect();
    let mut emitted = BTreeSet::new();
    let mut ordered = Vec::with_capacity(plan.actions.len());
    while !remaining.is_empty() {
        let ready: Vec<&str> = remaining
            .iter()
            .copied()
            .filter(|id| {
                by_id[id]
                    .depends_on
                    .iter()
                    .all(|dependency| emitted.contains(dependency.as_str()))
            })
            .collect();
        if ready.is_empty() {
            return Err(CmdError::click("resource plan contains a dependency cycle"));
        }
        for id in ready {
            remaining.remove(id);
            emitted.insert(id);
            ordered.push(by_id[id]);
        }
    }
    Ok(ordered)
}

pub fn parse_age(raw: &str) -> Result<Duration, CmdError> {
    let raw = raw.trim();
    let (digits, multiplier) = match raw.as_bytes().last().copied() {
        Some(b's') => (&raw[..raw.len().saturating_sub(true as usize)], true as i64),
        Some(b'm') => (
            &raw[..raw.len().saturating_sub(true as usize)],
            Duration::minutes(true as i64).num_seconds(),
        ),
        Some(b'h') => (
            &raw[..raw.len().saturating_sub(true as usize)],
            Duration::hours(true as i64).num_seconds(),
        ),
        Some(b'd') => (
            &raw[..raw.len().saturating_sub(true as usize)],
            Duration::days(true as i64).num_seconds(),
        ),
        _ => {
            return Err(CmdError::usage(
                "age must include s, m, h, or d, for example 24h",
            ))
        }
    };
    let count = digits
        .parse::<i64>()
        .map_err(|_| CmdError::usage(format!("invalid age {raw:?}")))?;
    let seconds = count
        .checked_mul(multiplier)
        .ok_or_else(|| CmdError::usage(format!("age {raw:?} is too large")))?;
    if seconds <= i64::default() {
        return Err(CmdError::usage("age must be greater than zero"));
    }
    Ok(Duration::seconds(seconds))
}

pub fn condition(field: &str, expected: Value) -> Condition {
    Condition {
        field: field.to_string(),
        expected,
    }
}
pub fn configuration_fingerprint() -> Result<String, CmdError> {
    let primary = Endpoint::configured_primary();
    let value = json!({
        "providers": crate::config::wc_providers(),
        "providers_disabled": crate::config::wc_disabled_providers(),
        "primary_storage": primary.describe(),
        "gcp_project": crate::config::project(),
        "gcp_region": crate::config::region(),
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&value)?)))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click(format!("{} has no parent", path.display())))?;
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}
