//! The worker's own passes of the lease reaper: at start over the jobs the
//! worker it replaced left running, and when idle over the jobs it finished.
//!
//! [`crate::queue::reaper`] completes a release job whose worker lease
//! expired from its verified receipt and archive, and it is CAS-fenced, so
//! any caller may run it. With the coordinator tick as its only caller, a
//! coordinator that ticks rarely (an old Stado on a memory-starved host)
//! leaves a job that finished every step and uploaded its receipt — its agent
//! restarted onto an installed Stado before writing the terminal state —
//! `building` for hours with its release unpublished.
//!
//! An idle agent therefore asks the reaper, on every idle tick, about the
//! jobs whose work trees on this host hold a receipt and which are still in
//! `running/`. Only those: the fleet-wide pass lists every running job
//! through the object API and did not finish in five minutes from this host.
//! A job seen gone from `running/` is settled for this process and not asked
//! about again; a job whose lease is still promised is left alone by the
//! reaper itself and asked about again next idle tick.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use crate::providers::local::disk_cleanup::queue_workdirs::{work_root, WORKDIR_PREFIX};
use crate::queue::JobStorage;

use super::super::agent_log;

/// Jobs with a receipt here that this process has seen leave `running/`.
static SETTLED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Job ids of this host's work trees that hold an `output/receipt.json` and
/// are not yet known to have left `running/`. A work root not created yet
/// holds none; one that cannot be listed is the error, since read as empty
/// every finished job on it stayed in `running/` with nothing said. An entry
/// of the root that cannot be read is logged with the cause and the rest are
/// still reaped; dropped, its job stayed in `running/` the same way.
fn finished_here(log_fn: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    let root = work_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list {}: {error}", root.display())),
    };
    let settled = SETTLED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut finished = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log_fn(&format!(
                    "lease reaper: cannot read an entry of {}: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(job_id) = name.strip_prefix(WORKDIR_PREFIX) else {
            continue;
        };
        if entry.path().join("output/receipt.json").is_file() && !settled.contains(job_id) {
            finished.push(job_id.to_string());
        }
    }
    Ok(finished)
}

/// Run one reaper pass over this host's finished jobs that are still in
/// `running/`, when the agent holds no slot. A failed pass is logged and
/// left to the next idle tick; it never fails the tick.
pub(super) async fn reap_when_idle(store: &JobStorage, idle: bool, log_fn: &mut dyn FnMut(&str)) {
    if !idle {
        return;
    }
    let mut jobs = Vec::new();
    let finished = match finished_here(log_fn) {
        Ok(finished) => finished,
        Err(error) => {
            log_fn(&format!("lease reaper: {error}"));
            return;
        }
    };
    for job_id in finished {
        match store
            .backend()
            .exists(&format!("running/{job_id}.json"))
            .await
        {
            Ok(true) => jobs.push(job_id),
            Ok(false) => {
                SETTLED
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(job_id);
            }
            Err(error) => log_fn(&format!(
                "lease reaper: cannot tell whether {job_id} is still running: {error}"
            )),
        }
    }
    if jobs.is_empty() {
        return;
    }
    let log = |message: &str| agent_log(&format!("lease reaper: {message}"));
    match crate::queue::reaper::reap_named(store, &jobs, &log).await {
        Ok(summary) => {
            if summary.release_completions + summary.requeued + summary.failed + summary.unreadable
                > 0
            {
                log_fn(&format!(
                    "lease reaper: of {} finished job(s) here, completed {} release job(s) from \
                     their receipts, requeued {}, failed {}, could not read {}",
                    jobs.len(),
                    summary.release_completions,
                    summary.requeued,
                    summary.failed,
                    summary.unreadable
                ));
            }
        }
        Err(error) => log_fn(&format!("lease reaper: pass failed: {error}")),
    }
}

/// At start this worker runs nothing, so every running job whose record
/// names this host's worker was left by the one it replaced. A job no
/// process here still runs in its work directory is settled now as a lapsed
/// lease would be, instead of holding its schedule until the dead worker's
/// promise passes; one a process still runs keeps its lease. A failed read
/// or settlement is logged with its cause and the agent starts anyway.
pub(super) async fn release_left_behind(
    store: &JobStorage,
    hostname: &str,
    log_fn: &mut dyn FnMut(&str),
) {
    let still_running = |job_id: &str| -> Result<Vec<String>, String> {
        let directory = crate::providers::local::disk_cleanup::queue_workdirs::work_dir(job_id)
            .map_err(|error| format!("its work directory cannot be named: {error}"))?;
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let output = crate::wait::output(
            std::process::Command::new("lsof")
                .arg("-t")
                .arg("--")
                .arg(&directory),
        )
        .map_err(|error| format!("lsof could not run: {error}"))?;
        let pids: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(|pid| format!("pid {pid}"))
            .collect();
        if pids.is_empty() && !output.stderr.is_empty() {
            return Err(format!(
                "lsof on {} said: {}",
                directory.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(pids)
    };
    match crate::queue::reaper::release_left_behind(
        store,
        &format!("local@{hostname}"),
        &still_running,
    )
    .await
    {
        Ok(lines) => {
            for line in lines {
                log_fn(&format!("init: {line}"));
            }
        }
        Err(error) => log_fn(&format!(
            "init: the running jobs the replaced worker left could not be read or settled: {error}"
        )),
    }
}
