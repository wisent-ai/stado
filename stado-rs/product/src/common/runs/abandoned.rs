//! Run areas named `<pid>-<revision>` whose creating process is gone: a
//! command that exports a revision into such an area and is killed leaves it
//! behind, and the next run of the same command under the same parent removes
//! it before creating its own. An entry whose name, process or revision
//! cannot be read is reported and left intact.

use std::path::Path;

/// Remove every run area under `runs` named `<pid>-<revision>` whose process
/// no longer runs. `current_revision` supplies the width of a revision in
/// this repository, so no object format is assumed; `what` names the command
/// in each diagnostic.
pub fn reclaim_abandoned(runs: &Path, current_revision: &str, what: &str) {
    let entries = match std::fs::read_dir(runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "{what}: cannot list run areas in {}: {error}",
                runs.display()
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "{what}: cannot read a run area in {}: {error}",
                    runs.display()
                );
                continue;
            }
        };
        let name = entry.file_name();
        let Some((pid, revision)) = name.to_str().and_then(|name| name.split_once('-')) else {
            eprintln!(
                "{what}: cannot identify the owning process of {}: expected <pid>-<revision>",
                entry.path().display()
            );
            continue;
        };
        let pid = match pid.parse::<std::num::NonZeroU32>() {
            Ok(pid) => pid,
            Err(error) => {
                eprintln!(
                    "{what}: cannot parse the owning process of {}: {error}",
                    entry.path().display()
                );
                continue;
            }
        };
        if revision.len() != current_revision.len()
            || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            eprintln!(
                "{what}: cannot identify the revision of {}: expected a full hexadecimal commit id",
                entry.path().display()
            );
            continue;
        }
        let pid = match i32::try_from(pid.get()) {
            Ok(pid) => pid,
            Err(error) => {
                eprintln!(
                    "{what}: owning process of {} is outside the platform PID range: {error}",
                    entry.path().display()
                );
                continue;
            }
        };
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) => {}
            Err(nix::errno::Errno::ESRCH) => {
                if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                    eprintln!(
                        "{what}: cannot remove the abandoned run {}: {error}",
                        entry.path().display()
                    );
                }
            }
            Err(error) => eprintln!(
                "{what}: cannot inspect the owning process of {}: {error}",
                entry.path().display()
            ),
        }
    }
}
