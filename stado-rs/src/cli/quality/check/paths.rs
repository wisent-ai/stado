//! The path half of the module check: a `crate::a::b` path in a module the
//! crate reaches, one of whose segments names a module file the tree holds
//! but no `mod` declaration reaches. That file is not compiled, so the path
//! resolves to nothing and no build of the commit compiles — a commit that
//! added `rulings/guard/mod.rs` and code calling `crate::rulings::guard`
//! without `mod guard;` in `rulings/mod.rs` is the shape. It is read from the
//! files alone, without name resolution: a segment naming an item, or a
//! module the tree holds no file for, is left to the build.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
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
    let mut listings = Listings::default();
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
                if let Some(held) = held_as_spelled(&source, &candidates, &mut listings)? {
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

/// The names each directory holds, listed once per check: every `crate::`
/// path of every reached file walks the same directories, so a listing is
/// kept for the check's length instead of read again for each candidate.
#[derive(Default)]
struct Listings(BTreeMap<PathBuf, BTreeSet<OsString>>);

impl Listings {
    /// The names `directory` holds, listing it the first time it is asked.
    fn of(&mut self, directory: &Path) -> Result<&BTreeSet<OsString>, String> {
        if !self.0.contains_key(directory) {
            let entries = std::fs::read_dir(directory)
                .map_err(|error| format!("error: cannot list {}: {error}", directory.display()))?;
            let mut names = BTreeSet::new();
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!("error: cannot list {}: {error}", directory.display())
                })?;
                names.insert(entry.file_name());
            }
            self.0.insert(directory.to_path_buf(), names);
        }
        match self.0.get(directory) {
            Some(names) => Ok(names),
            None => Err(format!(
                "error: the listing of {} was not kept",
                directory.display()
            )),
        }
    }
}

/// The first of `candidates` the tree holds under exactly that spelling, every
/// component below `source` looked up in its directory's listing. On a
/// case-insensitive filesystem `copy/Endpoint.rs` opens `copy/endpoint.rs`,
/// which is the module `endpoint`, so a path naming the item `Endpoint` must
/// not read as naming an undeclared module file.
fn held_as_spelled<'a>(
    source: &Path,
    candidates: &'a [PathBuf],
    listings: &mut Listings,
) -> Result<Option<&'a PathBuf>, String> {
    for candidate in candidates.iter().filter(|candidate| candidate.is_file()) {
        let Ok(relative) = candidate.strip_prefix(source) else {
            continue;
        };
        let mut parent = source.to_path_buf();
        let mut spelled = true;
        for component in relative.components() {
            if !listings.of(&parent)?.contains(component.as_os_str()) {
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
