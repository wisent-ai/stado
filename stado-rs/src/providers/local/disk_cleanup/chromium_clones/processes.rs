//! The one process-table snapshot a pass takes, and the live-process gate
//! that reads it.

use std::path::Path;
use std::process::Command;

/// Every live process's argv, taken ONCE for the whole pass.
///
/// One snapshot, not one probe per candidate, for the reason
/// [`crate::deploy::host_reclaim`] states about its own stages: a per-candidate
/// `ps | grep <path>` matches the grep's own argv and reports every path as
/// held. `None` means the process table could not be read at all, which this
/// cleaner treats as a refusal to delete rather than as an empty table.
pub(super) fn process_snapshot() -> Option<String> {
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
pub(super) fn held(snapshot: &str, path: &Path) -> bool {
    snapshot.contains(path.to_string_lossy().as_ref())
}
