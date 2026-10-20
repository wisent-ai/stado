//! The path half of the module check: a `crate::a::b` path in a module the
//! crate reaches, one of whose segments names a module file the tree holds
//! but no `mod` declaration reaches. That file is not compiled, so the path
//! resolves to nothing and no build of the commit compiles — a commit that
//! added `rulings/guard/mod.rs` and code calling `crate::rulings::guard`
//! without `mod guard;` in `rulings/mod.rs` is the shape. It is read from the
//! files alone, without name resolution: a segment naming an item, or a
//! module the tree holds no file for, is left to the build.
//!
//! This module also reclaims abandoned exported-source run paths, leaving
//! unreadable or unrecognized entries intact with a diagnostic.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::cli::release_cmd::reached_modules;

/// `crate::a::b…`, capturing the segments after `crate`.
static CRATE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bcrate(?P<segments>(?:::\w+)+)").expect("static"));

/// Every `crate::` path a reached module of the crate at `crate_dir` writes
/// whose walk through the declared modules meets a module file the tree holds
/// but no declaration reaches, one sentence each.
pub(super) fn unreached(crate_dir: &Path) -> Result<Vec<String>, String> {
    let reached = reached_modules(crate_dir)?;
    let source = crate_dir.join("src");
    let mut found = BTreeSet::new();
    for file in &reached {
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("error: cannot read {}: {error}", file.display()))?;
        for path in CRATE_PATH.captures_iter(&text) {
            let mut directory = source.clone();
            let mut named = String::from("crate");
            for segment in path["segments"]
                .split("::")
                .filter(|segment| !segment.is_empty())
            {
                named = format!("{named}::{segment}");
                let candidates = [
                    directory.join(format!("{segment}.rs")),
                    directory.join(segment).join("mod.rs"),
                ];
                if candidates
                    .iter()
                    .any(|candidate| reached.contains(candidate))
                {
                    directory = directory.join(segment);
                    continue;
                }
                if let Some(held) = held_as_spelled(&source, &candidates)? {
                    found.insert(format!(
                        "{} uses `{named}`, and the tree holds {}, which no `mod {segment};` reaches",
                        file.display(),
                        held.display()
                    ));
                }
                break;
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// The first of `candidates` the tree holds under exactly that spelling, every
/// component below `source` listed by its directory. On a case-insensitive
/// filesystem `copy/Endpoint.rs` opens `copy/endpoint.rs`, which is the module
/// `endpoint`, so a path naming the item `Endpoint` must not read as naming
/// an undeclared module file.
fn held_as_spelled<'a>(
    source: &Path,
    candidates: &'a [PathBuf],
) -> Result<Option<&'a PathBuf>, String> {
    for candidate in candidates.iter().filter(|candidate| candidate.is_file()) {
        let Ok(relative) = candidate.strip_prefix(source) else {
            continue;
        };
        let mut parent = source.to_path_buf();
        let mut spelled = true;
        for component in relative.components() {
            let entries = std::fs::read_dir(&parent)
                .map_err(|error| format!("error: cannot list {}: {error}", parent.display()))?;
            let mut listed = false;
            for entry in entries {
                let entry = entry
                    .map_err(|error| format!("error: cannot list {}: {error}", parent.display()))?;
                listed |= entry.file_name() == component.as_os_str();
            }
            if !listed {
                spelled = false;
                break;
            }
            parent.push(component);
        }
        if spelled {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// Reclaim run areas left by terminated checks. The revision being checked
/// supplies this repository's commit ID width, without assuming Git's object
/// format. Unrecognized or unreadable entries are reported and left intact.
pub(super) fn remove_abandoned(runs: &Path, current_revision: &str) {
    let entries = match std::fs::read_dir(runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "stado quality check: cannot list run areas in {}: {error}",
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
                    "stado quality check: cannot read a run area in {}: {error}",
                    runs.display()
                );
                continue;
            }
        };
        let name = entry.file_name();
        let Some((pid, revision)) = name.to_str().and_then(|name| name.split_once('-')) else {
            eprintln!("stado quality check: cannot identify the owning process of {}: expected <pid>-<revision>", entry.path().display());
            continue;
        };
        let pid = match pid.parse::<std::num::NonZeroU32>() {
            Ok(pid) => pid,
            Err(error) => {
                eprintln!(
                    "stado quality check: cannot parse the owning process of {}: {error}",
                    entry.path().display()
                );
                continue;
            }
        };
        if revision.len() != current_revision.len()
            || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            eprintln!(
                "stado quality check: cannot identify the revision of {}: expected a full hexadecimal commit id",
                entry.path().display()
            );
            continue;
        }
        let pid = match i32::try_from(pid.get()) {
            Ok(pid) => pid,
            Err(error) => {
                eprintln!(
                    "stado quality check: owning process of {} is outside the platform PID range: {error}",
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
                        "stado quality check: cannot remove the abandoned run {}: {error}",
                        entry.path().display()
                    );
                }
            }
            Err(error) => eprintln!(
                "stado quality check: cannot inspect the owning process of {}: {error}",
                entry.path().display()
            ),
        }
    }
}
