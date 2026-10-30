//! The package managers' download caches: the janitor's second tier at the
//! disk-full rule.
//!
//! Every cleaner before this one takes agent output, what the next build
//! makes again from what is on the disk. When a pass has taken all of it
//! and the volume cannot take the next build without reaching the rule
//! again, the next thing that comes back without its owner's say is what a
//! package manager downloaded and fetches again: Cargo's git checkouts,
//! npm's, Bun's, SwiftPM's, pip's, uv's and Homebrew's caches. They cost the
//! network and time on the next build, not anything only the owner has, so
//! they go before any proposal of the owner's own files; and they go only
//! then, largest first, and only until the volume has the room under the
//! rule a build of this host takes ([`super::margin`]).
//!
//! A cache a running process holds stays (`held_by_process`): a file in it
//! open, a process working in it or naming it, read from `lsof` and `ps`
//! ([`super::holders`]) right before it goes, and then it is moved aside in
//! one rename, so a tool that starts while the tree is removed makes a new
//! cache instead of reading a half-removed one. A tree moved aside that a
//! pass could not finish removing is taken by the next pass. Cargo's git
//! checkouts stay while any `cargo` or `rustc` runs (`build_running`): a
//! build reads them crate by crate without holding them open between two
//! compiler runs. They also stay while Cargo holds its package-cache lock
//! (`$CARGO_HOME/.package-cache`). When a table cannot be read every cache
//! stays (`process_table_unread`). Cargo's registry stays: build scripts
//! keep absolute paths into it for a whole build. Each removal is a line of
//! the janitor's removal record.

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use super::candidates::ledger;
use super::holders::Holders;
use super::janitor::policy::ownership;
use super::janitor::state::report::{met::Met, CleanupReport};
use super::margin::Margin;
use super::rule::read_fleet_volume;
use super::weles::{dir_size, remove_tree};
use super::JanitorError;

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "package_caches";

/// One cache: where it is and which tool keeps it.
pub struct PackageCache {
    /// Home-relative path, or `None` for Cargo's git checkouts, which live
    /// under Cargo's home.
    pub root: Option<&'static str>,
    pub tool: &'static str,
}

/// Every cache this cleaner takes. A product's own application cache
/// (`~/Library/Caches/<bundle id>` of an app the owner uses) is not one: it
/// is the owner's, and `stado space candidates` only proposes it.
pub const CACHES: &[PackageCache] = &[
    PackageCache {
        root: None,
        tool: "Cargo git dependency checkouts",
    },
    PackageCache {
        root: Some(".npm/_cacache"),
        tool: "npm package cache",
    },
    PackageCache {
        root: Some(".bun/install/cache"),
        tool: "Bun package cache",
    },
    PackageCache {
        root: Some("Library/Caches/org.swift.swiftpm"),
        tool: "SwiftPM package cache",
    },
    PackageCache {
        root: Some("Library/Caches/pip"),
        tool: "pip package cache",
    },
    PackageCache {
        root: Some(".cache/pip"),
        tool: "pip package cache",
    },
    PackageCache {
        root: Some(".cache/uv"),
        tool: "uv package cache",
    },
    PackageCache {
        root: Some("Library/Caches/Homebrew"),
        tool: "Homebrew download cache",
    },
    PackageCache {
        root: Some("Library/Caches/com.microsoft.VSCode.ShipIt"),
        tool: "VS Code update staging",
    },
];

/// Cargo's home: `CARGO_HOME`, else `~/.cargo`.
fn cargo_home(home: &Path) -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".cargo"),
    }
}

/// Where `cache` is on this host. Of Cargo's git cache only `checkouts/`
/// goes: each is a working tree Cargo checks out again from `db/`, the
/// fetched repositories, which stay.
pub fn cache_path(home: &Path, cache: &PackageCache) -> PathBuf {
    match cache.root {
        Some(root) => home.join(root),
        None => cargo_home(home).join("git").join("checkouts"),
    }
}

/// Where a cache is moved in one rename before it is removed: beside it, so
/// on the same volume, under a name no tool reads.
fn aside_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.stado-removing"))
}

/// Cargo's package-cache lock: free (this pass holds it, or no Cargo made
/// one yet, so none is held) or held by a running `cargo`.
enum CargoLock {
    Free(Option<std::fs::File>),
    Held,
}

/// The lock a running `cargo` holds while it fetches, taken without waiting
/// and held while Cargo's checkouts are removed. It is opened, never made:
/// a home where Cargo made none has nothing holding it.
fn cargo_package_lock(home: &Path) -> Result<CargoLock, JanitorError> {
    let path = cargo_home(home).join(".package-cache");
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CargoLock::Free(None))
        }
        Err(error) => {
            return Err(JanitorError::os(&format!(
                "Cargo's package-cache lock {} cannot be opened: {error}; it must be this \
                 account's file, as Cargo makes it, so a fetch running now is seen",
                path.display()
            )))
        }
    };
    // SAFETY: a descriptor this function owns; flock only takes an advisory
    // lock on it and returns an errno.
    let locked =
        unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    if locked.is_negative() {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(CargoLock::Held);
        }
        return Err(JanitorError::os(&format!(
            "Cargo's package-cache lock {} cannot be taken: {error}",
            path.display()
        )));
    }
    Ok(CargoLock::Free(Some(file)))
}

/// Whether the volume needs more room: it is at the rule, or it cannot take
/// `margin` more bytes before reaching it.
fn needs_room(home: &Path, margin: &Margin, report: &mut CleanupReport) -> bool {
    match read_fleet_volume(home) {
        Ok(reading) => reading.full() || reading.headroom_bytes() < margin.bytes,
        Err(error) => {
            report.add_error(CLEANER, &error);
            false
        }
    }
}

/// Remove a tree a pass moved aside and record it.
fn remove_aside(
    aside: &Path,
    path: &Path,
    bytes: i64,
    state_dir: Option<&Path>,
    met: &mut Met,
    report: &mut CleanupReport,
) {
    match remove_tree(aside) {
        Ok(()) => {
            met.removed(Some(bytes.unsigned_abs()));
            if let Some(state_dir) = state_dir {
                if let Err(error) =
                    ledger::record_tree(state_dir, CLEANER, report.writer, path, bytes, bytes)
                {
                    report.add_error("removal_record", &error);
                }
            }
        }
        Err(error) => {
            met.skipped("deletion_refused");
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "cannot remove {}, the cache moved aside from {}: {error}; the next pass \
                     removes it again",
                    aside.display(),
                    path.display()
                )),
            );
        }
    }
}

/// After every other cleaner ran: while the volume is at the rule or cannot
/// take `margin` more bytes, remove the package caches largest first until
/// it can. `enforcing` false measures every cache and removes none. Runs
/// under the pass's exclusive cleanup lock.
pub(in crate::providers::local::space_reclaim) fn scan_package_caches(
    home: &Path,
    state_dir: Option<&Path>,
    enforcing: bool,
    deletable: Option<&Path>,
    margin: &Margin,
    report: &mut CleanupReport,
) {
    let mut met = Met::default();
    if enforcing {
        // A cache an earlier pass moved aside and could not finish removing
        // is no tool's any more: it goes first, whatever the volume reads.
        for cache in CACHES {
            let path = cache_path(home, cache);
            let aside = aside_path(&path);
            if std::fs::symlink_metadata(&aside)
                .is_ok_and(|info| info.is_dir() && !info.file_type().is_symlink())
            {
                let bytes = dir_size(&aside);
                remove_aside(&aside, &path, bytes, state_dir, &mut met, report);
            }
        }
    }
    // `dir_size` sums lengths, so it is never negative.
    let mut present: Vec<(PathBuf, &PackageCache, i64)> = CACHES
        .iter()
        .map(|cache| (cache_path(home, cache), cache))
        .filter(|(path, _)| {
            std::fs::symlink_metadata(path)
                .is_ok_and(|info| info.is_dir() && !info.file_type().is_symlink())
        })
        .map(|(path, cache)| {
            let bytes = dir_size(&path);
            (path, cache, bytes)
        })
        .collect();
    present.sort_by_key(|(_, _, size)| std::cmp::Reverse(*size));
    met.listed(present.len());
    for (_, _, bytes) in &present {
        met.eligible(bytes.unsigned_abs());
    }
    report.margin = Some(margin.clone());
    if !enforcing || !needs_room(home, margin, report) {
        // Agent output left the room the next build needs, or the pass only
        // previews: every cache stays, counted so the report says it was
        // considered.
        if enforcing {
            for (_, _, bytes) in &present {
                met.kept("agent_output_was_enough", bytes.unsigned_abs());
            }
        }
        finish(met, report);
        return;
    }
    let mut enough = false;
    for (path, cache, bytes) in present {
        let size = bytes.unsigned_abs();
        if enough {
            met.kept("room_for_the_next_build", size);
            continue;
        }
        if !ownership::may_delete(deletable, &path) {
            met.kept(ownership::NOT_STADO_OWNED, size);
            continue;
        }
        // Read again for this cache alone, just before it goes: a tool
        // started while an earlier cache was removed is seen.
        let holders = match Holders::read(CLEANER) {
            Some(holders) => holders,
            None => {
                met.kept("process_table_unread", size);
                continue;
            }
        };
        if holders.hold(&path) {
            met.kept("held_by_process", size);
            continue;
        }
        let _cargo_lock = if cache.root.is_none() {
            if holders.cargo_runs() {
                met.kept("build_running", size);
                continue;
            }
            match cargo_package_lock(home) {
                Ok(CargoLock::Free(lock)) => lock,
                Ok(CargoLock::Held) => {
                    met.kept("held_by_process", size);
                    continue;
                }
                Err(error) => {
                    report.add_error(CLEANER, &error);
                    met.kept("cargo_lock_unreadable", size);
                    continue;
                }
            }
        } else {
            None
        };
        // One rename, right after the holders were read: a tool that starts
        // from here on finds no cache and makes a new one, instead of
        // reading one half removed.
        let aside = aside_path(&path);
        if let Err(error) = std::fs::rename(&path, &aside) {
            met.skipped("deletion_refused");
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "cannot move {} ({}) aside to {} before removing it: {error}",
                    path.display(),
                    cache.tool,
                    aside.display()
                )),
            );
            continue;
        }
        remove_aside(&aside, &path, bytes, state_dir, &mut met, report);
        enough = !needs_room(home, margin, report);
    }
    finish(met, report);
}

fn finish(met: Met, report: &mut CleanupReport) {
    match met.report() {
        Ok(record) => report.package_caches = record,
        Err(cause) => report.add_error(CLEANER, &JanitorError::os(&cause)),
    }
}
