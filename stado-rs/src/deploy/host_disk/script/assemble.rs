//! The builder that splices exactly the sections a scope reads into one
//! remote program, with the janitor's state and lock paths in place.

use super::*;

/// `$background`: the words that put a walk in the host's background band,
/// chosen by the host's own kernel at run time, since one script serves both.
pub fn background_section() -> String {
    format!(
        "if [ \"$(/usr/bin/uname -s 2>/dev/null || /bin/uname -s)\" = Darwin ]; then\n  \
         background='{}'\nelse\n  background='{}'\nfi\n",
        crate::deploy::host_scan::background_prefix(true),
        crate::deploy::host_scan::background_prefix(false),
    )
}

/// The refused list of one platform as one quoted, newline-separated word.
fn prune_word(darwin: bool) -> String {
    shlex_quote(
        &crate::providers::local::disk_cleanup::build_caches::privacy_protected_parts(darwin)
            .join("\n"),
    )
}

/// The remote program carrying exactly the sections SCOPE consumes, with the
/// janitor's state and lock paths in place.
///
/// Assembled from the section constants rather than from per-scope text: a
/// field two scopes share is produced by the same bytes in both, so the
/// cheap script cannot report a different `usage`, `state` or `snapshots`
/// than the full one. The sections are independent commands reading
/// independent sources, so dropping one cannot alter another's output.
pub fn remote_script_for(scope: DiskScope) -> String {
    let mut script = String::from("set -u\n");
    if scope == DiskScope::UsageOnly {
        script.push_str(DISK_USAGE_SECTION);
        script.push_str(VOLUMES_SECTION);
        return script;
    }
    if scope != DiskScope::StateOnly {
        script.push_str(DISK_USAGE_SECTION);
        script.push_str(VOLUMES_SECTION);
    }
    // Memory belongs to the state read, not to the filesystem measurement:
    // `host gates` splits its host reads into `UsageOnly` for free space and
    // `StateOnly` for the janitor, and memory was in neither, so a verdict
    // that carries a memory gate had nothing to fill it with unless the host's
    // agent was publishing. `vm_stat`, `sysctl` and one small state file are
    // what this adds to a read that is already running the janitor's.
    script.push_str(MEMORY_SECTION);
    script.push_str(CLEANUP_STATE_SECTION);
    let attributed = scope == DiskScope::Full || scope == DiskScope::FullWithoutCensus;
    if attributed {
        script.push_str(CLEANUP_LOCK_SECTION);
    }
    script.push_str(SNAPSHOT_SECTION);
    if attributed {
        // The walks of the whole home run in the host's background band, so
        // a report read on a loaded host adds to it least.
        script.push_str(&background_section());
        if scope == DiskScope::Full {
            script.push_str(BUILD_CACHE_SECTION);
        }
        script.push_str(INVENTORY_SECTION);
    }
    script
        .replace(
            "__CLONE_CONTAINER__",
            disk_cleanup::chromium_clones::CLONE_CONTAINER,
        )
        .replace(
            "__CLONE_ROOT__",
            disk_cleanup::chromium_clones::CLONE_ROOT_NAME,
        )
        .replace(
            STATE_PATH_MARK,
            &shlex_quote(&disk_cleanup::state_relative_path()),
        )
        .replace(
            LOCK_PATH_MARK,
            &shlex_quote(&disk_cleanup::lock_relative_path()),
        )
        .replace("__DARWIN_PRUNE__", &prune_word(true))
        .replace("__OTHER_PRUNE__", &prune_word(false))
}
