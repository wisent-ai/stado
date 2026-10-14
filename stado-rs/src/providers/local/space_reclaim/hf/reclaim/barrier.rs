//! The lock-barrier itself: building, discarding and recovering the private
//! hard-linked copy of the `.locks` namespace (Python `_hf_*lock_barrier*`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode};

use crate::providers::local::disk_cleanup::hf::{
    check_info, os_error, stable_identity, Identity, Parts, StableId, HF_BARRIER_MARKER,
    HF_BARRIER_NAME,
};
use crate::providers::local::disk_cleanup::{ifmt, safefs, JanitorError, IFDIR, IFREG};

/// Python `_hf_has_barrier_marker`.
fn has_barrier_marker(directory_fd: RawFd, root_info: &FileStat) -> Result<bool, JanitorError> {
    let info = match safefs::fstatat_nofollow(directory_fd, OsStr::new(HF_BARRIER_MARKER)) {
        Ok(info) => info,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(exc) => return Err(exc.into()),
    };
    check_info(&info, root_info)?;
    if ifmt(info.st_mode as u32) != IFREG {
        return Err(os_error("unsafe cache lock barrier marker"));
    }
    Ok(true)
}

/// Python `_hf_remove_barrier_tree`: delete the barrier copy of the lock
/// namespace. Every entry is re-validated (owner, device, plain dir/file)
/// before it is unlinked.
fn remove_barrier_tree(directory_fd: RawFd, root_info: &FileStat) -> Result<(), JanitorError> {
    safefs::fchmod(directory_fd, Mode::from_bits_truncate(0o700))?;
    let mut names = Vec::new();
    {
        let entries = safefs::DirEntries::open(directory_fd)?;
        for name in entries {
            names.push(name?);
        }
    }
    for name in names {
        let info = safefs::fstatat_nofollow(directory_fd, &name)?;
        check_info(&info, root_info)?;
        let kind = ifmt(info.st_mode as u32);
        if kind == IFDIR {
            let child = safefs::open_dir_at(directory_fd, &name)?;
            if stable_identity(&safefs::fstat(child.as_raw_fd())?) != stable_identity(&info) {
                return Err(os_error("cache lock barrier directory changed"));
            }
            remove_barrier_tree(child.as_raw_fd(), root_info)?;
            drop(child);
            safefs::rmdir_at(directory_fd, &name)?;
        } else if kind == IFREG {
            safefs::unlink_at(directory_fd, &name)?;
        } else {
            return Err(os_error("unsafe cache lock barrier entry"));
        }
    }
    Ok(())
}

/// Python `_hf_discard_barrier`.
pub(super) fn discard_barrier(root_fd: RawFd, root_info: &FileStat) -> Result<(), JanitorError> {
    let barrier_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    let result = remove_barrier_tree(barrier_fd.as_raw_fd(), root_info);
    drop(barrier_fd);
    result?;
    safefs::rmdir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    Ok(())
}

/// Restore an atomic exchange interrupted by process termination.
/// Python `_hf_recover_lock_barrier`.
pub fn recover_lock_barrier(root_fd: RawFd, root_info: &FileStat) -> Result<(), JanitorError> {
    let barrier_fd = match safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME)) {
        Ok(fd) => fd,
        Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(exc) => return Err(exc.into()),
    };
    let private_marked = {
        check_info(&safefs::fstat(barrier_fd.as_raw_fd())?, root_info)
            .and_then(|()| has_barrier_marker(barrier_fd.as_raw_fd(), root_info))
    };
    drop(barrier_fd);
    let private_marked = private_marked?;
    let locks_fd = safefs::open_dir_at(root_fd, OsStr::new(".locks"))?;
    let canonical_marked = {
        check_info(&safefs::fstat(locks_fd.as_raw_fd())?, root_info)
            .and_then(|()| has_barrier_marker(locks_fd.as_raw_fd(), root_info))
    };
    drop(locks_fd);
    let canonical_marked = canonical_marked?;
    if canonical_marked && !private_marked {
        safefs::rename_exchange(root_fd, OsStr::new(".locks"), OsStr::new(HF_BARRIER_NAME))?;
        discard_barrier(root_fd, root_info)?;
    } else if private_marked && !canonical_marked {
        discard_barrier(root_fd, root_info)?;
    } else {
        return Err(os_error("ambiguous cache lock barrier residue"));
    }
    Ok(())
}

/// Python `_hf_prepare_lock_barrier`: build a private hard-linked copy of
/// the whole `.locks` namespace so the atomic exchange never destroys a
/// lock another process holds.
pub(super) fn prepare_lock_barrier(
    root_fd: RawFd,
    root_info: &FileStat,
    lock_state: &BTreeMap<Parts, Identity>,
) -> Result<StableId, JanitorError> {
    safefs::mkdir_at(
        root_fd,
        OsStr::new(HF_BARRIER_NAME),
        Mode::from_bits_truncate(0o700),
    )?;
    let barrier_fd = safefs::open_dir_at(root_fd, OsStr::new(HF_BARRIER_NAME))?;
    let result = (|barrier_fd: &OwnedFd| {
        let marker = safefs::open_file_at(
            barrier_fd.as_raw_fd(),
            OsStr::new(HF_BARRIER_MARKER),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::from_bits_truncate(0o400),
        )?;
        drop(marker);
        let mut directories: Vec<&Parts> = lock_state
            .iter()
            .filter(|(parts, ident)| !parts.is_empty() && ident.ifmt == IFDIR)
            .map(|(parts, _)| parts)
            .collect();
        directories.sort_by_key(|parts| parts.len());
        for parts in &directories {
            let parent = safefs::open_path(barrier_fd.as_raw_fd(), &parts[..parts.len() - 1])?;
            safefs::mkdir_at(
                parent.as_raw_fd(),
                &parts[parts.len() - 1],
                Mode::from_bits_truncate(0o700),
            )?;
        }
        for (parts, ident) in lock_state {
            if parts.is_empty() || ident.ifmt != IFREG {
                continue;
            }
            let mut source_parts: Parts = vec![OsString::from(".locks")];
            source_parts.extend_from_slice(&parts[..parts.len() - 1]);
            let source_parent = safefs::open_path(root_fd, &source_parts)?;
            let destination_parent =
                safefs::open_path(barrier_fd.as_raw_fd(), &parts[..parts.len() - 1])?;
            let expected_parent = &lock_state[&parts[..parts.len() - 1].to_vec()];
            if stable_identity(&safefs::fstat(source_parent.as_raw_fd())?)
                != expected_parent.stable()
            {
                return Err(os_error("cache lock parent changed while building barrier"));
            }
            safefs::link_at(
                source_parent.as_raw_fd(),
                destination_parent.as_raw_fd(),
                &parts[parts.len() - 1],
            )?;
            let linked =
                safefs::fstatat_nofollow(destination_parent.as_raw_fd(), &parts[parts.len() - 1])?;
            if stable_identity(&linked) != ident.stable() {
                return Err(os_error("cache lock changed while building barrier"));
            }
        }
        for parts in directories.iter().rev() {
            let descriptor = safefs::open_path(barrier_fd.as_raw_fd(), parts)?;
            safefs::fchmod(descriptor.as_raw_fd(), Mode::from_bits_truncate(0o555))?;
        }
        safefs::fchmod(barrier_fd.as_raw_fd(), Mode::from_bits_truncate(0o555))?;
        Ok(stable_identity(&safefs::fstat(barrier_fd.as_raw_fd())?))
    })(&barrier_fd);
    drop(barrier_fd);
    match result {
        Ok(identity) => Ok(identity),
        Err(exc) => {
            discard_barrier(root_fd, root_info)?;
            Err(exc)
        }
    }
}
