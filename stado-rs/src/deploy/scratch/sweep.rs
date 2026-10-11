//! The leases a host holds, read back from the host: listed, one destroyed
//! by name, or every expired one swept.

use chrono::Utc;
use serde_json::{json, Map, Value};

use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::lease;
use super::{destroy_row, host_leases, lease_json, reap_rows};

/// Every lease a host holds, each with the account's real presence.
pub async fn list(target_name: &str, runner: &Runner) -> Result<Map<String, Value>, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let home = host_channel::remote_home(&target, runner).await?;
    let (held, read_code) = host_leases(&target, &home, runner).await?;
    let now = Utc::now();
    let mut report = host_channel::base_report(&target);
    report.insert(
        "leases".into(),
        Value::Array(held.iter().map(|row| lease_json(row, now)).collect()),
    );
    report.insert("exit_code".into(), Value::from(read_code));
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
    let (held, _read_code) = host_leases(&target, &home, runner).await?;
    let Some(row) = held.iter().find(|row| row.name == name) else {
        return Err(DeployError(format!(
            "no scratch lease named '{name}' on '{}'",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
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
    let (held, read_code) = host_leases(target, &home, runner).await?;
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
    report.insert("exit_code".into(), Value::from(read_code));
    report.insert("status".into(), Value::from("reaped"));
    Ok(report)
}
