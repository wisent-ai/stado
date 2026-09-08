mod phases;
mod restart;
mod target;
mod verify;

use serde_json::{json, Map, Value};

use super::{
    activate_script, plan, probe_script, recheck_staged_script, ReleasePlan, StagedRelease, MARKER,
};
use crate::deploy::{host_channel, service, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

pub use target::{activate_staged_target, release_target};

// ---------------------------------------------------------------------------
// Reading the host back
// ---------------------------------------------------------------------------

/// The `STADO_RELEASE` markers of one program's stdout, in order.
///
/// Matched with a slice pattern the way
/// [`crate::deploy::service::parse_markers`] matches its own, so a marker
/// with the wrong arity is ignored rather than mis-read, and a chatty login
/// shell contributes nothing.
pub fn markers(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            [MARKER, key, value] => Some(((*key).to_string(), (*value).to_string())),
            _ => None,
        })
        .collect()
}

/// One marker's value, or the empty string.
pub fn marker<'a>(markers: &'a [(String, String)], key: &str) -> &'a str {
    markers
        .iter()
        .find(|(name, _)| name == key)
        .map_or("", |(_, value)| value.as_str())
}

/// Every value one repeated marker carried, in the order the host emitted
/// them. A tree probe names one path per marker, and a path list is exactly
/// the kind of value that must not be flattened into one field: `say` caps
/// each field at 200 characters, so a joined list would be a truncated list.
pub fn marker_values<'a>(markers: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    markers
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect()
}

/// What went wrong in a program that did not finish.
///
/// The remote program's stderr is its diagnostic channel. Keep its last line
/// whenever one exists; a final progress marker such as `status failed` says
/// only that the phase failed and must not replace the captured cause. The
/// last marker remains the fallback for scripts that reported no stderr, and
/// the transport fallback covers commands that produced neither.
pub(super) fn step_failure(markers: &[(String, String)], output: &CommandOutput) -> String {
    if !output.stderr.trim().is_empty() {
        return host_channel::last_error_line(output, "ssh failed");
    }
    match markers.last() {
        Some((key, value)) => format!("{key} {value}"),
        None => host_channel::last_error_line(output, "ssh failed"),
    }
}

/// One executed phase, as the report carries it.
fn step_entry(step: &str, state: &str, detail: Option<String>) -> Value {
    let mut entry = Map::new();
    entry.insert("step".to_string(), json!(step));
    entry.insert("state".to_string(), json!(state));
    if let Some(detail) = detail {
        entry.insert("detail".to_string(), json!(detail));
    }
    Value::Object(entry)
}

/// Close a report as a failure at one phase, leaving everything the earlier
/// phases established in place.
fn fail(report: &mut Map<String, Value>, exit_code: i32, error: String) -> Value {
    report.insert("exit_code".to_string(), json!(exit_code));
    report.insert(
        "status".to_string(),
        json!(host_channel::FAILED_STATUS.to_string()),
    );
    report.insert("error".to_string(), json!(error));
    Value::Object(std::mem::take(report))
}

/// Recheck and atomically switch only the ordinary active program path.
///
/// Storage authority recovery must install its forward unit definition before
/// the API starts, so it performs unit restoration itself after this handoff.
/// The returned digest is the mapped active file and must equal the digest
/// persisted by [`stage_declared_release`].
pub async fn activate_staged_program(
    target: &ComputeTarget,
    staged: &StagedRelease,
    runner: &Runner,
) -> Result<String, DeployError> {
    let plan = plan(target, &staged.request, staged.self_store)?;
    let recheck = host_channel::run_script(target, &recheck_staged_script(&plan)?, runner).await?;
    let recheck_markers = markers(&recheck.stdout);
    if !recheck.ok() || marker(&recheck_markers, "step") != "stage" {
        return Err(DeployError(step_failure(&recheck_markers, &recheck)));
    }
    if marker(&recheck_markers, "staged_sha256") != staged.staged_sha256 {
        return Err(DeployError(
            "staged runtime digest changed before activation".to_string(),
        ));
    }
    let probe = host_channel::run_script(target, &probe_script(&plan), runner).await?;
    let probe_markers = markers(&probe.stdout);
    if probe.ok()
        && marker(&probe_markers, "step") == "probe"
        && marker(&probe_markers, "active_sha256") == staged.staged_sha256
    {
        return Ok(staged.staged_sha256.clone());
    }
    let activated = host_channel::run_script(target, &activate_script(&plan), runner).await?;
    let activate_markers = markers(&activated.stdout);
    if !activated.ok() || marker(&activate_markers, "step") != "activate" {
        return Err(DeployError(step_failure(&activate_markers, &activated)));
    }
    let active = marker(&activate_markers, "active_sha256").to_string();
    if active != staged.staged_sha256 {
        return Err(DeployError(
            "active runtime does not map the staged release digest".to_string(),
        ));
    }
    Ok(active)
}

/// The report a delivery opens with: the plan as declared, before the host
/// has said anything about itself.
fn base_release_report(
    target: &ComputeTarget,
    plan: &ReleasePlan,
    units: &[service::ManagedService],
) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert("source_commit".to_string(), json!(plan.source_commit));
    report.insert("binary".to_string(), json!(plan.product.name));
    report.insert("version".to_string(), json!(plan.version));
    report.insert("platform".to_string(), json!(plan.platform));
    report.insert("declared_version".to_string(), json!(plan.declared_version));
    report.insert("release_uri".to_string(), json!(plan.release_uri()));
    report.insert("sha256".to_string(), json!(plan.sha256));
    // The address the TARGET was told to fetch from. Its absence cost an hour
    // on 2026-09-03: `fetch no_declared_size` says the answer carried no
    // `Content-Range`, and nothing in the receipt said which origin had
    // answered, so the one fact that separates "the store cannot serve ranges"
    // from "the target reached the wrong server" was not in the report.
    report.insert("release_api".to_string(), json!(plan.release_api));
    report.insert("staged_path".to_string(), json!(plan.staged_path()));
    report.insert("active_path".to_string(), json!(plan.active_path()));
    report.insert("install_root".to_string(), json!(plan.product.root()));
    report.insert("preserved_paths".to_string(), json!(plan.preserved_paths()));
    report.insert("dry_run".to_string(), json!(plan.dry_run));
    report.insert(
        "units".to_string(),
        json!(units
            .iter()
            .map(service::ManagedService::unit_id)
            .collect::<Vec<_>>()),
    );
    report
}
