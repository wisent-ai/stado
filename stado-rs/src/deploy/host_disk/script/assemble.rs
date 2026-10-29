//! The builder that splices exactly the sections a scope reads into one
//! remote program, with the janitor's state and lock paths in place.

use super::*;

/// `$background`: the words that put a walk in the host's background band,
/// chosen by the host's own kernel at run time, since one script serves both.
/// Every walk sends its errors to `/dev/null`, so a band program the host
/// does not have would empty every walk without a word; the script ends
/// first, naming the program, and the read fails with that sentence.
pub fn background_section() -> String {
    format!(
        "if [ \"$(/usr/bin/uname -s 2>/dev/null || /bin/uname -s)\" = Darwin ]; then\n  \
         background='{}'\nelse\n  background='{}'\nfi\n\
         band_program=${{background%% *}}\n\
         command -v \"$band_program\" >/dev/null 2>&1 || {{ missing=$?; printf '%s\\n' \
         \"the background band program $band_program is not on this host, so no walk can run\" \
         >&2; exit \"$missing\"; }}\n",
        crate::deploy::host_scan::background_prefix(true),
        crate::deploy::host_scan::background_prefix(false),
    )
}

/// The refused list of one platform as one quoted, newline-separated word,
/// walking the consent-gated folders in `declared`.
fn prune_word(darwin: bool, declared: &std::collections::BTreeSet<String>) -> String {
    shlex_quote(
        &crate::providers::local::space_reclaim::build_caches::privacy_protected_parts(
            darwin, declared,
        )
        .join("\n"),
    )
}

/// The remote program carrying exactly the sections SCOPE consumes, with the
/// janitor's state and lock paths in place, the target's declared work
/// root (`targets[].work_root`) measured beside its home, and its home walks
/// refusing the privacy list less the folders `targets[].privacy_grants`
/// declares for the program macOS holds responsible for the script (this
/// executable here, the SSH session program elsewhere). A grants entry that
/// cannot be read is the error, naming it.
///
/// Assembled from the section constants rather than from per-scope text: a
/// field two scopes share is produced by the same bytes in both, so the
/// cheap script cannot report a different `usage`, `state` or `snapshots`
/// than the full one. The sections are independent commands reading
/// independent sources, so dropping one cannot alter another's output.
pub fn remote_script_for(scope: DiskScope, target: &ComputeTarget) -> Result<String, DeployError> {
    let this_host = host_channel::target_is_this_host(target);
    let declared = target
        .privacy_grants()
        .map(|grants| {
            crate::targets::folders_for_script(&grants, this_host, &crate::config_file::home_dir())
        })
        .map_err(|error| DeployError(error.to_string()))?;
    // A host that declares no work root writes only under its home, so the
    // home's volume is the one measured.
    let work_root = match target.work_root.as_deref() {
        Some(root) => shlex_quote(root),
        None => String::new(),
    };
    let mut script = String::from("set -u\n");
    if scope == DiskScope::UsageOnly {
        script.push_str(DISK_USAGE_SECTION);
        script.push_str(VOLUMES_SECTION);
        return Ok(script.replace(WORK_ROOT_MARK, &work_root));
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
    Ok(script
        .replace(WORK_ROOT_MARK, &work_root)
        .replace(
            "__CLONE_CONTAINER__",
            space_reclaim::chromium_clones::CLONE_CONTAINER,
        )
        .replace(
            "__CLONE_ROOT__",
            space_reclaim::chromium_clones::CLONE_ROOT_NAME,
        )
        .replace(
            STATE_PATH_MARK,
            &shlex_quote(&space_reclaim::state_relative_path()),
        )
        .replace(
            LOCK_PATH_MARK,
            &shlex_quote(&space_reclaim::lock_relative_path()),
        )
        .replace("__DARWIN_PRUNE__", &prune_word(true, &declared))
        .replace("__OTHER_PRUNE__", &prune_word(false, &declared)))
}
