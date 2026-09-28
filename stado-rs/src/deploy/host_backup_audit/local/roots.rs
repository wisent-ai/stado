//! The two stores as wholes: whether they can be compared at all, and which
//! namespaces each holds.

use std::path::Path;

use super::{emit, one_line};

/// Every namespace directory under a store's `ecosystem/`, or the reason it
/// could not be listed completely.
pub(super) fn emit_namespaces(label: &str, root: &Path) {
    let listed = std::fs::read_dir(root.join("ecosystem")).and_then(|entries| {
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    });
    match listed {
        Ok(names) => {
            for name in &names {
                emit(format!(
                    "STADO_BACKUP_NAMESPACE\t{label}\t{}",
                    hex::encode(name)
                ));
            }
            emit(format!(
                "STADO_BACKUP_NAMESPACES_END\t{label}\t{}",
                names.len()
            ));
        }
        Err(error) => emit(format!(
            "STADO_BACKUP_NAMESPACES_ERROR\t{label}\t{}",
            one_line(&error.to_string())
        )),
    }
}

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
