//! The atomic rename-exchange dance around one deletion (Python
//! `_hf_enter_lock_barrier` / `_hf_leave_lock_barrier` / `_hf_exchange`).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, RawFd};

use nix::sys::stat::FileStat;

use crate::providers::local::disk_cleanup::hf::inventory::locks::scan_lock_state;
use crate::providers::local::disk_cleanup::hf::reclaim::barrier::{
    discard_barrier, prepare_lock_barrier,
};
use crate::providers::local::disk_cleanup::hf::{
    os_error, stable_identity, Identity, Parts, StableId, HF_BARRIER_NAME,
};
use crate::providers::local::disk_cleanup::{
    safefs, CleanupReport, JanitorError, ScanBudget, IFREG,
};

/// Python `_hf_barrier_lock_state_matches`: after the exchange the private
/// namespace must equal the recorded state except for the +1 link count
/// the barrier hard links added to every regular file.
fn barrier_lock_state_matches(
    expected: &BTreeMap<Parts, Identity>,
    current: &BTreeMap<Parts, Identity>,
) -> bool {
    if expected.len() != current.len() || !expected.keys().zip(current.keys()).all(|(a, b)| a == b)
    {
        return false;
    }
    for (path, ident) in expected {
        let observed = &current[path];
        if ident.ifmt == IFREG {
            let matches = observed.dev == ident.dev
                && observed.ino == ident.ino
                && observed.ifmt == ident.ifmt
                && observed.size == ident.size
                && observed.mtime_ns == ident.mtime_ns
                && observed.nlink == ident.nlink + 1;
            if !matches {
                return false;
            }
        } else if observed != ident {
            return false;
        }
    }
    true
}

/// Python `_hf_enter_lock_barrier`. Returns (original, barrier) stable
/// identities used by [`leave_lock_barrier`].
pub(in crate::providers::local::disk_cleanup::hf) fn enter_lock_barrier(
    root_fd: RawFd,
    root_info: &FileStat,
    lock_state: &BTreeMap<Parts, Identity>,
    lock_fds: &[File],
    budget: &mut ScanBudget,
    report: &mut CleanupReport,
) -> Result<(StableId, StableId), JanitorError> {
    let root_key: Parts = Vec::new();
    let original = lock_state[&root_key].stable();
    let barrier = prepare_lock_barrier(root_fd, root_info, lock_state)?;
    let mut exchanged = false;
    let result = (|exchanged: &mut bool| {
        safefs::rename_exchange(root_fd, OsStr::new(".locks"), OsStr::new(HF_BARRIER_NAME))?;
        *exchanged = true;
        let canonical_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
        let private_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
        {
            if stable_identity(&safefs::fstat(canonical_fd.as_raw_fd())?) != barrier {
                return Err(os_error("cache lock barrier exchange changed"));
            }
            if stable_identity(&safefs::fstat(private_fd.as_raw_fd())?) != original {
                return Err(os_error("cache lock namespace changed during exchange"));
            }
        }
        drop(canonical_fd);
        drop(private_fd);
        let held_identities: BTreeSet<StableId> = lock_fds
            .iter()
            .map(|fd| safefs::fstat(fd.as_raw_fd()).map(|i| stable_identity(&i)))
            .collect::<io::Result<BTreeSet<_>>>()?;
        let expected_held: BTreeSet<StableId> = lock_state
            .values()
            .filter(|ident| ident.ifmt == IFREG)
            .map(Identity::stable)
            .collect();
        let (current, new_fds, present) = scan_lock_state(
            root_fd,
            root_info,
            budget,
            report,
            true,
            HF_BARRIER_NAME,
            &held_identities,
        )?;
        let check = (|current: &BTreeMap<Parts, Identity>| {
            if !present || !barrier_lock_state_matches(lock_state, current) {
                return Err(os_error("cache lock set changed during barrier exchange"));
            }
            if held_identities != expected_held {
                return Err(os_error("held cache lock changed during barrier exchange"));
            }
            Ok(())
        })(&current);
        drop(new_fds);
        check?;
        Ok((original, barrier))
    })(&mut exchanged);
    match result {
        Ok(identities) => Ok(identities),
        Err(exc) => {
            if exchanged {
                safefs::rename_exchange(
                    root_fd,
                    OsStr::new(".locks"),
                    OsStr::new(HF_BARRIER_NAME),
                )?;
            }
            discard_barrier(root_fd, root_info)?;
            Err(exc)
        }
    }
}

/// Python `_hf_leave_lock_barrier`: exchange back, verify restoration,
/// discard the private namespace.
pub(in crate::providers::local::disk_cleanup::hf) fn leave_lock_barrier(
    root_fd: RawFd,
    root_info: &FileStat,
    identities: (StableId, StableId),
) -> Result<(), JanitorError> {
    let (original, barrier) = identities;
    let canonical_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
    let private_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    {
        if stable_identity(&safefs::fstat(canonical_fd.as_raw_fd())?) != barrier {
            return Err(os_error("cache lock barrier changed before restoration"));
        }
        if stable_identity(&safefs::fstat(private_fd.as_raw_fd())?) != original {
            return Err(os_error(
                "held cache lock namespace changed before restoration",
            ));
        }
    }
    drop(canonical_fd);
    drop(private_fd);
    safefs::rename_exchange(root_fd, OsStr::new(".locks"), OsStr::new(HF_BARRIER_NAME))?;
    let restored_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
    {
        if stable_identity(&safefs::fstat(restored_fd.as_raw_fd())?) != original {
            return Err(os_error("cache lock namespace restoration failed"));
        }
    }
    drop(restored_fd);
    discard_barrier(root_fd, root_info)?;
    Ok(())
}
