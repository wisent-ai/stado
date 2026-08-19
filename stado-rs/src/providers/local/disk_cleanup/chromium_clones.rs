//! Chromium code-sign clone cleanup: eviction of the per-launch bundle clones
//! macOS leaves in this account's temporary container.
//!
//! NO Python original. Measured on `control-host` on 2026-08-18: free
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    use super::super::kit::{self, TempHome};

    /// A clone root of this cleaner's own making, in a scratch home. NEVER the
    /// real per-user container: other processes on this machine keep live
    /// clones there, and a test is not entitled to a single one of them.
    fn clone_root(th: &TempHome) -> PathBuf {
        let root = th.join("clones").join(CLONE_ROOT_NAME);
        std::fs::create_dir_all(&root).expect("scratch clone root");
        root
    }

    /// One clone bundle, sized by a real file so `expected_bytes` has
    /// something to add up, aged `age_secs` into the past.
    fn make_clone(root: &Path, name: &str, age_secs: i64) -> PathBuf {
        let clone = root.join(name);
        let bundle = clone
            .join("Chromium.app.bundle")
            .join("Contents")
            .join("MacOS");
        std::fs::create_dir_all(&bundle).expect("clone bundle");
        std::fs::write(bundle.join("Chromium"), vec![7u8; 4096]).expect("clone executable");
        kit::backdate_tree(&clone, age_secs);
        clone
    }

    /// The policy this cleaner runs under: `min_age_seconds` at the registry
    /// floor of one day, with the root pointed at the scratch tree.
    fn cleaner(root: &Path) -> Value {
        json!({
            "min_age_seconds": 86400,
            "root": root.display().to_string(),
        })
    }

    fn enforce_pass(th: &TempHome, root: &Path) -> Value {
        let registry = kit::registry_json(
            "testhost",
            kit::policy_json("enforce", json!({CLEANER: cleaner(root)})),
        );
        kit::run_pass(th, registry, "testhost", 0, false)
    }

    fn counts(report: &Value) -> Value {
        report["cleaners"][CLEANER].clone()
    }

    /// Two days is past the one-day floor; two minutes is not.
    const STALE: i64 = 2 * 86400;
    const FRESH: i64 = 120;

    /// The clone of a finished launch goes, and the report counts the bytes it
    /// measured rather than the bytes it guessed.
    #[test]
    fn a_stale_clone_of_a_finished_launch_is_removed() {
        let th = TempHome::new();
        let root = clone_root(&th);
        let doomed = make_clone(&root, "code_sign_clone.aaaaaa", STALE);
        // The newest clone is kept unconditionally, so a second, newer one has
        // to exist for the first to be a candidate at all.
        let newest = make_clone(&root, "code_sign_clone.zzzzzz", STALE - 3600);
        let report = enforce_pass(&th, &root);
        assert_eq!(counts(&report)["deleted_items"], 1, "{report}");
        assert!(!doomed.exists(), "the stale clone survived");
        assert!(newest.exists(), "the newest clone was removed");
        assert_eq!(counts(&report)["skipped"]["newest_clone"], 1, "{report}");
    }

    /// A clone a live process names in its argv survives every other rule.
    /// The process is real, its argv carries the exact path, and the clone is
    /// stale, unheld by anything else, and not the newest — so the process
    /// table is the only thing standing between it and removal.
    #[test]
    fn a_clone_a_live_process_holds_is_left_alone() {
        let th = TempHome::new();
        let root = clone_root(&th);
        let held_clone = make_clone(&root, "code_sign_clone.aaaaaa", STALE);
        make_clone(&root, "code_sign_clone.zzzzzz", STALE - 3600);
        // `sleep` with the path as a trailing argument. The trailing `; :`
        // stops the shell from exec'ing over itself, which would drop the
        // argument this test is about.
        let mut holder = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30; :")
            .arg("stado-clone-test-holder")
            .arg(&held_clone)
            .spawn()
            .expect("holder");
        let report = enforce_pass(&th, &root);
        holder.kill().expect("holder stopped");
        holder.wait().expect("holder reaped");
        assert_eq!(counts(&report)["deleted_items"], 0, "{report}");
        assert_eq!(counts(&report)["skipped"]["active_run"], 1, "{report}");
        assert!(held_clone.exists(), "a held clone was removed");
    }

    /// A clone younger than the policy's minimum age survives, because that is
    /// what a browser that is still running looks like: macOS made its clone
    /// at launch.
    ///
    /// Its stale sibling goes in the same pass, which is what makes the
    /// survival mean something — the pass was armed and deleting, and age is
    /// the only difference between the two.
    #[test]
    fn a_clone_younger_than_the_age_gate_is_left_alone() {
        let th = TempHome::new();
        let root = clone_root(&th);
        let fresh = make_clone(&root, "code_sign_clone.aaaaaa", FRESH);
        let stale = make_clone(&root, "code_sign_clone.zzzzzz", STALE);
        let report = enforce_pass(&th, &root);
        assert_eq!(counts(&report)["skipped"]["too_young"], 1, "{report}");
        assert!(fresh.exists(), "a clone younger than the gate was removed");
        assert!(!stale.exists(), "the stale sibling survived: {report}");
        assert_eq!(counts(&report)["deleted_items"], 1, "{report}");
    }

    /// Only entries macOS itself named are candidates: a directory that
    /// merely lives in the root, a symlink, and a plain file all survive.
    #[test]
    fn only_entries_macos_named_are_candidates() {
        let th = TempHome::new();
        let root = clone_root(&th);
        make_clone(&root, "code_sign_clone.zzzzzz", STALE);
        let foreign = root.join("someone-elses-directory");
        std::fs::create_dir_all(&foreign).expect("foreign directory");
        kit::backdate_tree(&foreign, STALE);
        let plain = root.join("code_sign_clone.notadir");
        std::fs::write(&plain, b"not a clone").expect("plain file");
        kit::set_mtime(&plain, kit::now_epoch() - STALE);
        let link = root.join("code_sign_clone.link");
        std::os::unix::fs::symlink(&foreign, &link).expect("symlink");
        let report = enforce_pass(&th, &root);
        let skipped = &counts(&report)["skipped"];
        assert_eq!(skipped["reserved_or_hidden"], 1, "{report}");
        assert_eq!(skipped["not_run_directory"], 2, "{report}");
        assert_eq!(counts(&report)["deleted_items"], 0, "{report}");
        assert!(
            foreign.exists() && plain.exists() && link.exists(),
            "{report}"
        );
    }

    /// A host with no clone root — every Linux host, and any Mac that has
    /// never launched Chromium — reports that and touches nothing.
    #[test]
    fn a_missing_root_is_reported_not_searched_for() {
        let th = TempHome::new();
        let absent = th.join("clones").join(CLONE_ROOT_NAME);
        let report = enforce_pass(&th, &absent);
        assert_eq!(counts(&report)["skipped"]["root_absent"], 1, "{report}");
        assert_eq!(counts(&report)["scanned_items"], 0, "{report}");
    }

    /// In the janitor's report mode nothing is deleted, and the counts still
    /// say what an enforcing pass would have taken.
    #[test]
    fn report_mode_names_the_clones_it_would_take() {
        let th = TempHome::new();
        let root = clone_root(&th);
        let candidate = make_clone(&root, "code_sign_clone.aaaaaa", STALE);
        make_clone(&root, "code_sign_clone.zzzzzz", STALE - 3600);
        let registry = kit::registry_json(
            "testhost",
            kit::policy_json("report", json!({CLEANER: cleaner(&root)})),
        );
        let report = kit::run_pass(&th, registry, "testhost", 0, false);
        assert_eq!(counts(&report)["eligible_items"], 1, "{report}");
        assert_eq!(counts(&report)["deleted_items"], 0, "{report}");
        assert!(
            counts(&report)["expected_bytes"]
                .as_i64()
                .unwrap_or_default()
                >= 4096,
            "{report}"
        );
        assert!(candidate.exists(), "report mode deleted a clone");
    }

    /// The default root is derived from macOS's own answer for this account's
    /// temporary container, and it is the directory the outage was measured
    /// in. Read-only: the path is compared, never enumerated.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_default_root_is_the_containers_clone_directory() {
        let temp = darwin_user_temp_dir().expect("macOS reports a temporary container");
        let root = default_root().expect("macOS has a clone container");
        assert_eq!(root.file_name().unwrap(), CLONE_ROOT_NAME);
        assert_eq!(root.parent().unwrap().file_name().unwrap(), CLONE_CONTAINER);
        assert_eq!(root.parent().unwrap().parent(), temp.parent());
    }
}
