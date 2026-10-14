//! The cleaner entry point (Python `_run_hf`): the keep-or-evict decision
//! over the inventory, the reclamation loop, and the report it writes as it
//! goes.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::providers::local::disk_cleanup::hf::inventory::locks::scan_lock_state;
use crate::providers::local::disk_cleanup::hf::inventory::scan_cache;
use crate::providers::local::disk_cleanup::hf::reclaim::barrier::recover_lock_barrier;
use crate::providers::local::disk_cleanup::hf::reclaim::exchange::{
    enter_lock_barrier, leave_lock_barrier,
};
use crate::providers::local::disk_cleanup::hf::reclaim::recheck::{
    recheck_ref, recheck_repository_snapshots,
};
use crate::providers::local::disk_cleanup::hf::reclaim::unlink::execute_candidate;
use crate::providers::local::disk_cleanup::hf::{
    check_info, identity, identity_from_metadata, os_error, Identity, RepoScan,
};
use crate::providers::local::disk_cleanup::{
    fixed_root, free_bytes, safefs, CleanupReport, JanitorError, ScanCount,
};

/// The hub cache, relative to `$HOME`.
fn hub_parts() -> [OsString; 3] {
    [
        OsString::from(".cache"),
        OsString::from("huggingface"),
        OsString::from("hub"),
    ]
}

/// Run one HF eviction pass over every snapshot the hub can fetch again.
/// Scan-phase failures land in the report as skips or bounded errors; only
/// failures Python lets ESCAPE `_run_hf` (a vanished cache root mid-pass, a
/// failed free-space probe) are returned as `Err`.
pub fn run_hf(
    home: &Path,
    enforcing: bool,
    active_job_count: i64,
    report: &mut CleanupReport,
) -> Result<(), JanitorError> {
    if active_job_count > 0 {
        report.skip_hf("active_jobs", 1);
        return Ok(());
    }

    let mut budget = ScanCount;
    let scan_phase = (|budget: &mut ScanCount, report: &mut CleanupReport| {
        let Some(root) = fixed_root(home, &hub_parts(), false)? else {
            report.skip_hf("root_absent", 1);
            return Ok(None);
        };
        let root_fd = safefs::open_dir_path(&root)?;
        let mut root_info = safefs::fstat(root_fd.as_raw_fd())?;
        check_info(&root_info, &root_info)?;
        let path_info = std::fs::metadata(&root).map_err(JanitorError::from)?;
        if identity(&root_info) != identity_from_metadata(&path_info) {
            return Err(os_error("cache root changed while opening"));
        }
        if enforcing {
            recover_lock_barrier(root_fd.as_raw_fd(), &root_info)?;
            root_info = safefs::fstat(root_fd.as_raw_fd())?;
            let path_info = std::fs::metadata(&root).map_err(JanitorError::from)?;
            if identity(&root_info) != identity_from_metadata(&path_info) {
                return Err(os_error("cache root changed during lock barrier recovery"));
            }
        }
        let (lock_state_map, lock_fds, locks_present) = match scan_lock_state(
            root_fd.as_raw_fd(),
            &root_info,
            budget,
            report,
            true,
            ".locks",
            &BTreeSet::new(),
        ) {
            Ok(value) => value,
            Err(exc) if exc.code == "BlockingIOError" => {
                report.skip_hf("cache_locked", 1);
                return Ok(None);
            }
            Err(exc) => return Err(exc),
        };
        let scans = scan_cache(root_fd.as_raw_fd(), &root_info, budget, report)?;
        Ok(Some((
            root_fd,
            root_info,
            lock_state_map,
            lock_fds,
            locks_present,
            scans,
        )))
    })(&mut budget, report);

    let (root_fd, mut root_info, lock_state_map, lock_fds, locks_present, scans) = match scan_phase
    {
        Ok(Some(value)) => value,
        Ok(None) => return Ok(()),
        Err(exc) => {
            report.add_error("huggingface_cache", &exc);
            return Ok(());
        }
    };
    // root_fd / lock_fds are RAII guards: closing them (Python's
    // try/finally os.close) happens when this pass returns.

    let mut candidates: Vec<(usize, usize)> = scans
        .iter()
        .enumerate()
        .flat_map(|(scan_index, scan)| {
            (0..scan.candidates.len()).map(move |candidate_index| (scan_index, candidate_index))
        })
        .collect();
    report.hf.eligible_items = candidates.len() as i64;
    candidates.sort_by(|a, b| {
        let ca = &scans[a.0].candidates[a.1];
        let cb = &scans[b.0].candidates[b.1];
        ca.modified
            .partial_cmp(&cb.modified)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ca.repo.cmp(&cb.repo))
            .then_with(|| ca.commit.cmp(&cb.commit))
    });

    let mut scans = scans;
    for (scan_index, candidate_index) in candidates {
        report.hf.expected_bytes += scans[scan_index].candidates[candidate_index].expected;
        if !enforcing {
            continue;
        }
        if !locks_present {
            report.skip_hf("lock_root_absent", 1);
            break;
        }
        // Python `_fixed_root(..., required=True)` and `.stat()` raise
        // straight out of _run_hf here.
        let current_root =
            fixed_root(home, &hub_parts(), true)?.expect("required=true never yields None");
        let current_stat = std::fs::metadata(&current_root).map_err(JanitorError::from)?;
        if identity_from_metadata(&current_stat) != identity(&root_info) {
            report.skip_hf("root_changed", 1);
            break;
        }
        let recheck =
            (|scans: &mut Vec<RepoScan>, budget: &mut ScanCount, report: &mut CleanupReport| {
                recheck_repository_snapshots(
                    root_fd.as_raw_fd(),
                    &root_info,
                    &scans[scan_index],
                    budget,
                    report,
                )?;
                let refs = scans[scan_index].candidates[candidate_index].refs.clone();
                let commit = scans[scan_index].candidates[candidate_index]
                    .commit
                    .to_string_lossy()
                    .into_owned();
                for (path_parts, ident) in &refs {
                    budget.tick(report)?;
                    recheck_ref(root_fd.as_raw_fd(), path_parts, ident, &commit)?;
                }
                let (current_locks, temporary_fds, current_locks_present) = scan_lock_state(
                    root_fd.as_raw_fd(),
                    &root_info,
                    budget,
                    report,
                    false,
                    ".locks",
                    &BTreeSet::new(),
                )?;
                drop(temporary_fds);
                if !current_locks_present || current_locks != lock_state_map {
                    return Err(os_error("cache lock set changed"));
                }
                let known: BTreeSet<Identity> = lock_state_map.values().copied().collect();
                for descriptor in &lock_fds {
                    if !known.contains(&identity(&safefs::fstat(descriptor.as_raw_fd())?)) {
                        return Err(os_error("held cache lock changed"));
                    }
                }
                Ok(())
            })(&mut scans, &mut budget, report);
        if let Err(exc) = recheck {
            report.add_error("huggingface_recheck", &exc);
            break;
        }
        let before = free_bytes(home)?;
        let delete_result =
            (|scans: &mut Vec<RepoScan>, budget: &mut ScanCount, report: &mut CleanupReport| {
                let barrier_identities = enter_lock_barrier(
                    root_fd.as_raw_fd(),
                    &root_info,
                    &lock_state_map,
                    &lock_fds,
                    budget,
                    report,
                )?;
                let exec_outcome = execute_candidate(
                    root_fd.as_raw_fd(),
                    &scans[scan_index].candidates[candidate_index],
                );
                let leave_outcome =
                    leave_lock_barrier(root_fd.as_raw_fd(), &root_info, barrier_identities);
                // Python `finally:` semantics: a leave error replaces an exec
                // error; an exec error propagates through a clean leave.
                match (exec_outcome, leave_outcome) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(exec), Ok(())) => Err(exec),
                    (_, Err(leave)) => Err(leave),
                }
            })(&mut scans, &mut budget, report);
        if let Err(exc) = delete_result {
            report.add_error("huggingface_delete", &exc);
            break;
        }
        scans[scan_index].candidates[candidate_index].deleted = true;
        // Python `root_info = os.fstat(root_fd)`: the barrier dance
        // replaced direct children of the hub root (`.locks` exchange +
        // barrier rmdir), so the cached root identity is stale without
        // this refresh.
        root_info = safefs::fstat(root_fd.as_raw_fd())?;
        let after = free_bytes(home)?;
        let actual = (after - before).max(0);
        report.hf.deleted_items += 1;
        report.hf.actual_free_delta_bytes += actual;
    }
    Ok(())
}
