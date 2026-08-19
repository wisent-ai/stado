//! Chromium code-sign clone cleanup: eviction of the per-launch bundle clones
//! macOS leaves in this account's temporary container.
//!
//! NO Python original. Measured on `charless-mac-mini` on 2026-08-18: free
//! space had fallen to about 2 GiB against the registry's 55 GiB policy, its
//! queue agent published `disk_pressure_unresolved`, admission failed closed,
//! and every release build queued behind that host for hours. Three consumers
//! held the space. Two of them are now stages of
//! [`crate::deploy::host_reclaim`] — `$HOME/.stado/build-work` at about 21 GiB
//! and the legacy delivered worker trees at about 9 GiB. The third had no
//! owner anywhere in the product: `<temporary container>/`[`CLONE_CONTAINER`]
//! `/`[`CLONE_ROOT_NAME`], where macOS clones the entire browser bundle on
//! EVERY launch so it can validate a signature against an object nobody can
//! swap underneath it. Weles drives Chromium for browser automation, so that
//! host launches it constantly, and a run that is killed leaves its clone
//! behind: 137 of them on the mini when this was written, 130 untouched for
//! more than a day, and neither the janitor nor any command removed or even
//! reported a single one.
//!
//! What may be taken is the clone of a launch that is over, and three gates
//! establish that, because macOS records nothing about which clone belongs to
//! which process:
//!
//! - **the policy's minimum age.** The clone is made at launch, so a browser
//!   that started within the retention window owns a clone younger than the
//!   gate. The registry floors this cleaner at a day
//!   ([`crate::targets`]'s per-cleaner minimum), which is the same floor the
//!   weles and build-cache cleaners carry and the same one the shell script
//!   written during the outage used.
//! - **the newest clone, kept unconditionally.** A browser that has been up
//!   longer than the retention window has a clone older than the gate, and it
//!   is the most recent one in the root: keeping it costs one bundle and
//!   removes the only case age alone cannot see. Same rule, same reason, as
//!   `host reclaim`'s "never the newest artefact".
//! - **one snapshot of the process table per pass.** A clone whose path any
//!   live argv names is never a candidate — that is what an app launched out
//!   of its own clone (a translocated bundle) looks like from outside.
//!
//! Every operation is path-based, as in [`super::weles`] rather than
//! [`super::hf`]: the root is a fixed, owner-only directory of shallow entries
//! macOS itself named, so the ordered refusals below plus the parent check are
//! the safety gate, and the tree walk and the removal are that module's —
//! imported, not copied.
//!
//! `expected_bytes` here is apparent size, and for clones it OVERSTATES what
//! comes back: macOS makes them with `clonefile`, so a clone shares its blocks
//! with the installed bundle until one of them is written to. The number that
//! is true is `actual_free_delta_bytes`, measured either side of each removal
//! the same way every other cleaner measures it.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use super::weles::{dir_size, remove_tree};
use super::{euid, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report, and this scan have
/// to name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "chromium_clones";

/// The directory macOS keeps code-sign clones in, beside the `T` (temporary)
/// and `C` (cache) directories of the same per-user container.
///
/// One letter, unhelpfully, and there is no `confstr` variable for it — only
/// for its siblings — so the container is resolved through
/// [`darwin_user_temp_dir`] and this name is joined onto it.
pub const CLONE_CONTAINER: &str = "X";

/// The per-application clone root. `org.chromium.Chromium` is the bundle
/// identifier of the Chromium builds Weles drives; Chrome, Safari and every
/// other signed app get their own root beside it and are NOT this cleaner's
/// business, because the measurement that authorized this code is Chromium's.
pub const CLONE_ROOT_NAME: &str = "org.chromium.Chromium.code_sign_clone";

/// The prefix macOS gives every clone it makes in that root
/// (`code_sign_clone.XXXXXX`).
///
/// Required of a candidate: an entry the OS did not name is not a clone, and
/// the one thing this cleaner must never do is delete something that merely
/// happens to live in a directory it was pointed at. Exported so
/// [`crate::deploy::host_reclaim`]'s stage requires the same name of the same
/// entries — two spellings would be two definitions of "clone", and the
/// operator would meet whichever ran first.
pub const CLONE_ENTRY_PREFIX: &str = "code_sign_clone.";

/// This account's temporary container as macOS itself reports it
/// (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, e.g.
/// `/var/folders/zy/l0_0w9dn0k94n1b7xnt7kpv80000gn/T/`), or `None` where the
/// platform has no such thing.
///
/// Read from libc and not from `$TMPDIR`, because the janitor runs both from a
/// launchd agent and from an ssh session, and only the first of those two is
/// guaranteed to carry that variable — a cleaner that silently found no root
/// over ssh would report a healthy no-op on the exact host whose disk was
/// full. The `unsafe` is the same shape as [`super::euid`]'s `geteuid`: one
/// libc call whose contract is a byte count into a buffer we own.
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let mut buffer = vec![0u8; nix::libc::PATH_MAX as usize];
    let written = unsafe {
        nix::libc::confstr(
            nix::libc::_CS_DARWIN_USER_TEMP_DIR,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    // 0 is "this variable has no value"; a length past the buffer is a value
    // that was truncated, and a truncated path is not a path.
    if written == 0 || written > buffer.len() {
        return None;
    }
    buffer.truncate(written - 1); // confstr counts the terminating NUL
    let path = PathBuf::from(OsString::from_vec(buffer));
    path.is_absolute().then_some(path)
}

/// No per-user clone container exists off Apple platforms: macOS validates
/// code signatures this way and nothing else does.
#[cfg(not(target_os = "macos"))]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    None
}

/// Where the clones of this account's Chromium launches live, when the
/// platform has such a place.
pub fn default_root() -> Option<PathBuf> {
    let container = darwin_user_temp_dir()?.parent()?.to_path_buf();
    Some(container.join(CLONE_CONTAINER).join(CLONE_ROOT_NAME))
}

/// Every live process's argv, taken ONCE for the whole pass.
///
/// One snapshot, not one probe per candidate, for the reason
/// [`crate::deploy::host_reclaim`] states about its own stages: a per-candidate
/// `ps | grep <path>` matches the grep's own argv and reports every path as
/// held. `None` means the process table could not be read at all, which this
/// cleaner treats as a refusal to delete rather than as an empty table.
fn process_snapshot() -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(["-Ao", "args="])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// True when some live process names this exact path in its argv.
fn held(snapshot: &str, path: &Path) -> bool {
    snapshot.contains(path.to_string_lossy().as_ref())
}

/// Scan the Chromium clone root and evict the clones of finished launches.
///
/// `remaining_scan` is this cleaner's share of `max_scan_items` left by the
/// cleaners that ran before it, and `deadline` is the pass deadline the HF and
/// build-cache scans honour. It matters here and not in [`super::weles`]:
/// sizing one clone means walking a whole browser bundle, and the root holds
/// one per launch.
pub fn scan_chromium_clones(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = match &configured.root {
            Some(configured_root) => crate::config_file::expand_tilde(configured_root),
            None => match default_root() {
                Some(root) => root,
                None => {
                    report.skip_clones("root_absent", 1);
                    return Ok(());
                }
            },
        };
        if !root.is_dir() {
            // A host that has never launched Chromium, or a host that is not a
            // Mac. Neither is a fault, and neither is a reason to look
            // anywhere else.
            report.skip_clones("root_absent", 1);
            return Ok(());
        }
        let mut ordered: Vec<(OsString, PathBuf)> = Vec::new();
        {
            let entries = std::fs::read_dir(&root)?;
            for entry in entries {
                let entry = entry?;
                ordered.push((entry.file_name(), entry.path()));
                if ordered.len() as i64 >= remaining_scan {
                    report.caps.scan = true;
                    report.skip_clones("scan_cap", 1);
                    break;
                }
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        // The most recent CLONE of the enumerated set, by the mtime macOS
        // stamped when it made it. Kept whatever else is true — see the module
        // header: a session older than the retention window still owns exactly
        // this one, and nothing in the OS says which session that is.
        //
        // Chosen among entries that could be candidates at all, and not among
        // everything in the root: a stray directory nobody launched, sitting
        // there with the freshest mtime, would otherwise "protect" itself and
        // leave the live browser's clone as the newest thing eligible — the
        // guard inverted into the one deletion it exists to prevent.
        let newest = ordered
            .iter()
            .filter(|(name, _)| name.to_string_lossy().starts_with(CLONE_ENTRY_PREFIX))
            .filter_map(|(_, path)| {
                let info = std::fs::symlink_metadata(path).ok()?;
                let clone = info.is_dir() && !info.file_type().is_symlink();
                clone.then(|| (info.mtime(), path.clone()))
            })
            .max_by_key(|(mtime, _)| *mtime)
            .map(|(_, path)| path);
        // Without a process table there is no live-process gate, and this
        // cleaner does not delete with a gate missing.
        let Some(snapshot) = process_snapshot() else {
            report.skip_clones("process_table_unavailable", ordered.len() as i64);
            return Ok(());
        };
        let mut deleted_bytes = 0i64;
        for (name, path) in ordered {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_clones("scan_deadline", 1);
                break;
            }
            report.clones.scanned_items += 1;
            let name = name.to_string_lossy();
            if !name.starts_with(CLONE_ENTRY_PREFIX) {
                report.skip_clones("reserved_or_hidden", 1);
                continue;
            }
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_clones("stat_failed", 1);
                    continue;
                }
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                report.skip_clones("not_run_directory", 1);
                continue;
            }
            // A clone the OS made for THIS account, on the volume the policy's
            // watermarks are measured against. Anything else is either not
            // ours to delete or would not move the number that matters.
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_clones("unsafe_owner_or_device", 1);
                continue;
            }
            if info.mtime() as f64 > now - configured.min_age_seconds as f64 {
                report.skip_clones("too_young", 1);
                continue;
            }
            if newest.as_deref() == Some(path.as_path()) {
                report.skip_clones("newest_clone", 1);
                continue;
            }
            if held(&snapshot, &path) {
                report.skip_clones("active_run", 1);
                continue;
            }
            report.clones.eligible_items += 1;
            let expected = dir_size(&path);
            report.clones.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.clones.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_clones("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_clones("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            // The clone must still be a direct child of the root it was
            // enumerated from, the same lexical check the weles scan makes
            // before it removes a run.
            if path.parent() != Some(root.as_path()) {
                report.skip_clones("escapes_root", 1);
                continue;
            }
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.clones.actual_free_delta_bytes += delta.max(0);
                    report.clones.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error(CLEANER, &exc),
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}

