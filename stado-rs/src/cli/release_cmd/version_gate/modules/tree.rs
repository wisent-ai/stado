//! The module tree of a crate, read from its files alone: its roots, every
//! `mod name;` each reachable file declares, and the files those name.
//!
//! Each file is parsed as Rust (`syn`), so a `mod` inside a comment or a
//! string is not a declaration, an inline `mod outer { mod inner; }` resolves
//! one directory deeper as rustc resolves it, and a `#[path = "…"]`
//! attribute names the file it gives its module. A file that is not Rust is
//! refused by name: skipped, its children would read as orphans.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::ext::IdentExt;

/// One `mod name;` declaration: its name, the inline modules it stands in
/// (outermost first), and the file a `#[path]` attribute gives it.
struct Declaration {
    name: String,
    within: Vec<String>,
    path: Option<String>,
}

impl Declaration {
    /// The files the declaration may resolve to: the `#[path]` file, relative
    /// to the declaring file's own directory, or `<name>.rs` and
    /// `<name>/mod.rs` where its module's children live.
    fn candidates(&self, own_directory: &Path, child_directory: &Path) -> Vec<PathBuf> {
        if let Some(path) = &self.path {
            return vec![own_directory.join(path)];
        }
        let directory = self
            .within
            .iter()
            .fold(child_directory.to_path_buf(), |directory, inline| {
                directory.join(inline)
            });
        vec![
            directory.join(format!("{}.rs", self.name)),
            directory.join(&self.name).join("mod.rs"),
        ]
    }
}

/// The string a `#[path = "…"]` attribute gives, when one is there.
fn path_attribute(attributes: &[syn::Attribute]) -> Option<String> {
    attributes
        .iter()
        .find_map(|attribute| match &attribute.meta {
            syn::Meta::NameValue(pair) if pair.path.is_ident("path") => match &pair.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
}

fn collect(items: &[syn::Item], within: &[String], into: &mut Vec<Declaration>) {
    for item in items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        let name = module.ident.unraw().to_string();
        match &module.content {
            None => into.push(Declaration {
                name,
                within: within.to_vec(),
                path: path_attribute(&module.attrs),
            }),
            Some((_, inner)) => {
                let mut deeper = within.to_vec();
                deeper.push(name);
                collect(inner, &deeper, into);
            }
        }
    }
}

/// Every `mod name;` of the file at `file`.
fn declarations(file: &Path) -> Result<Vec<Declaration>, String> {
    let text = fs::read_to_string(file)
        .map_err(|error| format!("error: cannot read {}: {error}", file.display()))?;
    let parsed = syn::parse_file(&text)
        .map_err(|error| format!("error: cannot parse {} as Rust: {error}", file.display()))?;
    let mut found = Vec::new();
    collect(&parsed.items, &[], &mut found);
    Ok(found)
}

/// The `[[bin]]` targets of a Cargo manifest that name their path.
#[derive(serde::Deserialize)]
struct Manifest {
    bin: Option<Vec<Binary>>,
}

#[derive(serde::Deserialize)]
struct Binary {
    path: Option<String>,
}

/// `src/lib.rs` and every `[[bin]]` path the crate's `Cargo.toml` names. A
/// manifest that cannot be read or parsed is refused by name: read as none,
/// every binary's modules would read as orphans.
pub(super) fn crate_roots(crate_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    let lib = crate_dir.join("src/lib.rs");
    if lib.is_file() {
        roots.push(lib);
    }
    let manifest_path = crate_dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("error: cannot read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&text)
        .map_err(|error| format!("error: cannot parse {}: {error}", manifest_path.display()))?;
    for binary in manifest.bin.into_iter().flatten() {
        let Some(path) = binary.path else {
            continue;
        };
        let candidate = crate_dir.join(path);
        if candidate.is_file() {
            roots.push(candidate);
        }
    }
    Ok(roots)
}

/// The crate's roots: `src/lib.rs`, every `[[bin]]` path, and `src/main.rs`.
fn every_root(crate_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = crate_roots(crate_dir)?;
    let main = crate_dir.join("src/main.rs");
    if main.is_file() && !roots.contains(&main) {
        roots.push(main);
    }
    Ok(roots)
}

/// Where a file's `mod` declarations resolve: `foo/mod.rs` and a crate root
/// own their own directory; `foo/bar.rs` owns `foo/bar/`.
fn child_directory(path: &Path, roots: &BTreeSet<PathBuf>) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("error: {} has no directory", path.display()))?;
    if path.file_name().is_some_and(|name| name == "mod.rs") || roots.contains(path) {
        return Ok(parent.to_path_buf());
    }
    let stem = path
        .file_stem()
        .ok_or_else(|| format!("error: {} has no file name", path.display()))?;
    Ok(parent.join(stem))
}

/// Every module file reachable from `roots` through `mod` declarations. A
/// module file that cannot be read or parsed is the error, naming it.
pub(super) fn reachable(roots: &[PathBuf]) -> Result<BTreeSet<PathBuf>, String> {
    let root_set: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    let mut seen = BTreeSet::new();
    let mut stack = roots.to_vec();
    while let Some(current) = stack.pop() {
        if seen.contains(&current) || !current.is_file() {
            continue;
        }
        seen.insert(current.clone());
        let own_directory = current
            .parent()
            .ok_or_else(|| format!("error: {} has no directory", current.display()))?;
        let children = child_directory(&current, &root_set)?;
        for declaration in declarations(&current)? {
            for candidate in declaration.candidates(own_directory, &children) {
                if candidate.is_file() && !seen.contains(&candidate) {
                    stack.push(candidate);
                }
            }
        }
    }
    Ok(seen)
}

/// Every module file the crate at `crate_dir` reaches from its roots through
/// `mod` declarations, read from the files alone.
pub(crate) fn reached_modules(crate_dir: &Path) -> Result<BTreeSet<PathBuf>, String> {
    reachable(&every_root(crate_dir)?)
}

/// Every `mod name;` a module reachable from the crate at `crate_dir`
/// declares that names no file in the tree — neither `<name>.rs` nor
/// `<name>/mod.rs` where it resolves, nor the file a `#[path]` attribute
/// gives it — one sentence each. Read from the files alone, compiling
/// nothing: a commit that deleted a module file another still declares, or
/// declared one it never added, fails here before anything builds on it.
pub(crate) fn undeclared_files(crate_dir: &Path) -> Result<Vec<String>, String> {
    let roots = every_root(crate_dir)?;
    let root_set: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    let mut missing = Vec::new();
    for file in reachable(&roots)? {
        let own_directory = file
            .parent()
            .ok_or_else(|| format!("error: {} has no directory", file.display()))?;
        let children = child_directory(&file, &root_set)?;
        for declaration in declarations(&file)? {
            let candidates = declaration.candidates(own_directory, &children);
            if !candidates.iter().any(|candidate| candidate.is_file()) {
                missing.push(format!(
                    "{} declares `mod {};`, and the tree holds none of {}",
                    file.display(),
                    declaration.name,
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
