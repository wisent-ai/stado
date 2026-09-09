//! Every cleaner this binary implements, in one declaration.
//!
//! Three readers needed this list and each carried its own: the registry
//! contract's allowed-name array, `fleet_shape`'s table of the release each
//! cleaner first shipped in, and — until this module — nothing at all on the
//! side that has to tell an operator which mechanism could reach the bytes
//! filling a disk. On 2026-09-09 `charless-mac-mini` sat 7.6 GiB below its
//! declared target with 52.4 GiB in `~/.stado/local-storage` and 10.4 GiB in
//! `~/.stado/local-backup`, and `stado space report` said no declared stage
//! looked there. Both statements were true and the useful one was missing:
//! this binary implements `release_store` and `backup_twins`, which sweep
//! exactly those two roots, and that host declared neither.
//!
//! So the catalogue is the answer to "what could hold this disk", and the
//! command that arms one (`stado space cleaners declare`) and the report that
//! measures coverage read the same rows.

/// One cleaner the janitor can run, and what an operator needs to know before
/// declaring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanerDeclaration {
    /// The key under `targets[].disk_cleanup.cleaners`.
    pub name: &'static str,
    /// The first released `stado` that accepts this name in a registry policy.
    ///
    /// Declaring a name an older binary does not know made that host read
    /// `cleaners: null` and switch off every cleaner it was already running,
    /// which is why this is part of the declaration rather than folklore.
    pub since: &'static str,
    /// Where it sweeps when the policy names no root, relative to the
    /// account's home. `None` means the cleaner resolves its own root: the
    /// account's home for build caches, the operating system's temporary
    /// container for Chromium clones.
    pub default_root: &'static str,
    /// What it takes, in one clause an operator can act on.
    pub sweeps: &'static str,
    /// The lowest `min_age_seconds` a policy may declare for it, and the value
    /// a declaration that names none is written with.
    ///
    /// The floor is per cleaner because what makes an item safe to take is per
    /// cleaner. A build tree or a Chromium code-sign clone is only known to be
    /// idle by age, so both need a day; the Hugging Face cache is
    /// content-addressed and re-fetchable within the hour. `queue_workdirs`,
    /// `backup_twins` and `release_store` have no floor and are not weaker for
    /// it: a workdir is safe when its job is terminal, a replica when the
    /// primary holds those exact bytes, a version when nobody still names it,
    /// and all three are proved in the pass that deletes. A floor would only
    /// subtract — the workdirs that took the always-on mac under its watermark
    /// were minutes old, and so was the replica that gave it back 17 GiB.
    pub min_age_floor_seconds: i64,
}

/// Every cleaner, in the order the registry contract lists them.
pub const CLEANERS: &[CleanerDeclaration] = &[
    CleanerDeclaration {
        name: "backup_twins",
        since: "0.13.0",
        default_root: ".stado/local-backup",
        sweeps: "same-disk replica objects whose primary copy is intact",
        min_age_floor_seconds: 0,
    },
    CleanerDeclaration {
        name: "build_caches",
        since: "0.9.5",
        default_root: "",
        sweeps: "directories carrying a build tool's own CACHEDIR.TAG",
        min_age_floor_seconds: 86_400,
    },
    CleanerDeclaration {
        name: "chromium_clones",
        since: "0.9.5",
        default_root: "",
        sweeps: "the operating system's per-launch code-signing clones",
        min_age_floor_seconds: 86_400,
    },
    CleanerDeclaration {
        name: "huggingface_cache",
        since: "0.9.5",
        default_root: ".cache/huggingface",
        sweeps: "model blobs the hub can fetch again",
        min_age_floor_seconds: 3_600,
    },
    CleanerDeclaration {
        name: "queue_workdirs",
        since: "0.12.0",
        default_root: ".stado/work/jobs",
        sweeps: "work trees of jobs the queue reports terminal",
        min_age_floor_seconds: 0,
    },
    CleanerDeclaration {
        name: "release_store",
        since: "0.15.26",
        default_root: ".stado/local-storage/ecosystem/releases",
        sweeps: "published release versions past the rollback ladder this host keeps",
        min_age_floor_seconds: 0,
    },
    CleanerDeclaration {
        name: "weles_recordings",
        since: "0.9.5",
        default_root: ".stado/weles/recordings",
        sweeps: "session recordings already uploaded to the object store",
        min_age_floor_seconds: 86_400,
    },
];

/// One cleaner by name.
pub fn cleaner(name: &str) -> Option<&'static CleanerDeclaration> {
    CLEANERS.iter().find(|entry| entry.name == name)
}

/// Every name, for a refusal that lists what an operator may declare.
pub fn names() -> Vec<&'static str> {
    CLEANERS.iter().map(|entry| entry.name).collect()
}

/// Whether `installed` is at least `required`, comparing `X.Y.Z` numerically.
///
/// An unreadable or absent version answers false, so an unknown host is
/// treated as unable to take a new cleaner rather than assumed able: being
/// wrong that way costs a note, and being wrong the other way switches off
/// every cleaner that host already runs.
pub fn version_at_least(installed: &str, required: &str) -> bool {
    let parse = |value: &str| -> Option<(u64, u64, u64)> {
        let bare = value.trim().trim_start_matches('v');
        let bare = bare.split('-').next().unwrap_or_default();
        let mut parts = bare.split('.').map(|part| part.parse::<u64>().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    };
    match (parse(installed), parse(required)) {
        (Some(installed), Some(required)) => installed >= required,
        _ => false,
    }
}
