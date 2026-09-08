//! Lease a disposable target on a host the fleet already manages.
//!
//! A capability that installs a runner, restarts a unit, reclaims a byte or
//! delivers a release can only be proved on a machine. The operator's own
//! machines are not that machine: a test may not touch the registry, vault or
//! hosts a fleet actually runs on. Renting one per run costs money, and the
//! provider adapters that create ephemeral VMs are the dispatcher's, not a
//! test's.
//!
//! So this is the third way, and it uses what the fleet already has: a
//! throwaway local account on a registered host, created through the same
//! registry-authorized channel `host user create` rides, trusted by the keys
//! that already reach that host, declared in a registry document of its own,
//! and destroyed when its lease is up.
//!
//! Three properties are the whole design:
//!
//! - **The lease is written on the host.** Whoever reaps it needs only the
//!   machine — not the store, not the caller, not a memory of what ran.
//! - **Every claim is read back from the host.** `list` asks whether the
//!   account is really there; `destroy` believes a probe, never the delete
//!   command's own word.
//! - **A lifetime is not optional.** `create` reaps that host's expired leases
//!   before taking a new one, and the agent's janitor tick reaps locally, so a
//!   leaked account is bounded by its declared TTL.

pub mod declaration;
pub mod lease;
pub mod registry_out;
pub mod remote;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use declaration::render_duration;
use lease::ScratchLease;
use remote::HostLease;

use crate::deploy::{host_channel, host_users, DeployError, Runner};
use crate::targets::ComputeTarget;

/// What one lease needs to be taken.
#[derive(Debug, Clone, Default)]
pub struct LeaseRequest {
    pub target: String,
    pub profile: String,
    pub name: Option<String>,
    pub ttl: Option<String>,
    pub root: Option<PathBuf>,
}

/// Take one lease: create the account, trust it, record it, prove it can be
/// entered, and declare it in a registry of its own.
pub async fn create(
    request: &LeaseRequest,
    runner: &Runner,
) -> Result<Map<String, Value>, DeployError> {
    let profile = declaration::profile(&request.profile)?;
    let target = host_channel::canonical_target(&request.target).await?;
    if !profile.accepts_platform(&target.release_platform) {
        return Err(profile.platform_refusal(&target.name, &target.release_platform));
    }
    let ttl = profile.lease_ttl(request.ttl.as_deref())?;
    let name = match request.name.as_deref() {
        Some(chosen) => {
            lease::validate_name(chosen)?;
            chosen.to_string()
        }
        None => lease::generate_name(),
    };
    host_users::validate_username(&name)?;

    let home = host_channel::remote_home(&target, runner).await?;
    let held = host_leases(&target, &home, runner).await?;
    let swept = reap_rows(&target, &home, &held, true, runner).await?;
    if let Some(row) = held
        .iter()
        .find(|row| row.name == name && !swept.destroyed.contains(&row.name))
    {
        let until = row.lease.as_ref().map_or_else(
            || "an unreadable record".to_string(),
            |held| held.expires_at.clone(),
        );
        return Err(DeployError(format!(
            "scratch lease '{name}' already exists on '{}' and expires at {until}",
            target.name
        )));
    }

    // The root is checked before an account exists, so a refusal here leaves
    // the host exactly as it was.
    let root = request
        .root
        .clone()
        .unwrap_or_else(|| lease::local_root(&name));
    if root.exists() {
        return Err(DeployError(format!(
            "{} already exists; refusing to write a scratch registry over it",
            root.display()
        )));
    }

    let record = ScratchLease::new(&name, &profile.name, &target.name, ttl)?;
    open_account(&target, &name, &profile.shell, runner).await?;

    let ssh = scratch_ssh(&target, &name)?;
    let leased: ComputeTarget = serde_json::from_value(
        registry_out::document(&record, &target, &ssh)["targets"][0].clone(),
    )
    .map_err(|exc| DeployError(format!("the leased target does not parse: {exc}")))?;

    let entered = match settle(&target, &record, &home, &leased, runner).await {
        Ok(entered) => entered,
        Err(exc) => {
            // Never leave an account nobody can enter: undo what this call made
            // and report the original failure, with the rollback's own verdict.
            let rollback = destroy_row(&target, &home, &row_for(&record), runner).await;
            let detail = match rollback {
                Ok(_) => "the account was deleted again".to_string(),
                Err(rollback) => format!("the rollback also failed: {}", rollback.0),
            };
            return Err(DeployError(format!("{}; {detail}", exc.0)));
        }
    };

    let registry_path = registry_out::write(&root, &record, &target, &ssh)?;

    let mut report = host_channel::base_report(&target);
    report.insert("name".into(), Value::from(record.name.clone()));
    report.insert("target".into(), Value::from(target.name.clone()));
    report.insert("profile".into(), Value::from(record.profile.clone()));
    report.insert(
        "mechanism".into(),
        Value::from(profile.mechanism.as_str().to_string()),
    );
    report.insert("username".into(), Value::from(record.username.clone()));
    report.insert("ssh".into(), Value::from(ssh));
    report.insert("created_at".into(), Value::from(record.created_at.clone()));
    report.insert("expires_at".into(), Value::from(record.expires_at.clone()));
    report.insert("ttl".into(), Value::from(render_duration(ttl)));
    report.insert(
        "storage_root".into(),
        Value::from(root.display().to_string()),
    );
    report.insert(
        "registry_path".into(),
        Value::from(registry_path.display().to_string()),
    );
    report.insert("account".into(), Value::from("created"));
    report.insert("verified_login".into(), Value::from(entered.login));
    report.insert("home_path".into(), Value::from(entered.home_path));
    report.insert("reaped".into(), json!(swept.destroyed));
    report.insert("reap_failures".into(), json!(swept.failures));
    report.insert("exit_code".into(), Value::from(entered.exit_code));
    report.insert("status".into(), Value::from("leased"));
    Ok(report)
}

/// Every lease a host holds, each with the account's real presence.
pub async fn list(target_name: &str, runner: &Runner) -> Result<Map<String, Value>, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let home = host_channel::remote_home(&target, runner).await?;
    let held = host_leases(&target, &home, runner).await?;
    let now = Utc::now();
    let mut report = host_channel::base_report(&target);
    report.insert(
        "leases".into(),
        Value::Array(held.iter().map(|row| lease_json(row, now)).collect()),
    );
    report.insert("status".into(), Value::from("read"));
    Ok(report)
}

/// Destroy one lease by name, and prove the host is clear of it.
pub async fn destroy(
    target_name: &str,
    name: &str,
    runner: &Runner,
) -> Result<Map<String, Value>, DeployError> {
    lease::validate_name(name)?;
    let target = host_channel::canonical_target(target_name).await?;
    let home = host_channel::remote_home(&target, runner).await?;
    let held = host_leases(&target, &home, runner).await?;
    let Some(row) = held.iter().find(|row| row.name == name) else {
        return Err(DeployError(format!(
            "no scratch lease named '{name}' on '{}'",
            target.name
        )));
    };
    let mut report = host_channel::base_report(&target);
    for (key, value) in destroy_row(&target, &home, row, runner).await? {
        report.insert(key, value);
    }
    report.insert("status".into(), Value::from("destroyed"));
    Ok(report)
}

/// Destroy every expired lease on one host, previewing unless `apply`.
pub async fn reap(
    target_name: &str,
    apply: bool,
    runner: &Runner,
) -> Result<Map<String, Value>, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    reap_target(&target, apply, runner).await
}

/// The sweep, against an already-resolved target. This is the entry point the
/// host agent's janitor tick uses: it holds its own registry entry already, and
/// a sweep that re-resolved the registry would go silent exactly when the store
/// does.
pub async fn reap_target(
    target: &ComputeTarget,
    apply: bool,
    runner: &Runner,
) -> Result<Map<String, Value>, DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let held = host_leases(target, &home, runner).await?;
    let now = Utc::now();
    let swept = reap_rows(target, &home, &held, apply, runner).await?;
    let mut report = host_channel::base_report(target);
    report.insert("apply".into(), Value::from(apply));
    report.insert(
        "leases".into(),
        Value::Array(
            held.iter()
                .map(|row| {
                    let mut entry = lease_json(row, now);
                    entry["action"] = Value::from(swept.action(row, apply, now));
                    entry
                })
                .collect(),
        ),
    );
    report.insert("destroyed".into(), json!(swept.destroyed.len()));
    report.insert("kept".into(), json!(swept.kept));
    report.insert("failures".into(), json!(swept.failures));
    report.insert("status".into(), Value::from("reaped"));
    Ok(report)
}

mod ops;
use ops::{destroy_row, open_account, reap_rows, row_for, scratch_ssh, settle};
pub use ops::{host_leases, lease_json, probe_state, Entered, Swept};

/// A read of one lease's timestamps against a moment in time.
pub fn expired_at(row: &HostLease, now: DateTime<Utc>) -> bool {
    row.lease.as_ref().is_none_or(|held| held.expired(now))
}
