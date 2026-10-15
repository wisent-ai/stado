//! The lines of one checkout that reach another product.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::Command;

use super::channel::{Channel, Names};
use crate::common::capture;

/// Where the product's own tests live: what they call is not what the
/// product needs to run.
pub(super) const TESTS: &str = "tests/";
/// Prose, not code: a product named in documentation is not called.
const PROSE_EXTENSION: &str = "md";
/// The forge's automation, a directory at any depth: what it runs builds
/// and checks the product; it is not the product running.
const FORGE_AUTOMATION: &str = ".github";
/// Files git reads about the checkout itself: they name paths, never call
/// anything.
const GIT_METADATA: &[&str] = &[".gitignore", ".gitattributes", ".gitmodules"];
/// The extension of an example a person copies: what it shows is read, if
/// at all, by code that is read on its own.
const EXAMPLE_EXTENSION: &str = "example";
/// The records package managers write of the packages they resolved: a
/// package named like a product is not that product, and a package the
/// product imports is read where its code imports it.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "Package.resolved",
    "Gemfile.lock",
    "poetry.lock",
    "uv.lock",
];
/// A shell script, where a program is started by naming it first.
const SHELL_EXTENSION: &str = "sh";
/// What a line opens with when it is a comment, in the languages the
/// products are written in.
const COMMENT_OPENERS: &[&str] = &["//", "#", "*", "--", "<!--"];

/// One line that reaches another product.
#[derive(Clone, Debug, Serialize)]
pub struct Site {
    /// The repository the line is in.
    pub repository: String,
    /// The file, relative to its checkout.
    pub file: String,
    /// The line number, counted from one.
    pub line: usize,
    /// How the line reaches the product.
    pub channel: Channel,
}

/// The object type git records for a submodule: the entry is another
/// repository's commit, a directory in the working tree, not a file of this
/// checkout.
const SUBMODULE_OBJECT: &str = "commit";

/// Whether the tracked file at `path` is something the product never runs:
/// its tests, prose, the forge's automation, git's metadata, an example to
/// copy, or a package manager's lockfile.
fn never_run(path: &str) -> bool {
    let file = Path::new(path);
    let named = |names: &[&str]| {
        file.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| names.contains(&name))
    };
    path.starts_with(TESTS)
        || file
            .components()
            .any(|component| component.as_os_str() == FORGE_AUTOMATION)
        || named(GIT_METADATA)
        || named(LOCKFILES)
        || file
            .extension()
            .is_some_and(|extension| extension == PROSE_EXTENSION || extension == EXAMPLE_EXTENSION)
}

/// Every tracked file of `checkout` that is product code, by path. A
/// submodule is another repository's code and is not read as this one's.
fn tracked(checkout: &Path) -> Result<Vec<String>> {
    let output = capture(Command::new("git").arg("-C").arg(checkout).args([
        "ls-files",
        "-z",
        "--format=%(objecttype)%x09%(path)",
    ]))?;
    if !output.status.success() {
        bail!(
            "git ls-files in {} failed ({}): {}",
            checkout.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listing = String::from_utf8(output.stdout)
        .with_context(|| format!("git ls-files in {} is not UTF-8", checkout.display()))?;
    let mut files = Vec::new();
    for entry in listing.split('\0').filter(|entry| !entry.is_empty()) {
        let (object, path) = entry.split_once('\t').with_context(|| {
            format!(
                "git ls-files in {} answered {entry:?}, which is not \
                 `<object type>\\t<path>`",
                checkout.display()
            )
        })?;
        if object == SUBMODULE_OBJECT || never_run(path) {
            continue;
        }
        files.push(path.to_owned());
    }
    files.dedup();
    Ok(files)
}

/// Every line of `repository`'s checkout that reaches one of `others`, by the
/// other product's id. A file that is not UTF-8 text holds no source line; a
/// tracked file deleted from the working tree is no longer the product's
/// code; any other file that cannot be read is refused by name.
pub fn sites(
    repository: &str,
    checkout: &Path,
    others: &[&Names],
) -> Result<BTreeMap<String, Vec<Site>>> {
    let mut found: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    for file in tracked(checkout)? {
        let path = checkout.join(&file);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let shell = Path::new(&file)
            .extension()
            .is_some_and(|extension| extension == SHELL_EXTENSION);
        for (number, line) in (NonZeroUsize::MIN.get()..).zip(text.lines()) {
            let line = line.trim();
            if COMMENT_OPENERS
                .iter()
                .any(|opener| line.starts_with(opener))
            {
                continue;
            }
            for other in others {
                if let Some(channel) = other.channel(line, shell) {
                    found.entry(other.id.clone()).or_default().push(Site {
                        repository: repository.to_owned(),
                        file: file.clone(),
                        line: number,
                        channel,
                    });
                }
            }
        }
    }
    Ok(found)
}
