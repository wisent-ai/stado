//! Weles recordings cleanup: whole-run eviction gated on age, durable
//! upload proof, and run inactivity.
//!
//! Port of the weles half of `stado/providers/local/disk/cleanup.py`
//! (`_weles_upload_proof_ok`, `_weles_run_active`, `_weles_dir_size`,
//! `_scan_weles`). Unlike the HF cleaner this pass is path-based (as in
//! the Python): the safety gate is the ordered series of refusals before
//! `rmtree`, plus the lexical commonpath check — every refusal below is
//! covered by the module test suite.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{euid, fixed_root, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

/// True only when the run carries a valid whole-run upload proof and
/// nothing was written into the run afterwards.
///
/// The proof (recordings/<run>/.uploaded.json, written by the Weles worker
/// after a zero-failure mirror) is invalidated by any newer direct child —
/// a file added after the upload means storage is no longer complete.
/// Python `_weles_upload_proof_ok`.
fn upload_proof_ok(run_dir: &Path) -> bool {
    let proof_path = run_dir.join(".uploaded.json");
    let text = match std::fs::read_to_string(&proof_path) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let proof: Value = match serde_json::from_str(&text) {
        Ok(proof) => proof,
        Err(_) => return false,
    };
    if !proof.is_object() || proof.get("version") != Some(&Value::from(1)) {
        return false;
    }
    match proof.get("file_count") {
        Some(Value::Number(n)) if n.as_i64().is_some_and(|c| c > 0) => {}
        _ => return false,
    }
    let Some(uploaded_at_raw) = proof.get("uploaded_at").and_then(Value::as_str) else {
        return false;
    };
    let uploaded_at = match parse_iso_timestamp(uploaded_at_raw) {
        Some(ts) => ts,
        None => return false,
    };
    let entries = match std::fs::read_dir(run_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        if entry.file_name() == ".uploaded.json" {
            continue;
        }
        // DirEntry::metadata does not follow symlinks
        // (entry.stat(follow_symlinks=False) parity).
        match entry.metadata() {
            Ok(info) => {
                if info.mtime() as f64 > uploaded_at {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

/// Python `datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()`.
fn parse_iso_timestamp(raw: &str) -> Option<f64> {
    let replaced = raw.replace('Z', "+00:00");
    let dt = chrono::DateTime::parse_from_rfc3339(&replaced).ok()?;
    let seconds = dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_nanos()) / 1e9;
    Some(seconds)
}

/// Any direct child fresher than cutoff means the run is likely live
/// (dir mtime alone misses in-place file writes). Errors read as
/// inactive: the outer age gate already passed.
/// Python `_weles_run_active`.
fn run_active(path: &Path, cutoff: f64) -> bool {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        match entry.metadata() {
            Ok(info) => {
                if info.mtime() as f64 > cutoff {
                    return true;
                }
            }
            Err(_) => continue,
        }
    }
    false
}

/// Python `_weles_dir_size`: recursive file-size total. os.walk
/// classifies with entry.is_dir() (FOLLOWS symlinks: a symlinked dir is
/// listed but, with followlinks=False, never recursed and never sized);
/// getsize also follows symlinks. Unreadable entries are skipped.
fn dir_size(path: &Path) -> i64 {
    let mut total = 0i64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let child = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            let Ok(target) = std::fs::metadata(&child) else { continue };
            if target.is_dir() {
                if !kind.is_symlink() {
                    stack.push(child);
                }
            } else {
                total += target.len() as i64;
            }
        }
    }
    total
}

/// Python `shutil.rmtree(entry.path)`: refuses a top-level symlink, never
/// follows symlinked directories, unlinks everything else. Errors abort
/// the removal and surface to the caller (Python's default onerror).
fn remove_tree(path: &Path) -> io::Result<()> {
    let info = std::fs::symlink_metadata(path)?;
    if info.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot call rmtree on a symbolic link",
        ));
    }
    if !info.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a directory"));
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(path)?
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    for child in entries {
        let child_info = std::fs::symlink_metadata(&child)?;
        if child_info.is_dir() && !child_info.file_type().is_symlink() {
            remove_tree(&child)?;
        } else {
            std::fs::remove_file(&child)?;
        }
    }
    std::fs::remove_dir(path)
}

/// Scan the weles recordings root and evict eligible runs.
/// Python `_scan_weles`.
pub fn scan_weles(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get("weles_recordings") else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = if let Some(configured_root) = &configured.root {
            let expanded = crate::config_file::expand_tilde(configured_root);
            if !expanded.is_dir() {
                report.skip_weles("root_absent", 1);
                return Ok(());
            }
            expanded
        } else {
            let parts = [std::ffi::OsString::from("weles"), std::ffi::OsString::from("recordings")];
            match fixed_root(home, &parts, false)? {
                Some(root) => root,
                None => {
                    report.skip_weles("root_absent", 1);
                    return Ok(());
                }
            }
        };
        let mut ordered: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
        {
            let entries = std::fs::read_dir(&root)?;
            for entry in entries {
                let entry = entry?;
                ordered.push((entry.file_name(), entry.path()));
                if ordered.len() as i64 >= remaining_scan {
                    break;
                }
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        let mut deleted_bytes = 0i64;
        for (name, path) in ordered {
            report.weles.scanned_items += 1;
            if name == "local" || name.to_string_lossy().starts_with('.') {
                report.skip_weles("reserved_or_hidden", 1);
                continue;
            }
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_weles("stat_failed", 1);
                    continue;
                }
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                report.skip_weles("not_run_directory", 1);
                continue;
            }
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_weles("unsafe_owner_or_device", 1);
                continue;
            }
            if info.mtime() as f64 > now - configured.min_age_seconds as f64 {
                report.skip_weles("too_young", 1);
                continue;
            }
            if !configured.allow_missing_upload_proof && !upload_proof_ok(&path) {
                // Without durable whole-run upload proof, age alone is never
                // sufficient authorization to delete (default, conservative).
                report.skip_weles("upload_proof_unavailable_v1", 1);
                continue;
            }
            if run_active(&path, now - configured.min_age_seconds as f64) {
                report.skip_weles("active_run", 1);
                continue;
            }
            report.weles.eligible_items += 1;
            let expected = dir_size(&path);
            report.weles.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.weles.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_weles("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_weles("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            // os.path.commonpath([root, entry.path]) != root — lexical
            // escape check; the run dir must stay a direct child.
            if path.parent() != Some(root.as_path()) {
                report.skip_weles("escapes_root", 1);
                continue;
            }
            // Python wraps the free-space probes + rmtree in one
            // try/except (OSError, shutil.Error) per entry.
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.weles.actual_free_delta_bytes += delta.max(0);
                    report.weles.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error("weles_recordings", &exc),
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error("weles_recordings", &exc);
    }
}

#[cfg(test)]
mod tests {
    use super::super::kit::*;
    use super::*;
    use serde_json::json;

    fn enforce_registry(cleaner: Value) -> Value {
        registry_json(
            "testhost",
            super::super::kit::policy_json("enforce", json!({"weles_recordings": cleaner})),
        )
    }

    // ------------------------------------------------------------------
    // upload proof validation
    // ------------------------------------------------------------------

    #[test]
    fn upload_proof_rules() {
        let th = TempHome::new();
        let run = make_weles_run(&th.home, "run1", &[("data.bin", b"x")]);
        let now = now_epoch();
        assert!(!upload_proof_ok(&run)); // no proof at all
        std::fs::write(run.join(".uploaded.json"), "not json").unwrap();
        assert!(!upload_proof_ok(&run));
        std::fs::write(run.join(".uploaded.json"), json!({"version": 2, "file_count": 1, "uploaded_at": "2026-01-01T00:00:00+00:00"}).to_string()).unwrap();
        assert!(!upload_proof_ok(&run)); // wrong proof version
        write_upload_proof(&run, now - 100, 0);
        assert!(!upload_proof_ok(&run)); // zero file_count
        write_upload_proof(&run, now - 100, 1);
        // data.bin is newer than uploaded_at: the mirror is stale.
        assert!(!upload_proof_ok(&run));
        // Once the child predates the proof, the proof is valid.
        set_mtime(&run.join("data.bin"), now - 200);
        assert!(upload_proof_ok(&run));
        // A broken timestamp invalidates the proof.
        std::fs::write(run.join(".uploaded.json"), json!({"version": 1, "file_count": 1, "uploaded_at": "when?"}).to_string()).unwrap();
        assert!(!upload_proof_ok(&run));
        // "Z" suffix timestamps parse (fromisoformat replace parity):
        // stamp the proof AFTER the child's mtime, with a Z suffix.
        let stamp = chrono::DateTime::from_timestamp(now - 100, 0)
            .unwrap()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        std::fs::write(
            run.join(".uploaded.json"),
            json!({"version": 1, "file_count": 1, "uploaded_at": stamp}).to_string(),
        )
        .unwrap();
        assert!(upload_proof_ok(&run));
    }

    #[test]
    fn run_active_detects_fresh_children() {
        let th = TempHome::new();
        let run = make_weles_run(&th.home, "run1", &[("old.bin", b"x")]);
        let now = now_epoch();
        set_mtime(&run.join("old.bin"), now - 100_000);
        assert!(!run_active(&run, (now - 86_400) as f64));
        std::fs::write(run.join("fresh.bin"), b"y").unwrap();
        assert!(run_active(&run, (now - 86_400) as f64));
        assert!(!run_active(&th.join("does-not-exist"), (now - 86_400) as f64));
    }

    #[test]
    fn dir_size_counts_like_os_walk() {
        let th = TempHome::new();
        let run = make_weles_run(&th.home, "run1", &[("a", b"aaaa"), ("b", b"bb")]);
        std::fs::create_dir(run.join("sub")).unwrap();
        std::fs::write(run.join("sub/c"), b"ccc").unwrap();
        assert_eq!(dir_size(&run), 9);
        // A symlink to a file counts the TARGET size (getsize follows).
        let outside = th.join("outside");
        std::fs::write(&outside, b"12345").unwrap();
        std::os::unix::fs::symlink(&outside, run.join("link-file")).unwrap();
        assert_eq!(dir_size(&run), 14);
        // A symlink to a directory is listed but never walked or sized.
        let outside_dir = th.join("outside-dir");
        std::fs::create_dir(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("big"), b"xxxxxxxxxx").unwrap();
        std::os::unix::fs::symlink(&outside_dir, run.join("link-dir")).unwrap();
        assert_eq!(dir_size(&run), 14);
    }

    #[test]
    fn remove_tree_refuses_top_symlink_and_never_follows() {
        let th = TempHome::new();
        let run = make_weles_run(&th.home, "run1", &[("a", b"aaaa")]);
        // An inside symlink pointing OUTSIDE the root: unlinked, never followed.
        let outside = th.join("outside-file");
        std::fs::write(&outside, b"precious").unwrap();
        std::os::unix::fs::symlink(&outside, run.join("escape")).unwrap();
        std::fs::create_dir(run.join("sub")).unwrap();
        std::fs::write(run.join("sub/deep"), b"deep").unwrap();
        remove_tree(&run).unwrap();
        assert!(!run.exists());
        assert!(outside.exists());
        // shutil.rmtree refuses a symbolic link at the top.
        let run2 = make_weles_run(&th.home, "run2", &[("a", b"a")]);
        let link = th.join("run2-link");
        std::os::unix::fs::symlink(&run2, &link).unwrap();
        assert!(remove_tree(&link).is_err());
        assert!(run2.exists());
    }

    // ------------------------------------------------------------------
    // the full scan through run_with_lock
    // ------------------------------------------------------------------

    fn aged_run(th: &TempHome, run: &str, proof: bool) -> PathBuf {
        let now = now_epoch();
        let dir = make_weles_run(&th.home, run, &[("data.bin", b"payload")]);
        if proof {
            set_mtime(&dir.join("data.bin"), now - 300);
            write_upload_proof(&dir, now - 200, 1);
        }
        backdate_tree(&dir, 2 * 86_400);
        dir
    }

    #[test]
    fn enforce_deletes_only_proven_runs() {
        let th = TempHome::new();
        let proven = aged_run(&th, "proven-run", true);
        let unproven = aged_run(&th, "unproven-run", false);
        let report = run_pass(&th, enforce_registry(weles_cleaner()), "testhost", 0, false);
        let weles = &report["cleaners"]["weles_recordings"];
        assert_eq!(weles["deleted_items"], 1, "{report}");
        assert_eq!(weles["skipped"]["upload_proof_unavailable_v1"], 1, "{report}");
        assert!(!proven.exists());
        assert!(unproven.exists());
    }

    #[test]
    fn allow_missing_proof_opt_in_deletes_unproven() {
        let th = TempHome::new();
        let unproven = aged_run(&th, "unproven-run", false);
        let cleaner = json!({"min_age_seconds": 86400, "allow_missing_upload_proof": true});
        let report = run_pass(&th, enforce_registry(cleaner), "testhost", 0, false);
        assert_eq!(report["cleaners"]["weles_recordings"]["deleted_items"], 1, "{report}");
        assert!(!unproven.exists());
    }

    #[test]
    fn report_mode_counts_but_never_deletes_weles() {
        let th = TempHome::new();
        let proven = aged_run(&th, "proven-run", true);
        let registry = registry_json(
            "testhost",
            super::super::kit::policy_json("report", json!({"weles_recordings": weles_cleaner()})),
        );
        let report = run_pass(&th, registry, "testhost", 0, false);
        let weles = &report["cleaners"]["weles_recordings"];
        assert_eq!(weles["eligible_items"], 1, "{report}");
        assert_eq!(weles["deleted_items"], 0);
        assert!(weles["expected_bytes"].as_i64().unwrap() > 0);
        assert!(proven.exists());
        assert_eq!(report["outcome"], "report_only");
    }

    #[test]
    fn young_and_active_runs_are_skipped() {
        let th = TempHome::new();
        let now = now_epoch();
        // Young: run dir mtime inside min_age.
        let young = make_weles_run(&th.home, "young-run", &[("data.bin", b"x")]);
        let _ = &young;
        // Old dir, valid proof, but a child newer than the age cutoff
        // (yet older than the proof, so the proof itself stays valid —
        // the active-run check is what must fire).
        let active = aged_run(&th, "active-run", true);
        std::fs::write(active.join("live.bin"), b"just now").unwrap();
        set_mtime(&active.join("live.bin"), now - 1000);
        write_upload_proof(&active, now - 100, 2);
        set_mtime(&active, now - 2 * 86_400); // dir itself stays old
        let report = run_pass(&th, enforce_registry(weles_cleaner()), "testhost", 0, false);
        let weles = &report["cleaners"]["weles_recordings"];
        assert_eq!(weles["skipped"]["too_young"], 1, "{report}");
        assert_eq!(weles["skipped"]["active_run"], 1, "{report}");
        assert_eq!(weles["deleted_items"], 0);
        assert!(young.exists());
        assert!(active.exists());
        // active_run is NOT a public skip reason (Python parity).
        let public = super::super::sanitize_cleanup_report(&report);
        assert_eq!(public["cleaners"]["weles_recordings"]["skipped"], json!({"too_young": 1}));
    }

    #[test]
    fn reserved_hidden_and_non_directory_entries_are_skipped() {
        let th = TempHome::new();
        let root = th.join("weles/recordings");
        let local = make_weles_run(&th.home, "local", &[("x", b"x")]);
        backdate_tree(&local, 2 * 86_400);
        let hidden = root.join(".hidden-run");
        std::fs::create_dir_all(&hidden).unwrap();
        backdate_tree(&hidden, 2 * 86_400);
        let stray_file = root.join("stray-file");
        std::fs::write(&stray_file, b"x").unwrap();
        set_mtime(&stray_file, now_epoch() - 2 * 86_400);
        // A symlinked "run dir" — even pointing inside the root — is
        // refused (not_run_directory), and a symlink-to-/etc attack dies
        // at the same guard.
        let real = make_weles_run(&th.home, "real-run", &[("x", b"x")]);
        backdate_tree(&real, 2 * 86_400);
        let link = root.join("link-run");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let etc_link = root.join("etc-run");
        std::os::unix::fs::symlink("/etc", &etc_link).unwrap();
        let report = run_pass(&th, enforce_registry(weles_cleaner()), "testhost", 0, false);
        let weles = &report["cleaners"]["weles_recordings"];
        assert_eq!(weles["skipped"]["reserved_or_hidden"], 2, "{report}");
        assert_eq!(weles["skipped"]["not_run_directory"], 3, "{report}");
        assert_eq!(weles["deleted_items"], 0);
        assert!(local.exists());
        assert!(hidden.exists());
        assert!(stray_file.exists());
        assert!(real.exists());
        assert!(Path::new("/etc").exists());
    }

    #[test]
    fn missing_upload_proof_after_upload_is_stale() {
        let th = TempHome::new();
        let now = now_epoch();
        // Proof exists, but a file appeared AFTER the upload: the mirror
        // is incomplete, so age alone must not authorize deletion.
        let dir = make_weles_run(&th.home, "stale-run", &[("data.bin", b"payload")]);
        write_upload_proof(&dir, now - 200, 1);
        std::fs::write(dir.join("late.bin"), b"written after upload").unwrap();
        set_mtime(&dir.join("late.bin"), now - 100);
        set_mtime(&dir.join("data.bin"), now - 300);
        backdate_tree(&dir, 2 * 86_400);
        // backdate_tree rewrote the child mtimes to 2 days ago, which is
        // BEFORE uploaded_at... restore the attack shape: late.bin newer
        // than the proof's uploaded_at but dir old.
        write_upload_proof(&dir, now - 300, 1);
        set_mtime(&dir.join("late.bin"), now - 100);
        set_mtime(&dir.join("data.bin"), now - 400);
        set_mtime(&dir, now - 2 * 86_400);
        let report = run_pass(&th, enforce_registry(weles_cleaner()), "testhost", 0, false);
        let weles = &report["cleaners"]["weles_recordings"];
        assert_eq!(weles["skipped"]["upload_proof_unavailable_v1"], 1, "{report}");
        assert_eq!(weles["deleted_items"], 0);
        assert!(dir.exists());
    }

    #[test]
    fn configured_root_override_is_honored() {
        let th = TempHome::new();
        let custom = th.join("custom-recordings");
        std::fs::create_dir(&custom).unwrap();
        let run = custom.join("proven-run");
        std::fs::create_dir(&run).unwrap();
        std::fs::write(run.join("data.bin"), b"payload").unwrap();
        let now = now_epoch();
        set_mtime(&run.join("data.bin"), now - 300);
        write_upload_proof(&run, now - 200, 1);
        backdate_tree(&run, 2 * 86_400);
        let cleaner = json!({
            "min_age_seconds": 86400,
            "root": custom.to_string_lossy(),
        });
        let report = run_pass(&th, enforce_registry(cleaner), "testhost", 0, false);
        assert_eq!(report["cleaners"]["weles_recordings"]["deleted_items"], 1, "{report}");
        assert!(!run.exists());
    }

    #[test]
    fn absent_root_reports_root_absent() {
        let th = TempHome::new();
        let report = run_pass(&th, enforce_registry(weles_cleaner()), "testhost", 0, false);
        assert_eq!(report["cleaners"]["weles_recordings"]["skipped"]["root_absent"], 1, "{report}");
        assert_eq!(report["outcome"], "no_eligible_items");
    }
}
