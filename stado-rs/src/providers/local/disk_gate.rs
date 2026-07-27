//! Admission-only disk-space diagnostics for the local agent loop.
//!
//! Port of `stado/providers/local/disk/gate.py`.
//!
//! All cleanup is owned by the registry-authorized policy engine. This
//! module only probes the filesystems and refuses new slots while pressure
//! is unresolved.

use std::io::Write;
use std::path::Path;

/// Python `_free_gb` (`shutil.disk_usage(path).free`): available blocks x
/// fragment size, in GB. -1.0 when the path cannot be statvfs'd.
pub fn free_gb(path: &Path) -> f64 {
    match nix::sys::statvfs::statvfs(path) {
        Ok(stat) => stat.blocks_available() as f64 * stat.fragment_size() as f64 / 1024f64.powi(3),
        Err(_) => -1.0,
    }
}

/// True if the agent can create, flush, and remove a file on this FS.
/// Python `_write_probe_ok` (NamedTemporaryFile delete=True: drop removes).
pub fn write_probe_ok(path: &Path) -> bool {
    let Ok(mut file) = tempfile::NamedTempFile::with_prefix_in(".wc-disk-probe-", path) else {
        return false;
    };
    file.write_all(b"x")
        .and_then(|()| file.flush())
        .and_then(|()| file.as_file().sync_all())
        .is_ok()
}

/// Python `_dir_size_gb`: recursive file-size total in GB. 0 if missing.
pub fn dir_size_gb(path: &Path) -> f64 {
    if !path.is_dir() {
        return 0.0;
    }
    dir_size_bytes(path) as f64 / 1024f64.powi(3)
}

fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            total += dir_size_bytes(&child);
        } else if let Ok(md) = child.metadata() {
            total += md.len();
        }
    }
    total
}

/// Python `_largest_child_dir_gb`: size of the largest direct child dir.
pub fn largest_child_dir_gb(path: &Path) -> f64 {
    let mut largest = 0.0f64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0.0;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            largest = largest.max(dir_size_gb(&child));
        }
    }
    largest
}

/// Raw probe values feeding the admission decision (Python's local
/// variables in `gate_and_maybe_evict`). Injectable so the decision logic
/// is testable with fabricated statvfs data.
#[derive(Debug, Clone, Copy)]
pub struct GateObservation {
    pub home_free_gb: f64,
    pub home_write_probe_ok: bool,
    pub staging_free_gb: f64,
    pub largest_pending_gb: f64,
}

/// Pure admission decision (Python's two `refuse` branches). Returns
/// `refuse` and emits the same log lines through `log_fn`.
pub fn decide(obs: &GateObservation, log_fn: &mut dyn FnMut(&str)) -> bool {
    let mut refuse = !obs.home_write_probe_ok;
    if refuse {
        log_fn(&format!(
            "$HOME write probe failed (~{:.1} GB free); refusing slots this tick",
            obs.home_free_gb
        ));
    }
    // Staging-pressure backpressure is also admission-only.
    if !refuse && obs.staging_free_gb >= 0.0 && obs.staging_free_gb < obs.largest_pending_gb {
        log_fn(&format!(
            "staging low (~{:.0}GB free < measured pending dir {:.0}GB); refusing slots",
            obs.staging_free_gb, obs.largest_pending_gb
        ));
        refuse = true;
    }
    refuse
}

/// Path-free disk diagnostics (Python's `diag` dict). Aggregate
/// measurements only; never publish local filesystem paths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskGateDiag {
    pub free_disk_gb: f64,
    pub home_write_probe_ok: bool,
    pub staging_free_gb: f64,
    pub largest_pending_raw_dir_gb: f64,
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Return admission refusal and path-free disk diagnostics.
/// Python `gate_and_maybe_evict`.
///
/// The historical name remains as the local-agent API, but this function
/// never deletes data. The policy engine runs separately before admission.
pub fn gate_and_maybe_evict(log_fn: &mut dyn FnMut(&str)) -> (bool, DiskGateDiag) {
    let home = crate::config_file::expand_tilde("~");
    let obs = observe(&home);
    let refuse = decide(&obs, log_fn);
    let diag = DiskGateDiag {
        free_disk_gb: round1(obs.home_free_gb),
        home_write_probe_ok: obs.home_write_probe_ok,
        staging_free_gb: round1(obs.staging_free_gb),
        largest_pending_raw_dir_gb: round1(obs.largest_pending_gb),
    };
    (refuse, diag)
}

/// The probe half of [`gate_and_maybe_evict`], split out so tests can
/// point $HOME at a TempDir.
pub(crate) fn observe(home: &Path) -> GateObservation {
    let home_free_gb = free_gb(home);
    let home_write_probe_ok = write_probe_ok(home);
    let staging_root = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let staging_free_gb = free_gb(Path::new(&staging_root));
    let largest_pending_gb =
        largest_child_dir_gb(&Path::new(&staging_root).join("wisent_raw_pending"));
    GateObservation {
        home_free_gb,
        home_write_probe_ok,
        staging_free_gb,
        largest_pending_gb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide_lines(obs: &GateObservation) -> (bool, Vec<String>) {
        let mut lines = Vec::new();
        let refuse = decide(obs, &mut |line: &str| lines.push(line.to_string()));
        (refuse, lines)
    }

    #[test]
    fn decision_matrix() {
        let healthy = GateObservation {
            home_free_gb: 500.0,
            home_write_probe_ok: true,
            staging_free_gb: 200.0,
            largest_pending_gb: 50.0,
        };
        assert!(!decide_lines(&healthy).0);

        // $HOME write probe failure refuses outright.
        let (refuse, lines) = decide_lines(&GateObservation {
            home_write_probe_ok: false,
            ..healthy
        });
        assert!(refuse);
        assert!(
            lines[0].contains("$HOME write probe failed (~500.0 GB free)"),
            "{lines:?}"
        );

        // Staging free below the largest pending raw dir refuses.
        let (refuse, lines) = decide_lines(&GateObservation {
            staging_free_gb: 40.0,
            ..healthy
        });
        assert!(refuse);
        assert!(
            lines[0].contains("staging low (~40GB free < measured pending dir 50GB)"),
            "{lines:?}"
        );

        // Equal values do NOT refuse (Python `0 <= free < largest`).
        let (refuse, _) = decide_lines(&GateObservation {
            staging_free_gb: 50.0,
            ..healthy
        });
        assert!(!refuse);

        // Unreadable staging (-1.0) is not gated on.
        let (refuse, _) = decide_lines(&GateObservation {
            staging_free_gb: -1.0,
            ..healthy
        });
        assert!(!refuse);

        // Home probe failure dominates; the staging rule never fires.
        let (_, lines) = decide_lines(&GateObservation {
            home_write_probe_ok: false,
            staging_free_gb: 1.0,
            ..healthy
        });
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn dir_size_helpers_on_real_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![0u8; 2048]).unwrap();
        std::fs::create_dir(dir.path().join("big")).unwrap();
        std::fs::write(dir.path().join("big/b.bin"), vec![0u8; 4096]).unwrap();
        std::fs::create_dir(dir.path().join("small")).unwrap();
        std::fs::write(dir.path().join("small/c.bin"), vec![0u8; 1024]).unwrap();

        let gib = 1024f64.powi(3);
        assert_eq!(dir_size_gb(dir.path()), 7168.0 / gib);
        assert_eq!(largest_child_dir_gb(dir.path()), 4096.0 / gib);
        assert_eq!(dir_size_gb(Path::new("/nonexistent-wc-path")), 0.0);
        assert_eq!(largest_child_dir_gb(Path::new("/nonexistent-wc-path")), 0.0);
    }

    #[test]
    fn probes_on_tempdir_are_healthy() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_probe_ok(dir.path()));
        // The probe file is removed afterwards (delete=True parity).
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
        assert!(free_gb(dir.path()) > 0.0);
        assert_eq!(free_gb(Path::new("/nonexistent-wc-path")), -1.0);
    }

    #[test]
    fn gate_returns_diag_with_rounded_values() {
        let dir = tempfile::tempdir().unwrap();
        let obs = observe(dir.path());
        assert!(obs.home_write_probe_ok);
        let diag = DiskGateDiag {
            free_disk_gb: round1(obs.home_free_gb),
            home_write_probe_ok: true,
            staging_free_gb: round1(obs.staging_free_gb),
            largest_pending_raw_dir_gb: round1(obs.largest_pending_gb),
        };
        assert!(diag.free_disk_gb > 0.0);
        assert_eq!(diag.largest_pending_raw_dir_gb, 0.0);
    }
}
