//! Cleaner names, supported releases and retention floors shared by readers
//! and the registry validator. Scan roots describe scope, not removable bytes.

/// One cleaner the janitor can run, and what an operator needs to know before
/// declaring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanerDeclaration {
    /// The key under `targets[].disk_cleanup.cleaners`.
    pub name: &'static str,
    /// First release implementing this cleaner.
    pub since: &'static str,
    /// Home-relative default. An empty value requires the host's resolved
    /// build-cache or macOS container reading.
    pub default_root: &'static str,
    /// What it takes, in one clause an operator can act on.
    pub sweeps: &'static str,
    /// The lowest `min_age_seconds` a policy may declare for it, and the value
    /// a declaration that names none is written with.
    /// Existing per-cleaner retention floors. Lifecycle-owned cleaners prove
    /// terminal jobs, matching replicas or unreferenced versions instead.
    pub min_age_floor_seconds: i64,
    /// Extra version requirement for roots previously accepted but not consumed.
    pub root_override_since: Option<&'static str>,
}

/// Every cleaner, in the order the registry contract lists them.
pub const CLEANERS: &[CleanerDeclaration] = &[
    CleanerDeclaration {
        name: "backup_twins",
        since: "0.13.0",
        default_root: super::backup_twins::BACKUP_ROOT,
        sweeps: "same-disk replica objects whose primary copy is intact",
        min_age_floor_seconds: 0,
        root_override_since: None,
    },
    CleanerDeclaration {
        name: "build_caches",
        since: "0.9.5",
        default_root: "",
        sweeps: "directories carrying a build tool's own CACHEDIR.TAG",
        min_age_floor_seconds: 86_400,
        root_override_since: None,
    },
    CleanerDeclaration {
        name: "chromium_clones",
        since: "0.9.5",
        default_root: "",
        sweeps: "the operating system's per-launch code-signing clones",
        min_age_floor_seconds: 86_400,
        root_override_since: None,
    },
    CleanerDeclaration {
        name: "huggingface_cache",
        since: "0.9.5",
        default_root: ".cache/huggingface/hub",
        sweeps: "model blobs the hub can fetch again",
        min_age_floor_seconds: 3_600,
        root_override_since: Some("0.17.0"),
    },
    CleanerDeclaration {
        name: "queue_workdirs",
        since: "0.12.0",
        default_root: ".stado/work/jobs",
        sweeps: "work trees of jobs the queue reports terminal",
        min_age_floor_seconds: 0,
        root_override_since: Some("0.17.0"),
    },
    CleanerDeclaration {
        name: "release_store",
        since: "0.15.26",
        default_root: super::release_store::RELEASES_ROOT,
        sweeps: "published release versions past the rollback ladder this host keeps",
        min_age_floor_seconds: 0,
        root_override_since: None,
    },
    CleanerDeclaration {
        name: "weles_recordings",
        since: "0.9.5",
        default_root: "weles/recordings",
        sweeps: "recordings admitted by the declared age and upload-proof policy",
        min_age_floor_seconds: 86_400,
        root_override_since: None,
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
        let version = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    };
    match (parse(installed), parse(required)) {
        (Some(installed), Some(required)) => installed >= required,
        _ => false,
    }
}
