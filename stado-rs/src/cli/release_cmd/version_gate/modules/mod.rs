//! Every `.rs` file under `src/` must be reachable from a crate root.
//!
//! A Rust source file that no `mod` declaration names is not a compile error:
//! it is not compiled at all, and `cargo check`, `cargo clippy` and a file
//! listing all look untouched. [`tree`] resolves every `mod` declaration from
//! `src/lib.rs` and each `[[bin]]` path in `Cargo.toml`; this reports any
//! `.rs` file the walk never reached.
//!
//! Files already unreachable are listed in a `--known` file, one path per
//! line with `# reason` beside it. That file is a ratchet: anything
//! unreachable and unlisted fails, and a listed path that no longer exists
//! fails too, because a stale entry would re-admit a real orphan later.
//! [`Outcome`] names the three results the command's exit status reports.

mod tree;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) use tree::{reached_modules, undeclared_files};

/// Every file reachable; an orphan or a stale entry; or the crate or the
/// known file could not be read at all.
pub(super) enum Outcome {
    Clean,
    Findings,
    Unreadable(String),
}

/// The canonical path of `path`. A path that does not exist keeps the
/// spelling it was given, so a stale `--known` entry is named as written;
/// any other failure is refused by name.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(format!("error: cannot resolve {}: {error}", path.display())),
    }
}

fn read_known(path: Option<&Path>, crate_dir: &Path) -> Result<Vec<String>, String> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate_dir.join(path)
    };
    let text = fs::read_to_string(&resolved)
        .map_err(|error| format!("error: cannot read {}: {error}", resolved.display()))?;
    Ok(text
        .lines()
        .map(|raw| match raw.split_once('#') {
            Some((entry, _reason)) => entry.trim().to_string(),
            None => raw.trim().to_string(),
        })
        .filter(|entry| !entry.is_empty())
        .collect())
}

/// Every `.rs` file under `directory`. A directory that cannot be listed is
/// the error, naming it: skipped, its files escaped the orphan check.
fn every_rs(directory: &Path, into: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("error: cannot list {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("error: cannot list {}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            every_rs(&path, into)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.insert(resolve(&path)?);
        }
    }
    Ok(())
}

/// What the check found, every path resolved.
struct Findings {
    roots: Vec<PathBuf>,
    every: BTreeSet<PathBuf>,
    seen: BTreeSet<PathBuf>,
    known: BTreeSet<PathBuf>,
}

fn find(crate_dir: &Path, source: &Path, known: Option<&Path>) -> Result<Findings, String> {
    let mut every = BTreeSet::new();
    every_rs(source, &mut every)?;
    let roots = tree::crate_roots(crate_dir)?
        .iter()
        .map(|root| resolve(root))
        .collect::<Result<Vec<PathBuf>, String>>()?;
    if roots.is_empty() {
        return Err(format!(
            "error: {} declares no crate root",
            crate_dir.display()
        ));
    }
    let seen = tree::reachable(&roots)?
        .iter()
        .map(|path| resolve(path))
        .collect::<Result<BTreeSet<PathBuf>, String>>()?;
    let known = read_known(known, crate_dir)?
        .iter()
        .map(|entry| resolve(&crate_dir.join(entry)))
        .collect::<Result<BTreeSet<PathBuf>, String>>()?;
    Ok(Findings {
        roots,
        every,
        seen,
        known,
    })
}

pub(super) fn check(crate_dir: &Path, known: Option<&Path>, as_json: bool) -> Outcome {
    let crate_dir = match resolve(crate_dir) {
        Ok(crate_dir) => crate_dir,
        Err(detail) => return Outcome::Unreadable(detail),
    };
    let source = crate_dir.join("src");
    if !source.is_dir() {
        return Outcome::Unreadable(format!("error: {} is not a directory", source.display()));
    }
    let Findings {
        roots,
        every,
        seen,
        known,
    } = match find(&crate_dir, &source, known) {
        Ok(findings) => findings,
        Err(detail) => return Outcome::Unreadable(detail),
    };
    let stale: Vec<&PathBuf> = known.iter().filter(|path| !every.contains(*path)).collect();
    let orphans: Vec<&PathBuf> = every
        .iter()
        .filter(|path| !seen.contains(*path) && !known.contains(*path))
        .collect();
    let reachable = every.iter().filter(|path| seen.contains(*path)).count();
    let relative = |path: &Path| match path.strip_prefix(&crate_dir) {
        Ok(inside) => inside.display().to_string(),
        Err(_outside_the_crate) => path.display().to_string(),
    };
    if as_json {
        let report = serde_json::json!({
            "crate": crate_dir.display().to_string(),
            "roots": roots.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            "files": every.len(),
            "reachable": reachable,
            "known": known.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            "stale": stale.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            "unreachable": orphans.iter().map(|path| relative(path)).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report serialises")
        );
    } else {
        println!("{} .rs files under {}", every.len(), relative(&source));
        let names: Vec<String> = roots.iter().map(|path| relative(path)).collect();
        println!("{} crate root(s): {}", roots.len(), names.join(", "));
        for path in &stale {
            println!("stale --known entry (no such file): {}", relative(path));
        }
        if orphans.is_empty() {
            println!("\nevery file is reachable from a crate root");
        } else {
            println!("\n{} file(s) no `mod` declaration reaches:", orphans.len());
            for path in &orphans {
                println!("  {}", relative(path));
            }
            println!("\nEach is compiled into nothing. Declare it, delete it, or record it\nin the --known file with the reason beside it.");
        }
    }
    if orphans.is_empty() && stale.is_empty() {
        Outcome::Clean
    } else {
        Outcome::Findings
    }
}
