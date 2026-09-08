//! Giving a lease back: delete the account, forget the record, and prove it.

use chrono::Utc;
use serde_json::{json, Map, Value};

use super::probe_state;
use crate::deploy::scratch::lease::{self, ScratchLease};
use crate::deploy::scratch::registry_out;
use crate::deploy::scratch::remote::{self, HostLease};
use crate::deploy::{host_channel, host_user_delete, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The row a just-created lease would have had, for the rollback path.
pub fn row_for(record: &ScratchLease) -> HostLease {
    HostLease {
        name: record.name.clone(),
        account_present: true,
        home_path: None,
        lease: Some(record.clone()),
        unreadable: None,
    }
}

/// Destroy one lease and prove the host is clear of it: the account, its home,
/// and the record. The verdict comes from a probe, not from the delete
/// command's own word — an account `sysadminctl` said it removed and `id` still
/// answers for is exactly the disagreement worth catching.
pub async fn destroy_row(
    target: &ComputeTarget,
    home: &str,
    row: &HostLease,
    runner: &Runner,
) -> Result<Map<String, Value>, DeployError> {
    let username = row
        .lease
        .as_ref()
        .map_or_else(|| row.name.clone(), |held| held.username.clone());
    host_user_delete::validate_deletable(&username)?;
    let deleted = host_user_delete::delete_user(&username, target, false, runner).await;
    let record_path = lease::record_path(home, &row.name);
    let command = remote::forget_command(&record_path);
    let forgotten =
        host_channel::run_program(target, &["/bin/sh", "-c", command.as_str()], runner).await?;
    let state = probe_state(target, &username, &record_path, runner).await?;
    let root = registry_out::remove(&lease::local_root(&row.name))?;
    if !state.is_clear() {
        let said = deleted
            .error
            .clone()
            .unwrap_or_else(|| deleted.status.clone());
        return Err(DeployError(format!(
            "scratch '{}' is not gone from '{}': account {}, home {}, record {} \
             (the delete said {said}, the record removal exited {})",
            row.name, target.name, state.account, state.home, state.record, forgotten.code
        )));
    }
    let mut report = Map::new();
    report.insert("name".into(), Value::from(row.name.clone()));
    report.insert("target".into(), Value::from(target.name.clone()));
    report.insert("username".into(), Value::from(username));
    report.insert("account".into(), Value::from(state.account.clone()));
    report.insert("home".into(), Value::from(state.home.clone()));
    report.insert(
        "home_path".into(),
        row.home_path.clone().map_or(Value::Null, Value::from),
    );
    report.insert("record".into(), Value::from(state.record.clone()));
    report.insert("destroyed_at".into(), Value::from(lease::now_stamp()));
    report.insert("storage_root".into(), Value::from(root));
    report.insert("exit_code".into(), Value::from(forgotten.code));
    Ok(report)
}

/// What one sweep did.
#[derive(Debug, Clone, Default)]
pub struct Swept {
    pub destroyed: Vec<String>,
    pub kept: usize,
    pub failures: Vec<Value>,
    pub reports: Vec<Value>,
}

impl Swept {
    /// What happened to one row, or would have happened in a preview.
    pub fn action(&self, row: &HostLease, apply: bool, now: chrono::DateTime<Utc>) -> &'static str {
        if !crate::deploy::scratch::expired_at(row, now) {
            return "kept";
        }
        if self.destroyed.contains(&row.name) {
            return "destroyed";
        }
        if apply {
            "failed"
        } else {
            "would-destroy"
        }
    }
}

/// Destroy every expired row, or measure them when `apply` is false. One row's
/// failure never stops the sweep: a host with two leaks must lose both.
pub async fn reap_rows(
    target: &ComputeTarget,
    home: &str,
    rows: &[HostLease],
    apply: bool,
    runner: &Runner,
) -> Result<Swept, DeployError> {
    let now = Utc::now();
    let mut swept = Swept::default();
    for row in rows {
        if !crate::deploy::scratch::expired_at(row, now) {
            swept.kept += 1;
            continue;
        }
        if !apply {
            continue;
        }
        match destroy_row(target, home, row, runner).await {
            Ok(report) => {
                swept.destroyed.push(row.name.clone());
                swept.reports.push(Value::Object(report));
            }
            Err(exc) => swept
                .failures
                .push(json!({"name": row.name.clone(), "error": exc.0})),
        }
    }
    Ok(swept)
}
