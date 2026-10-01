//! An application's public surface, read statically from the sources its
//! release manifest step names: the bundle identifier and URL schemes of an
//! Info.plist (`bundle-id:`, `url-scheme:`), the executable products of a
//! `Package.swift` (`product:`), and the path literals a Swift file appends
//! (`harness-path:`). A surface that cannot be read is an error, never a
//! shorter list: the rule reads a shorter surface as removed capability.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use plist::Value;
use regex::Regex;

use super::AppSources;

pub(super) type Read<T> = Result<T, String>;
/// Maps a repository-relative path to its bytes: the tree's, or a tag's.
pub(super) type Loader<'a> = &'a dyn Fn(&str) -> Read<Vec<u8>>;

const INTERPOLATION: &str = "\\(";
const KEY_IDENTIFIER: &str = "CFBundleIdentifier";
const KEY_URL_TYPES: &str = "CFBundleURLTypes";
const KEY_URL_SCHEMES: &str = "CFBundleURLSchemes";
const KEY_SHORT_VERSION: &str = "CFBundleShortVersionString";

/// An executable product; `.executableTarget(` cannot match.
static EXECUTABLE_PRODUCT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\.executable\s*\(\s*name:\s*"(?P<name>[^"]+)""#).expect("valid")
});
static APPENDED_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\.appending\s*\(\s*path:\s*"(?P<path>[^"]+)""#).expect("valid"));

fn text(load: Loader, path: &str) -> Read<String> {
    String::from_utf8(load(path)?).map_err(|error| format!("{path}: not UTF-8 ({error})"))
}

fn products(path: &str, manifest: &str) -> Read<Vec<String>> {
    let names = EXECUTABLE_PRODUCT
        .captures_iter(manifest)
        .map(|found| found["name"].trim().to_string())
        .collect::<Vec<_>>();
    if names.is_empty() || names.iter().any(String::is_empty) {
        return Err(format!("{path}: declares no named executable product, so the manifest was reshaped rather than emptied; refusing to report a shorter surface"));
    }
    Ok(names)
}

fn appended(path: &str, source: &str) -> Read<Vec<String>> {
    let mut found = Vec::new();
    for capture in APPENDED_PATH.captures_iter(source) {
        let name = capture["path"].trim().to_string();
        if name.is_empty() || name.contains(INTERPOLATION) {
            return Err(format!("{path}: the path literal {name:?} is empty or interpolated, so it names no fixed location"));
        }
        found.push(name);
    }
    if found.is_empty() {
        return Err(format!(
            "{path}: appends no path literal; the contract is unknown rather than empty"
        ));
    }
    Ok(found)
}

fn info(load: Loader, path: &str) -> Read<plist::Dictionary> {
    Value::from_reader(std::io::Cursor::new(load(path)?))
        .map_err(|error| format!("{path}: could not be parsed as a plist: {error}"))?
        .into_dictionary()
        .ok_or_else(|| format!("{path}: the top level is not a dictionary"))
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_string)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn bundle_names(load: Loader, path: &str) -> Read<Vec<String>> {
    let info = info(load, path)?;
    let identifier = non_empty(info.get(KEY_IDENTIFIER))
        .ok_or_else(|| format!("{path}: {KEY_IDENTIFIER} is missing or empty"))?;
    let mut names = vec![format!("bundle-id:{identifier}")];
    // An absent CFBundleURLTypes is a legitimate, breaking removal of every
    // scheme; a malformed one is an error.
    let Some(types) = info.get(KEY_URL_TYPES) else {
        return Ok(names);
    };
    for entry in types
        .as_array()
        .ok_or_else(|| format!("{path}: {KEY_URL_TYPES} is not an array"))?
    {
        let entry = entry
            .as_dictionary()
            .ok_or_else(|| format!("{path}: a {KEY_URL_TYPES} entry is not a dictionary"))?;
        let Some(schemes) = entry.get(KEY_URL_SCHEMES) else {
            continue;
        };
        for scheme in schemes
            .as_array()
            .ok_or_else(|| format!("{path}: {KEY_URL_SCHEMES} is not an array"))?
        {
            let scheme = non_empty(Some(scheme))
                .ok_or_else(|| format!("{path}: a {KEY_URL_SCHEMES} entry is empty"))?;
            names.push(format!("url-scheme:{scheme}"));
        }
    }
    Ok(names)
}

/// The surface of one tree, wherever its bytes come from.
pub(super) fn of(load: Loader, sources: &AppSources) -> Read<Vec<String>> {
    let mut names = BTreeSet::new();
    if let Some(plist) = &sources.info_plist {
        names.extend(bundle_names(load, plist)?);
    }
    if let Some(package) = &sources.package_json {
        names.extend(super::javascript::package::names(
            load,
            package,
            &sources.export_conditions,
        )?);
    }
    if let Some(manifest) = &sources.products {
        for name in products(manifest, &text(load, manifest)?)? {
            names.insert(format!("product:{name}"));
        }
    }
    if let Some(project) = &sources.tuist_project {
        let helpers = &sources.tuist_helpers;
        names.extend(super::tuist::surface(load, project, helpers)?);
    }
    if let Some(table) = &sources.command_table {
        names.extend(super::cargo::commands(load, table)?);
    }
    names.extend(super::javascript::surface(load, sources)?);
    if let Some(manifest) = &sources.pyproject {
        names.extend(super::python::console_scripts(load, manifest)?);
    }
    for module in &sources.python_all {
        names.extend(super::python::exported(load, module)?);
    }
    for module in &sources.python_argparse {
        names.extend(super::python::cli_commands(load, module)?);
    }
    if let Some(dir) = &sources.python_manifests {
        names.extend(super::python::manifests(
            load,
            dir,
            &sources.manifest_suffixes,
        )?);
    }
    for source in &sources.appended_paths {
        for name in appended(source, &text(load, source)?)? {
            names.insert(format!("harness-path:{name}"));
        }
    }
    Ok(names.into_iter().collect())
}

/// The version the Info.plist declares, else the Tuist project's marketing
/// version, else the Cargo.toml's package version, else the pyproject's
/// `[project]` version, else setup.py's, else the package.json's.
pub(super) fn declared_version(load: Loader, sources: &AppSources) -> Read<String> {
    let source = sources.version_source();
    if sources.info_plist.is_none() {
        if let Some(project) = &sources.tuist_project {
            let helpers = &sources.tuist_helpers;
            return super::tuist::declared_version(load, project, helpers);
        }
        if let Some(manifest) = &sources.cargo_toml {
            return super::cargo::declared_version(load, manifest);
        }
        if let Some(manifest) = &sources.pyproject {
            return super::python::pyproject_version(load, manifest);
        }
        if let Some(script) = &sources.setup_py {
            return super::python::setup_version(load, script);
        }
    }
    let declared = match &sources.info_plist {
        Some(plist) => non_empty(info(load, plist)?.get(KEY_SHORT_VERSION)),
        None => serde_json::from_slice::<serde_json::Value>(&load(source)?)
            .map_err(|error| format!("{source}: not JSON ({error})"))?["version"]
            .as_str()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .map(str::to_string),
    };
    declared.ok_or_else(|| format!("{source}: declares no version"))
}

/// The entries of a directory the loader lists: a tag's `git show rev:dir`
/// answer (a `tree` header, a blank line, then one entry per line) or the
/// tree loader's listing in the same shape, directories ending in `/`.
pub(super) fn entries(load: Loader, dir: &str) -> Read<Vec<String>> {
    let listing = text(load, dir)?;
    let body = match listing.split_once("\n\n") {
        Some((header, body)) if header.starts_with("tree ") => body,
        _ => listing.as_str(),
    };
    Ok(body
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Reads repository-relative paths from the tree at `root`; a directory
/// answers its listing in the shape `entries` reads.
pub(super) fn tree(root: &Path) -> impl Fn(&str) -> Read<Vec<u8>> + '_ {
    move |relative: &str| {
        let path = root.join(relative);
        let unreadable = |error: std::io::Error| {
            format!(
                "{relative}: not readable under {} ({error})",
                root.display()
            )
        };
        if path.is_dir() {
            let mut names = Vec::new();
            for entry in std::fs::read_dir(&path).map_err(unreadable)? {
                let entry = entry.map_err(unreadable)?;
                let mut name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() {
                    name.push('/');
                }
                names.push(name);
            }
            names.sort();
            return Ok(names.join("\n").into_bytes());
        }
        std::fs::read(&path).map_err(unreadable)
    }
}
