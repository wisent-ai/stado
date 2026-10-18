//! The replica classification: every file of the backup against its primary
//! address, and, under reclaim with apply, the unlink of each twin proved.

use std::cell::RefCell;
use std::path::Path;

use super::files::{identity, relative, sha256, walk, FileState};
use super::{emit, one_line, LocalPass};

/// Why a same-size pair could not be proven twins: each copy whose content
/// could not be read, with its cause; empty when both were read.
fn unproven(left: std::io::Result<String>, right: std::io::Result<String>) -> String {
    [("local-backup", left), ("local-storage", right)]
        .into_iter()
        .filter_map(|(side, read)| match read {
            Ok(_) => None,
            Err(error) => Some(format!("{side} copy unread: {error}")),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Classify every replica file against its primary address and, under
/// reclaim with apply, unlink the twins this pass just proved. False when any
/// part of the replica could not be read: the pass then reports each failure
/// with its cause and emits no completion marker, so nothing downstream
/// treats a partial walk as the whole replica. A row's last field says why a
/// same-size pair stayed unproven, or why a twin could not be deleted.
pub(super) fn classify(pass: &LocalPass) -> bool {
    // Each deleted twin's byte count, and each refused deletion's cause.
    let mut deleted: Vec<u64> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    let failures = RefCell::new(Vec::<String>::new());
    walk(
        &pass.backup,
        &mut |path| {
            let name = match relative(path, &pass.backup) {
                Ok(name) => name,
                Err(cause) => {
                    failures.borrow_mut().push(cause);
                    return;
                }
            };
            let candidate = if name.starts_with("ecosystem/") {
                pass.primary.join(&name)
            } else {
                pass.primary
                    .join("ecosystem")
                    .join(&pass.namespace)
                    .join(&name)
            };
            let backup = identity(path, false);
            let (false, Some(bytes)) = (backup.state.missing_or_unreadable(), backup.bytes) else {
                failures.borrow_mut().push(format!(
                    "{name}: local-backup entry is {}",
                    backup.described()
                ));
                return;
            };
            let size = backup.size();
            let primary = identity(&candidate, false);
            let (verdict, why) = match primary.state {
                state if state.missing_or_unreadable() => (super::super::ABSENT, String::new()),
                _ if backup.state != FileState::Present
                    || primary.state != FileState::Present
                    || primary.bytes != backup.bytes =>
                {
                    (super::super::DIFFERS, String::new())
                }
                _ => match (sha256(path), sha256(&candidate)) {
                    (Ok(left), Ok(right)) if left == right => (super::super::TWIN, String::new()),
                    (Ok(_), Ok(_)) => (super::super::DIFFERS, String::new()),
                    (left, right) => (super::super::SAME_SIZE_UNPROVEN, unproven(left, right)),
                },
            };
            emit(format!(
                "STADO_BACKUP_AUDIT\t{verdict}\t{size}\t{name}\t{}",
                one_line(&why)
            ));
            if verdict != super::super::TWIN || !pass.reclaim {
                return;
            }
            if !pass.apply {
                emit(format!(
                    "STADO_BACKUP_RECLAIM\twould_delete\t{size}\t{name}"
                ));
                return;
            }
            match std::fs::remove_file(path) {
                Ok(()) => {
                    deleted.push(bytes);
                    emit(format!("STADO_BACKUP_RECLAIM\tdeleted\t{size}\t{name}"));
                }
                Err(error) => {
                    let cause = one_line(&error.to_string());
                    emit(format!(
                        "STADO_BACKUP_RECLAIM\tdelete_failed\t{size}\t{name}\t{cause}"
                    ));
                    refused.push(cause);
                }
            }
        },
        &mut |path: &Path| {
            failures.borrow_mut().push(format!(
                "{}: a linked directory was not walked",
                path.display()
            ))
        },
        &mut |detail| failures.borrow_mut().push(detail),
    );
    emit(format!(
        "STADO_BACKUP_RECLAIM_END\t{}\t{}\t{}",
        deleted.len(),
        deleted.iter().sum::<u64>(),
        refused.len()
    ));
    let failures = failures.into_inner();
    for failure in &failures {
        emit(format!(
            "STADO_BACKUP_AUDIT_UNAVAILABLE\tlocal-backup walk: {}",
            one_line(failure)
        ));
    }
    if !failures.is_empty() {
        return false;
    }
    emit("STADO_BACKUP_AUDIT_END\tclassified".into());
    true
}
