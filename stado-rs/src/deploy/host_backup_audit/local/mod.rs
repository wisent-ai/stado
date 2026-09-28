//! `stado host backup-audit-local`: the host half of `stado host
//! backup-audit`, run by the host's own Stado. It reads both stores on the
//! host, so no object body crosses the network, and prints the marker lines
//! [`super::parse_output`] folds into the report.
//!
//! The pass runs to completion: every same-size pair is hashed, so a
//! read-only pass proves every twin it can see and a reclaim deletes exactly
//! the twins it proved in the same run.

mod files;
mod roots;

use std::io::Write;
use std::path::{Path, PathBuf};

use files::{identity, metadata_path, relative, sha256, walk};

/// What the operator's side asked this host to do.
pub struct LocalPass {
    pub backup: PathBuf,
    pub primary: PathBuf,
    pub namespace: String,
    pub objects: Vec<String>,
    pub inventory_namespaces: Vec<String>,
    pub reclaim: bool,
    pub apply: bool,
}

/// A comma-separated list of hex-encoded UTF-8 names, as
/// [`super::remote_script`] writes it; empty entries are skipped.
pub fn hex_list(text: &str) -> Result<Vec<String>, String> {
    text.split(',')
        .filter(|value| !value.is_empty())
        .map(|value| {
            hex::decode(value)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or_else(|| format!("{value:?} is not hex-encoded UTF-8"))
        })
        .collect()
}

fn one_line(text: &str) -> String {
    text.replace(['\t', '\n'], " ")
}

fn emit(line: String) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
}

/// Size metadata of every object under each namespace of the replica; false
/// when anything could not be read.
fn inventory(pass: &LocalPass) -> bool {
    let mut complete = true;
    let mut fail = |scope: &str, detail: String| {
        complete = false;
        emit(format!(
            "STADO_BACKUP_AUDIT_UNAVAILABLE\t{scope} inventory: {}",
            one_line(&detail)
        ));
    };
    for scope in &pass.inventory_namespaces {
        let scope_root = pass.backup.join("ecosystem").join(scope);
        match std::fs::symlink_metadata(&scope_root) {
            Err(error) => {
                fail(scope, error.to_string());
                continue;
            }
            Ok(entry) if !entry.is_dir() => {
                fail(scope, "backup namespace root is not a directory".into());
                continue;
            }
            Ok(_) => {}
        }
        let problems = std::cell::RefCell::new(Vec::<String>::new());
        let mut rows: Vec<String> = Vec::new();
        walk(
            &scope_root,
            &mut |path| {
                let name = relative(path, &pass.backup);
                let backup = identity(path, false);
                let primary = identity(&pass.primary.join(&name), false);
                let primary_meta = identity(&metadata_path(&pass.primary, &name), false);
                let backup_meta = identity(&metadata_path(&pass.backup, &name), false);
                rows.push(format!(
                    "STADO_BACKUP_INVENTORY_OBJECT\t{}\t{}\t{}\t\t{}\t{}\t\t{}\t{}\t\t{}\t{}\t",
                    hex::encode(&name),
                    primary.state,
                    primary.size,
                    backup.state,
                    backup.size,
                    primary_meta.state,
                    primary_meta.size,
                    backup_meta.state,
                    backup_meta.size,
                ));
                if backup.state != "present" {
                    problems
                        .borrow_mut()
                        .push(format!("backup object is {}: {name}", backup.state));
                }
                for (label, state) in [
                    ("local-storage object", primary.state),
                    ("local-storage metadata", primary_meta.state),
                    ("local-backup metadata", backup_meta.state),
                ] {
                    if state == "unreadable" {
                        problems
                            .borrow_mut()
                            .push(format!("{label} is unreadable: {name}"));
                    }
                }
            },
            &mut |path| {
                problems
                    .borrow_mut()
                    .push(format!("non-directory entry omitted: {}", path.display()))
            },
            &mut |detail| problems.borrow_mut().push(detail),
        );
        rows.into_iter().for_each(emit);
        for problem in problems.into_inner() {
            fail(scope, problem);
        }
    }
    complete
}

fn exact(pass: &LocalPass) {
    for name in &pass.objects {
        let path = Path::new(name);
        let normal = path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
        if path.is_absolute() || !normal || name.ends_with('/') || name.contains("//") {
            emit("STADO_BACKUP_AUDIT_UNAVAILABLE\tinvalid exact object path".into());
            continue;
        }
        let sides = [
            identity(&pass.primary.join(name), true),
            identity(&pass.backup.join(name), true),
            identity(&metadata_path(&pass.primary, name), true),
            identity(&metadata_path(&pass.backup, name), true),
        ];
        let fields: Vec<String> = sides
            .iter()
            .map(|side| format!("{}\t{}\t{}", side.state, side.size, side.digest))
            .collect();
        emit(format!(
            "STADO_BACKUP_OBJECT\t{name}\t{}",
            fields.join("\t")
        ));
    }
}

/// Classify every replica file against its primary address and, under
/// reclaim with apply, unlink the twins this pass just proved. False when any
/// part of the replica could not be read: the pass then reports each failure
/// and emits no completion marker, so nothing downstream treats a partial
/// walk as the whole replica.
fn classify(pass: &LocalPass) -> bool {
    let (mut deleted, mut deleted_bytes, mut refused) = (0u64, 0u64, 0u64);
    let failures = std::cell::RefCell::new(Vec::<String>::new());
    walk(
        &pass.backup,
        &mut |path| {
            let name = relative(path, &pass.backup);
            let candidate = if name.starts_with("ecosystem/") {
                pass.primary.join(&name)
            } else {
                pass.primary
                    .join("ecosystem")
                    .join(&pass.namespace)
                    .join(&name)
            };
            let backup = identity(path, false);
            if backup.state == "absent" || backup.state == "unreadable" {
                failures
                    .borrow_mut()
                    .push(format!("{name}: local-backup entry is {}", backup.state));
                return;
            }
            let size = backup.size.clone();
            let primary = identity(&candidate, false);
            let verdict = match primary.state {
                "absent" | "unreadable" => super::ABSENT,
                _ if backup.state != "present"
                    || primary.state != "present"
                    || primary.size != size =>
                {
                    super::DIFFERS
                }
                _ => match (sha256(path), sha256(&candidate)) {
                    (Ok(left), Ok(right)) if left == right => super::TWIN,
                    (Ok(_), Ok(_)) => super::DIFFERS,
                    _ => super::SAME_SIZE_UNPROVEN,
                },
            };
            emit(format!("STADO_BACKUP_AUDIT\t{verdict}\t{size}\t{name}"));
            if verdict != super::TWIN || !pass.reclaim {
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
                    deleted += 1;
                    deleted_bytes += size.parse::<u64>().unwrap_or_default();
                    emit(format!("STADO_BACKUP_RECLAIM\tdeleted\t{size}\t{name}"));
                }
                Err(_) => {
                    refused += 1;
                    emit(format!(
                        "STADO_BACKUP_RECLAIM\tdelete_failed\t{size}\t{name}"
                    ));
                }
            }
        },
        &mut |path| {
            failures.borrow_mut().push(format!(
                "{}: a linked directory was not walked",
                path.display()
            ))
        },
        &mut |detail| failures.borrow_mut().push(detail),
    );
    emit(format!(
        "STADO_BACKUP_RECLAIM_END\t{deleted}\t{deleted_bytes}\t{refused}"
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

/// The whole host half, in the order the operator side reads it. False when
/// the pass was refused or could not read everything it walked, so the
/// caller exits nonzero and the remote program stops before its pruning
/// epilogue.
pub fn run(pass: &LocalPass) -> bool {
    if let Some(detail) = roots::overlapping(&pass.backup, &pass.primary) {
        emit(format!(
            "STADO_BACKUP_AUDIT_UNAVAILABLE\t{}",
            one_line(&detail)
        ));
        return false;
    }
    roots::emit_namespaces("local_storage", &pass.primary);
    roots::emit_namespaces("local_backup", &pass.backup);
    let complete = inventory(pass);
    if !pass.inventory_namespaces.is_empty() && pass.objects.is_empty() {
        if complete {
            emit("STADO_BACKUP_AUDIT_END\tinventory".into());
        }
        return complete;
    }
    if !pass.objects.is_empty() {
        exact(pass);
        if complete {
            emit("STADO_BACKUP_AUDIT_END\texact".into());
        }
        return complete;
    }
    classify(pass)
}
