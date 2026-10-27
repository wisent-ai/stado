//! Admission-only disk-space diagnostics for the local agent loop.
//!
//! Port of `stado/providers/local/disk/gate.py`.
//!
//! All cleanup is owned by the registry-authorized policy engine. This
//! module only probes the filesystems and refuses new work while pressure is
//! unresolved.

use std::io::Write;
use std::path::Path;

use crate::providers::local::host_memory::constants::GIB;

/// Bytes as GiB, as measured; a report rounds where it prints.
fn gib(bytes: u64) -> f64 {
    bytes as f64 / GIB as f64
}

/// The free space of the volume holding `path` in GiB, as the disk-full rule
/// reads a volume (the bytes an unprivileged writer may still take), or why
/// it could not be measured. The sentinel -1.0 this used to return was
/// published as a free-space reading and switched staging backpressure off.
/// The cause is published with the capacity diagnostics, which carry no local
/// path, so it names none; a caller that logs it beside a path adds the path
/// itself.
pub fn free_gb(path: &Path) -> Result<f64, String> {
    let volume = crate::providers::local::space_reclaim::rule::read_volume(path)
        .map_err(|error| format!("the volume cannot be measured: {error}"))?;
    Ok(volume.free_bytes as f64 / GIB as f64)
}

/// Whether the agent can create, flush, and remove a file on this volume, or
/// the operating system's reason it could not: a refused write with no cause
/// sent the operator to guess between a full disk, permissions and a
/// read-only mount. The reason is published with the capacity diagnostics,
/// which carry no local path, so it names the fleet volume rather than `path`.
pub fn write_probe(path: &Path) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::with_prefix_in(".wc-disk-probe-", path)
        .map_err(|error| format!("a probe file cannot be created on the fleet volume: {error}"))?;
    let written = file.write_all(b"x")
        .map_err(|error| format!("a probe file on the fleet volume cannot be written: {error}"))
        .and_then(|()| file.flush()
            .map_err(|error| format!("a probe file on the fleet volume cannot be flushed: {error}")))
        .and_then(|()| file.as_file().sync_all()
            .map_err(|error| format!("a probe file on the fleet volume cannot be synced: {error}")));
    let removed = file.close()
        .map_err(|error| format!("a probe file on the fleet volume cannot be removed: {error}"));
    match (written, removed) {
        (Err(mut cause), Err(removal)) => {
            cause.push_str("; ");
            cause.push_str(&removal);
            Err(cause)
        }
        (Err(cause), _) | (_, Err(cause)) => Err(cause),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// Measure a complete pending tree without following symbolic links.
/// An unreadable entry invalidates the measurement, not just that entry.
fn dir_size_bytes(path: &Path) -> Result<u64, String> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| format!("pending directory cannot be listed: {error}"))?;
    let bytes = entries.map(|entry| {
        let entry = entry.map_err(|error| format!("pending directory entry cannot be read: {error}"))?;
        let child = entry.path();
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|error| format!("pending directory entry cannot be inspected: {error}"))?;
        if metadata.is_dir() {
            dir_size_bytes(&child).map(u128::from)
        } else {
            Ok(u128::from(metadata.len()))
        }
    }).sum::<Result<u128, String>>()?;
    u64::try_from(bytes)
        .map_err(|error| format!("pending directory byte count cannot be represented: {error}"))
}

/// The largest complete child-tree measurement. An absent pending root or
/// no child directories means nothing pending; unreadable trees are errors.
pub fn largest_child_dir_gb(path: &Path) -> Result<Option<f64>, String> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("pending directory root cannot be listed: {error}")),
    };
    let mut largest = None;
    for entry in entries {
        let entry = entry.map_err(|error| format!("pending directory entry cannot be read: {error}"))?;
        let child = entry.path();
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|error| format!("pending directory entry cannot be inspected: {error}"))?;
        if metadata.is_dir() {
            let bytes = dir_size_bytes(&child)?;
            largest = Some(match largest {
                Some(previous) => std::cmp::max(previous, bytes),
                None => bytes,
            });
        }
    }
    Ok(largest.map(gib))
}

/// Raw probe values feeding the admission decision. An unreadable
/// measurement retains its cause instead of standing in for spare room.
#[derive(Debug, Clone)]
pub struct GateObservation {
    pub home_free_gb: Result<f64, String>,
    pub home_write_probe: Result<(), String>,
    pub staging_free_gb: Result<f64, String>,
    /// Complete largest pending-tree reading, or why it could not be read.
    pub largest_pending_gb: Result<Option<f64>, String>,
}

/// Pure admission decision (Python's two `refuse` branches). Returns
/// `refuse` and emits the same log lines through `log_fn`. Staging that
/// cannot be measured refuses new work as staging that is low does: whether
/// the next pending directory fits cannot be told.
pub fn decide(obs: &GateObservation, log_fn: &mut dyn FnMut(&str)) -> bool {
    let mut refuse = obs.home_write_probe.is_err();
    if let Err(probe) = &obs.home_write_probe {
        match &obs.home_free_gb {
            Ok(free) => log_fn(&format!(
                "fleet volume write probe failed ({probe}; {free:.1} GiB free); refusing new work this tick"
            )),
            Err(cause) => log_fn(&format!(
                "fleet volume write probe failed ({probe}; {cause}); refusing new work this tick"
            )),
        }
    }
    // Staging-pressure backpressure is also admission-only.
    if !refuse {
        match (&obs.staging_free_gb, &obs.largest_pending_gb) {
            (Ok(free), Ok(Some(pending))) if free < pending => {
                log_fn(&format!(
                    "staging low ({free:.1} GiB free < measured pending dir {pending:.1} GiB); refusing new work"
                ));
                refuse = true;
            }
            (Ok(_), Err(cause)) => {
                log_fn(&format!("pending staging size unread ({cause}); refusing new work"));
                refuse = true;
            }
            (Ok(_), Ok(_)) => {}
            (Err(cause), _) => {
                log_fn(&format!("staging free space unread ({cause}); refusing new work"));
                refuse = true;
            }
        }
    }
    refuse
}

/// Path-free disk diagnostics (Python's `diag` dict). Aggregate
/// measurements only; never publish local filesystem paths.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskGateDiag {
    /// The fleet volume's free GiB, or why it could not be measured.
    pub free_disk_gb: Result<f64, String>,
    /// Whether the fleet volume took a probe write, or why it did not.
    pub home_write_probe: Result<(), String>,
    /// The staging volume's free GiB, or why it could not be measured.
    pub staging_free_gb: Result<f64, String>,
    /// Complete reading, `Ok(None)` when nothing is pending.
    pub largest_pending_raw_dir_gb: Result<Option<f64>, String>,
    /// The same measured refusal sent to the log, for the claiming surfaces.
    pub refusal_detail: Option<String>,
}

/// Return admission refusal and path-free disk diagnostics.
/// This function never deletes data. The policy engine runs separately before admission.
/// `free_disk_gb` and the write probe measure the volume the fleet writes
/// to: the declared work root, or the home.
pub fn measure_admission(log_fn: &mut dyn FnMut(&str)) -> (bool, DiskGateDiag) {
    let volume = crate::providers::local::work_base::measured_volume();
    let obs = observe(&volume);
    let mut detail = String::new();
    let refuse = decide(&obs, &mut |message| {
        log_fn(message);
        if !detail.is_empty() {
            detail.push_str("; ");
        }
        detail.push_str(message);
    });
    let diag = DiskGateDiag {
        free_disk_gb: obs.home_free_gb,
        home_write_probe: obs.home_write_probe,
        staging_free_gb: obs.staging_free_gb,
        largest_pending_raw_dir_gb: obs.largest_pending_gb,
        refusal_detail: refuse.then_some(detail),
    };
    (refuse, diag)
}

/// Probe the selected fleet volume and this process's staging volume for
/// [`measure_admission`].
pub(crate) fn observe(home: &Path) -> GateObservation {
    let home_free_gb = free_gb(home);
    let home_write_probe = write_probe(home);
    // The system's temporary directory: TMPDIR when it names one, else the
    // platform's own, as every other temporary file of this process is put.
    let staging_root = std::env::temp_dir();
    let staging_free_gb = free_gb(&staging_root);
    let largest_pending_gb = largest_child_dir_gb(&staging_root.join("wisent_raw_pending"));
    GateObservation {
        home_free_gb,
        home_write_probe,
        staging_free_gb,
        largest_pending_gb,
    }
}
