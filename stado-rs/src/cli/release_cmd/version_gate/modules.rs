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

use std::collections::{BTreeMap, BTreeSet};
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

/// `mod name {`: an inline module. A `mod` declaration inside it resolves
/// one directory deeper, under the inline module's name, as rustc resolves
/// it: `mod outer { mod inner; }` in `a/mod.rs` names `a/outer/inner.rs`.
static INLINE_MODULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[^\S\n]*(?:pub(?:\([^)]*\))?[^\S\n]+)?mod[^\S\n]+(?:r#)?(\w+)\s*\{")
        .expect("static")
});

/// One `mod name;` declaration: where its line starts, its name, and the
/// inline modules it stands in, outermost first.
struct Declaration {
    start: usize,
    name: String,
    within: Vec<String>,
}

impl Declaration {
    /// The directory the declaration resolves in, given its file's.
    fn directory(&self, file_directory: &Path) -> PathBuf {
        self.within
            .iter()
            .fold(file_directory.to_path_buf(), |directory, inline| {
                directory.join(inline)
            })
    }
}

/// Every `mod name;` of `text` with the inline modules around it. Braces are
/// counted outside comments, string and character literals, so an inline
/// module ends at its own closing brace.
fn declarations(text: &str) -> Vec<Declaration> {
    let opens: BTreeMap<usize, String> = INLINE_MODULE
        .captures_iter(text)
        .filter_map(|found| Some((found.get(0)?.end() - 1, found.get(1)?.as_str().to_string())))
        .collect();
    let mut pending = MOD_DECLARATION
        .captures_iter(text)
        .filter_map(|found| Some((found.get(0)?.start(), found.get(1)?.as_str().to_string())))
        .peekable();
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut index = 0;
    loop {
        while let Some((start, name)) = pending.next_if(|(start, _)| *start <= index) {
            found.push(Declaration {
                start,
                name,
                within: stack.iter().map(|(name, _)| name.clone()).collect(),
            });
        }
        let Some(&byte) = bytes.get(index) else {
            break;
        };
        let after = |from: usize, pattern: &str| {
            text.get(from..)
                .and_then(|rest| rest.find(pattern))
                .map(|at| from + at)
        };
        index = match byte {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                after(index, "\n").unwrap_or(bytes.len())
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                after(index + 2, "*/").map_or(bytes.len(), |end| end + 2)
            }
            b'"' => {
                let mut at = index + 1;
                while let Some(&inside) = bytes.get(at) {
                    match inside {
                        b'\\' => at += 2,
                        b'"' => break,
                        _ => at += 1,
                    }
                }
                at + 1
            }
            b'r' if index
                .checked_sub(1)
                .and_then(|before| bytes.get(before))
                .is_none_or(|before| {
                    (!before.is_ascii_alphanumeric() && *before != b'_') || *before == b'b'
                })
                && matches!(bytes.get(index + 1), Some(b'"' | b'#')) =>
            {
                let hashes = bytes[index + 1..]
                    .iter()
                    .take_while(|byte| **byte == b'#')
                    .count();
                if bytes.get(index + 1 + hashes) == Some(&b'"') {
                    let closing = format!("\"{}", "#".repeat(hashes));
                    after(index + 2 + hashes, &closing)
                        .map_or(bytes.len(), |end| end + closing.len())
                } else {
                    index + 1
                }
            }
            b'\'' if bytes.get(index + 1) == Some(&b'\\') => {
                after(index + 2, "'").map_or(bytes.len(), |end| end + 1)
            }
            b'\'' if bytes.get(index + 2) == Some(&b'\'') => index + 3,
            b'{' => {
                if let Some(name) = opens.get(&index) {
                    stack.push((name.clone(), depth));
                }
                depth += 1;
                index + 1
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if stack.last().is_some_and(|(_, opened)| *opened == depth) {
                    stack.pop();
                }
                index + 1
            }
            _ => index + 1,
        };
    }
    found
}

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
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate_dir.join(path)
    };
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

/// Every module file reachable from `roots` through `mod` declarations. A
/// module file that cannot be read is the error, naming it: read as empty,
/// its children looked unreachable and the gate reported orphans that are not.
fn reachable(roots: &[PathBuf]) -> Result<BTreeSet<PathBuf>, String> {
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
            .map_err(|error| format!("error: cannot read {}: {error}", current.display()))?;
        for declaration in declarations(&text) {
            let directory = declaration.directory(&directory);
            let name = &declaration.name;
            for candidate in [
                directory.join(format!("{name}.rs")),
                directory.join(name).join("mod.rs"),
            ] {
                if candidate.is_file() && !seen.contains(&candidate) {
                    stack.push(candidate);
                }
            }
        }
    }
    Ok(seen)
}

/// The file a `#[path = "…"]` attribute on the line before a declaration
/// names, relative to the declaring file's directory, when there is one.
static PATH_ATTRIBUTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"#\[path\s*=\s*"(?P<path>[^"]+)"\s*\]"#).expect("static"));

/// The crate's roots: `src/lib.rs`, every `[[bin]]` path, and `src/main.rs`.
fn every_root(crate_dir: &Path) -> Vec<PathBuf> {
    let mut roots = crate_roots(crate_dir);
    let main = crate_dir.join("src/main.rs");
    if main.is_file() && !roots.contains(&main) {
        roots.push(main);
    }
    roots
}

/// Every module file the crate at `crate_dir` reaches from its roots through
/// `mod` declarations, read from the files alone.
pub(crate) fn reached_modules(crate_dir: &Path) -> Result<BTreeSet<PathBuf>, String> {
    reachable(&every_root(crate_dir))
}

/// Every `mod name;` a module reachable from the crate at `crate_dir`
/// declares that names no file in the tree — neither `<name>.rs` nor
/// `<name>/mod.rs` where it resolves, nor the file a `#[path]` attribute
/// gives it — one sentence each. Read from the files alone, compiling
/// nothing: a commit that deleted a module file another still declares, or
/// declared one it never added, fails here before anything builds on it.
pub(crate) fn undeclared_files(crate_dir: &Path) -> Result<Vec<String>, String> {
    let roots = every_root(crate_dir);
    let root_set: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    let mut missing = Vec::new();
    for file in reachable(&roots)? {
        let text = fs::read_to_string(&file)
            .map_err(|error| format!("error: cannot read {}: {error}", file.display()))?;
        let own_directory = file
            .parent()
            .ok_or_else(|| format!("error: {} has no directory", file.display()))?;
        let directory = child_directory(&file, &root_set);
        for declaration in declarations(&text) {
            let name = &declaration.name;
            let line_before = text[..declaration.start].trim_end().lines().last();
            let attributed = line_before
                .and_then(|line| PATH_ATTRIBUTE.captures(line))
                .and_then(|attribute| attribute.name("path").map(|path| path.as_str().to_string()));
            let candidates: Vec<PathBuf> = match attributed {
                Some(path) => vec![own_directory.join(path)],
                None => {
                    let directory = declaration.directory(&directory);
                    vec![
                        directory.join(format!("{name}.rs")),
                        directory.join(name).join("mod.rs"),
                    ]
                }
            };
            if !candidates.iter().any(|candidate| candidate.is_file()) {
                missing.push(format!(
                    "{} declares `mod {};`, and the tree holds none of {}",
                    file.display(),
                    name,
                    candidates
                        .iter()
                        .map(|candidate| candidate.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" or ")
                ));
            }
        }
    }
    Ok(missing)
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
            into.insert(resolve(&path));
        }
    }
    Ok(())
}

pub(super) fn check(crate_dir: &Path, known: Option<&Path>, as_json: bool) -> Outcome {
    let crate_dir = resolve(crate_dir);
    let source = crate_dir.join("src");
    if !source.is_dir() {
        return Outcome::Unreadable(format!("error: {} is not a directory", source.display()));
    }
    let mut every = BTreeSet::new();
    if let Err(detail) = every_rs(&source, &mut every) {
        return Outcome::Unreadable(detail);
    }
    let roots: Vec<PathBuf> = crate_roots(&crate_dir)
        .iter()
        .map(|root| resolve(root))
        .collect();
    if roots.is_empty() {
        return Outcome::Unreadable(format!(
            "error: {} declares no crate root",
            crate_dir.display()
        ));
    }
    let seen: BTreeSet<PathBuf> = match reachable(&roots) {
        Ok(seen) => seen.iter().map(|path| resolve(path)).collect(),
        Err(detail) => return Outcome::Unreadable(detail),
    };
    let known: BTreeSet<PathBuf> = match read_known(known, &crate_dir) {
        Ok(entries) => entries
            .iter()
            .map(|entry| resolve(&crate_dir.join(entry)))
            .collect(),
        Err(detail) => return Outcome::Unreadable(detail),
    };
    let stale: Vec<&PathBuf> = known.iter().filter(|path| !every.contains(*path)).collect();
    let orphans: Vec<&PathBuf> = every
        .iter()
        .filter(|path| !seen.contains(*path) && !known.contains(*path))
        .collect();
    let relative = |path: &Path| {
        path.strip_prefix(&crate_dir)
            .unwrap_or(path)
            .display()
            .to_string()
    };
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
