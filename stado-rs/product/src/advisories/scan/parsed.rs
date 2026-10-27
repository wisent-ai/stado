//! What the text of a Cargo lockfile resolves: its crates.io packages with
//! the packages that depend on each, and every package with its source.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};

/// The two ways a lockfile names the crates.io registry as a package's source.
const CRATES_IO_SOURCES: &[&str] = &[
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// One package a lockfile resolves from crates.io, and the packages of the
/// same lockfile that depend on it.
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub dependents: Vec<String>,
}

/// Every package a lockfile resolves, by name: each as `version` when it
/// comes from crates.io or a path, and as `version source` when it comes
/// from anywhere else, so a moved git revision reads as a change just as a
/// moved version does.
pub type Resolved = BTreeMap<String, BTreeSet<String>>;

/// One `[[package]]` of a lockfile.
struct Entry {
    name: String,
    version: String,
    source: Option<String>,
    dependencies: Vec<String>,
}

/// Every `[[package]]` of `text`; `label` names the lockfile in refusals.
fn entries(label: &str, text: &str) -> Result<Vec<Entry>> {
    let document: toml::Value =
        toml::from_str(text).with_context(|| format!("{label}: not a Cargo lockfile"))?;
    let listed = document
        .get("package")
        .and_then(toml::Value::as_array)
        .with_context(|| {
            format!(
                "{label}: not a Cargo lockfile: it holds no [[package]] list, and a lockfile \
                 always holds at least the workspace's own packages"
            )
        })?;
    listed
        .iter()
        .map(|entry| {
            let field = |name: &str| -> Result<String> {
                entry
                    .get(name)
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
                    .with_context(|| format!("{label}: a [[package]] has no {name}"))
            };
            Ok(Entry {
                name: field("name")?,
                version: field("version")?,
                source: entry
                    .get("source")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned),
                // Cargo leaves the key out for a package that depends on nothing.
                dependencies: entry
                    .get("dependencies")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// The package a lockfile's `dependencies` entry names, and the version it
/// names when it names one. Cargo writes `name` alone when the lock holds one
/// version of it, and `name version` (with its source when two sources share
/// the version) when it holds several.
fn depended(entry: &str) -> (String, String) {
    match entry.split_once(' ') {
        None => (entry.to_owned(), String::new()),
        Some((name, rest)) => match rest.split_once(' ') {
            None => (name.to_owned(), rest.to_owned()),
            Some((version, _source)) => (name.to_owned(), version.to_owned()),
        },
    }
}

/// The crates.io packages `text` resolves, in name and version order. A
/// package from a git source or a path is not in a registry's advisory
/// database and is left out.
pub fn packages(label: &str, text: &str) -> Result<Vec<Package>> {
    let mut dependents: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut found = Vec::new();
    for entry in entries(label, text)? {
        for dependency in &entry.dependencies {
            dependents
                .entry(depended(dependency))
                .or_default()
                .push(format!("{} {}", entry.name, entry.version));
        }
        if entry
            .source
            .as_deref()
            .is_some_and(|source| CRATES_IO_SOURCES.contains(&source))
        {
            found.push(Package {
                name: entry.name,
                version: entry.version,
                dependents: Vec::new(),
            });
        }
    }
    for package in &mut found {
        let mut named: Vec<String> = [package.version.clone(), String::new()]
            .into_iter()
            .filter_map(|at| dependents.get(&(package.name.clone(), at)))
            .flatten()
            .cloned()
            .collect();
        named.sort();
        named.dedup();
        package.dependents = named;
    }
    found.sort_by(|left, right| (&left.name, &left.version).cmp(&(&right.name, &right.version)));
    Ok(found)
}

/// Every package `text` resolves, from any source.
pub fn resolved(label: &str, text: &str) -> Result<Resolved> {
    let mut resolved = Resolved::new();
    for entry in entries(label, text)? {
        let at = match entry.source.as_deref() {
            Some(source) if !CRATES_IO_SOURCES.contains(&source) => {
                format!("{} {source}", entry.version)
            }
            Some(_) | None => entry.version,
        };
        resolved.entry(entry.name).or_default().insert(at);
    }
    Ok(resolved)
}
