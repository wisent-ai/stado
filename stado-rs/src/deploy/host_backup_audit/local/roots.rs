//! Whether the two stores can be compared at all.

use std::path::Path;

/// Why the replica and the primary store cannot be compared, if they cannot.
/// Both are resolved through every symbolic link first: a replica that is,
/// or lies inside, or contains the primary store would be classified as its
/// own twin, and a reclaim would then delete the only copy.
pub(super) fn overlapping(backup: &Path, primary: &Path) -> Option<String> {
    let resolve = |label: &str, path: &Path| {
        std::fs::canonicalize(path).map_err(|error| {
            format!(
                "{label} root {} cannot be resolved: {error}",
                path.display()
            )
        })
    };
    let (backup, primary) = match (
        resolve("local-backup", backup),
        resolve("local-storage", primary),
    ) {
        (Ok(backup), Ok(primary)) => (backup, primary),
        (Err(detail), _) | (_, Err(detail)) => return Some(detail),
    };
    (backup.starts_with(&primary) || primary.starts_with(&backup)).then(|| {
        format!(
            "local-backup root {} and local-storage root {} are the same tree or one lies inside the other; refusing to compare them",
            backup.display(),
            primary.display()
        )
    })
}
