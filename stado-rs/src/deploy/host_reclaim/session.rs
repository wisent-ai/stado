//! One reclamation, end to end: resolve the target, choose the keep-list
//! authority, run the program under its own timeout, and record on the host
//! whose disk changed what the run actually did.

use std::time::Duration;

use serde_json::{json, Value};

use crate::deploy::host_channel;
use crate::deploy::host_disk::gib_from_blocks;
use crate::deploy::{shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::declaration::DECLARATION_PATH;
use super::outcome::{parse_output, Reclamation};
use super::program::{remote_script, remote_script_with_stado};
use super::{AUDIT_LOG, DEFAULT_WORK_ROOTS};

/// A reclaim includes the registry janitor (whose declared pass may take up
/// to ten minutes) and removal of large, already-enumerated trees. The generic
/// two-minute host-read bound killed the transport mid-pass and left the
/// remote janitor running without a caller. This explicit operator command is
/// bounded independently at one hour.
const RECLAIM_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// The script that appends one audit record on the host whose disk changed.
///
/// The record is one line of JSON built by `serde_json` on this side and
/// spliced in as a single shell-quoted word, so operator prose cannot become
/// shell syntax and cannot break the line format either. The directory is
/// owner-only, the same way every other thing Stado keeps under `.stado` is.
const AUDIT_SCRIPT_TEMPLATE: &str = r#"set -u
umask 077
log="$HOME/@AUDIT_LOG@"
/bin/mkdir -p "$(/usr/bin/dirname "$log")" || exit 1
printf '%s\n' @RECORD@ >> "$log" || exit 1
printf 'STADO_RECLAIM_AUDITED\t%s\n' "$log"
"#;

/// Append the audit record for an applied reclamation, and return where it
/// landed on the host.
///
/// Separate from the reclamation script because the record states what the
/// reclamation actually did: the measurements have to exist before the record
/// can be true, and a record written up front would be a record of an
/// intention.
///
/// `actor` arrives from the caller rather than being read here, so that this
/// binary has ONE spelling of "who did this" — `cli/autonomy::actor`, the
/// same one `service ensure` stamps its own record with.
pub async fn record_audit(
    target: &ComputeTarget,
    reclamation: &Reclamation,
    reason: &str,
    actor: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let record = json!({
        "at": crate::models::isoformat_utc(chrono::Utc::now()),
        "host": target.name,
        "command": "stado space reclaim",
        "mode": reclamation.mode,
        "actor": actor,
        "reason": reason,
        "free_gb_before": reclamation.free_kb_before.map(|kb| gib_from_blocks(kb as f64)),
        "free_gb_after": reclamation.free_kb_after.map(|kb| gib_from_blocks(kb as f64)),
        "stages": reclamation
            .stages
            .iter()
            .map(|stage| json!({
                "stage": stage.stage,
                "items": stage.items,
                "paths": stage.paths,
                "refused": stage.refused,
                "free_gb_before": stage.free_kb_before.map(|kb| gib_from_blocks(kb as f64)),
                "free_gb_after": stage.free_kb_after.map(|kb| gib_from_blocks(kb as f64)),
                "local_terminality_evidence": stage.local_terminality_evidence,
            }))
            .collect::<Vec<Value>>(),
    });
    let script = AUDIT_SCRIPT_TEMPLATE
        .replace("@AUDIT_LOG@", AUDIT_LOG)
        .replace("@RECORD@", &shlex_quote(&record.to_string()));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the audit record could not be written",
        )));
    }
    for line in output.stdout.lines() {
        if let ["STADO_RECLAIM_AUDITED", path] = host_channel::marker_fields(line).as_slice() {
            return Ok((*path).to_string());
        }
    }
    Err(DeployError(
        "the host did not confirm the audit record".to_string(),
    ))
}

/// Run the reclamation on one canonical registry host.
///
/// Returns the resolved target alongside the reclamation because the caller
/// needs it for the report and for the audit record, and resolving it twice
/// would be two registry reads that could disagree.
pub async fn reclaim_host(
    target_name: &str,
    apply: bool,
    stages: &[String],
    runner: &Runner,
) -> Result<(ComputeTarget, Reclamation), DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let queue_selected = stages.iter().any(|stage| stage == "queue_workdirs");
    let mut unreadable = Vec::new();
    let live_jobs = if !queue_selected {
        Some(Vec::new())
    } else {
        match crate::queue::JobStorage::new().await {
            Ok(store) => {
                let mut ids = Vec::new();
                for state in ["queue", "running"] {
                    match store.list_jobs(state, 0).await {
                        Ok(jobs) => ids.extend(jobs.into_iter().map(|job| job.job_id)),
                        Err(error) => unreadable.push(format!("{state}/: {error}")),
                    }
                }
                if unreadable.is_empty() {
                    Some(ids)
                } else {
                    None
                }
            }
            Err(error) => {
                unreadable.push(format!("opening the queue store: {error}"));
                None
            }
        }
    };
    let target_free_gb = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.target_free_gb);
    let script = if host_channel::target_is_this_host(&target) {
        // A local reclaim must use the binary that owns this invocation.
        // Release capacity builds the corrected tree before installation;
        // selecting the older installed janitor would make that correction
        // unreachable.
        let current_stado = std::env::current_exe()
            .map_err(|error| DeployError(format!("cannot identify current Stado: {error}")))?
            .into_os_string()
            .into_string()
            .map_err(|_| DeployError("current Stado path is not valid UTF-8".to_string()))?;
        remote_script_with_stado(
            apply,
            stages,
            live_jobs.as_deref(),
            DEFAULT_WORK_ROOTS,
            target_free_gb,
            Some(&current_stado),
        )
    } else {
        remote_script(
            apply,
            stages,
            live_jobs.as_deref(),
            DEFAULT_WORK_ROOTS,
            target_free_gb,
        )
    };
    let output =
        host_channel::run_script_with_timeout(&target, &script, RECLAIM_TIMEOUT, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the reclamation did not run",
        )));
    }
    let mut reclamation = parse_output(&output.stdout, apply);
    if queue_selected && live_jobs.is_none() {
        reclamation.skipped.push((
            "queue_authority".to_string(),
            format!(
                "the queue store did not answer; queue_workdirs used conservative local \
                 terminality observations and kept every candidate without a full proof — {}",
                unreadable.join("; ")
            ),
        ));
    }
    if reclamation.stages.is_empty() {
        return Err(DeployError(format!(
            "{} declares no eligible space reclamation stage; add it to {} reclaim_stages",
            target.name, DECLARATION_PATH
        )));
    }
    Ok((target, reclamation))
}
