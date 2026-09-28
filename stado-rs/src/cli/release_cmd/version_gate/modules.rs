//! Every `.rs` file under `src/` must be reachable from a crate root.
//!
//! A Rust source file that no `mod` declaration names is not a compile error:
//! it is not compiled at all, and `cargo check`, `cargo clippy` and a file
//! listing all look untouched. This resolves every `mod` declaration from
//! `src/lib.rs` and each `[[bin]]` path in `Cargo.toml`, then reports any
//! `.rs` file the walk never reached.
//!
//! Files already unreachable are listed in a `--known` file, one path per
//! line with `# reason` beside it. That file is a ratchet: anything
//! unreachable and unlisted fails, and a listed path that no longer exists
//! fails too, because a stale entry would re-admit a real orphan later.
//! [`Outcome`] names the three results the command's exit status reports.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// `mod name;` / `pub mod name;` / `pub(crate) mod name;` and the raw
/// identifier `mod r#box;`. A `mod name {` block declares its contents inline
/// and resolves to no file, so it is not matched.
static MOD_DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[^\S\n]*(?:pub(?:\([^)]*\))?[^\S\n]+)?mod[^\S\n]+(?:r#)?(\w+)[^\S\n]*;")
        .expect("static")
});

/// `path = "src/bin/whatever.rs"` in Cargo.toml: every `[[bin]]` is a root.
static CARGO_BIN_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^path\s*=\s*"(src/[^"]+\.rs)""#).expect("static"));

/// Every file reachable; an orphan or a stale entry; or the crate or the
/// known file could not be read at all.
pub(super) enum Outcome {
    Clean,
    Findings,
    Unreadable(String),
}

fn resolve(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn read_known(path: Option<&Path>, crate_dir: &Path) -> Result<Vec<String>, String> {
    let Some(path) = path else { return Ok(Vec::new()) };
    let resolved = if path.is_absolute() { path.to_path_buf() } else { crate_dir.join(path) };
    let text = fs::read_to_string(&resolved)
        .map_err(|_| format!("error: {} is not a file", resolved.display()))?;
    Ok(text
        .lines()
        .map(|raw| raw.split('#').next().unwrap_or("").trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect())
}

fn crate_roots(crate_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let lib = crate_dir.join("src/lib.rs");
    if lib.is_file() {
        roots.push(lib);
    }
    let text = fs::read(crate_dir.join("Cargo.toml"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    for found in CARGO_BIN_PATH.captures_iter(&text) {
        let candidate = crate_dir.join(&found[1]);
        if candidate.is_file() {
            roots.push(candidate);
        }
    }
    roots
}

/// Where a file's `mod` declarations resolve: `foo/mod.rs` and a crate root
/// own their own directory; `foo/bar.rs` owns `foo/bar/`.
fn child_directory(path: &Path, roots: &BTreeSet<PathBuf>) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("/"));
    if path.file_name().is_some_and(|name| name == "mod.rs") || roots.contains(path) {
        return parent.to_path_buf();
    }
    parent.join(path.file_stem().unwrap_or_default())
}

fn reachable(roots: &[PathBuf]) -> BTreeSet<PathBuf> {
    let root_set: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    let mut seen = BTreeSet::new();
    let mut stack = roots.to_vec();
    while let Some(current) = stack.pop() {
        if seen.contains(&current) || !current.is_file() {
            continue;
        }
        seen.insert(current.clone());
        let directory = child_directory(&current, &root_set);
        let text = fs::read(&current)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        for found in MOD_DECLARATION.captures_iter(&text) {
            let name = &found[1];
            for candidate in [directory.join(format!("{name}.rs")), directory.join(name).join("mod.rs")] {
                if candidate.is_file() && !seen.contains(&candidate) {
                    stack.push(candidate);
                }
            }
        }
    }
    seen
}

fn every_rs(directory: &Path, into: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            every_rs(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.insert(resolve(&path));
        }
    }
}

pub(super) fn check(crate_dir: &Path, known: Option<&Path>, as_json: bool) -> Outcome {
    let crate_dir = resolve(crate_dir);
    let source = crate_dir.join("src");
    if !source.is_dir() {
        return Outcome::Unreadable(format!("error: {} is not a directory", source.display()));
    }
    let mut every = BTreeSet::new();
    every_rs(&source, &mut every);
    let roots: Vec<PathBuf> = crate_roots(&crate_dir).iter().map(|root| resolve(root)).collect();
    if roots.is_empty() {
        return Outcome::Unreadable(format!("error: {} declares no crate root", crate_dir.display()));
    }
    let seen: BTreeSet<PathBuf> = reachable(&roots).iter().map(|path| resolve(path)).collect();
    let known: BTreeSet<PathBuf> = match read_known(known, &crate_dir) {
        Ok(entries) => entries.iter().map(|entry| resolve(&crate_dir.join(entry))).collect(),
        Err(detail) => return Outcome::Unreadable(detail),
    };
    let stale: Vec<&PathBuf> = known.iter().filter(|path| !every.contains(*path)).collect();
    let orphans: Vec<&PathBuf> =
        every.iter().filter(|path| !seen.contains(*path) && !known.contains(*path)).collect();
    let relative = |path: &Path| path.strip_prefix(&crate_dir).unwrap_or(path).display().to_string();
    if as_json {
        let report = serde_json::json!({
            "crate": crate_dir.display().to_string(),
            "roots": roots.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            "files": every.len(),
            "reachable": every.len() as i64 - orphans.len() as i64 - known.len() as i64,
            "known": known.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            "stale": stale.iter().map(|path| relative(path)).collect::<Vec<_>>(),
            "unreachable": orphans.iter().map(|path| relative(path)).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&report).expect("report serialises"));
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
