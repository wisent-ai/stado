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

/// The condition Node matches in every environment, whatever the caller's set.
const UNCONDITIONAL: &str = "default";

/// Node's `resolvePackageTarget` (lib/internal/modules/esm/resolve.js) under
/// one condition set: `Some(true)` for a path, `Some(false)` for a blocked
/// target (`null`, an empty array, or an array whose every branch is blocked
/// or unmatched after at least one was blocked), `None` when nothing matched,
/// so the caller moves on. In a condition map the keys are tried in their
/// written order; `default` always matches, any other key when the set holds
/// it, and the first match decides even when it is blocked.
fn resolve(target: &serde_json::Value, conditions: &[&str]) -> Option<bool> {
    match target {
        serde_json::Value::String(path) => Some(!path.is_empty()),
        serde_json::Value::Null => Some(false),
        serde_json::Value::Array(items) if items.is_empty() => Some(false),
        serde_json::Value::Array(items) => {
            let mut blocked = false;
            for item in items {
                match resolve(item, conditions) {
                    Some(true) => return Some(true),
                    Some(false) => blocked = true,
                    None => {}
                }
            }
            blocked.then_some(false)
        }
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(condition, _)| {
                condition.as_str() == UNCONDITIONAL || conditions.contains(&condition.as_str())
            })
            .find_map(|(_, branch)| resolve(branch, conditions)),
        _ => Some(false),
    }
}

/// Whether an `exports` target opens an entry point under at least one of
/// the condition sets the product declares (`--export-conditions`, each a
/// comma-separated set such as the one its consumers load it with).
fn reachable(target: &serde_json::Value, condition_sets: &[String]) -> bool {
    condition_sets.iter().any(|set| {
        let conditions = set.split(',').map(str::trim).collect::<Vec<_>>();
        resolve(target, &conditions) == Some(true)
    })
}

/// The entry points `exports` opens (or `main`, when there is no `exports`).
/// A map whose keys all start with "." is a subpath map; one without any is a
/// condition map for the root; Node refuses a mix, and so does this.
fn export_names(
    document: &serde_json::Value,
    path: &str,
    conditions: &[String],
) -> Read<Vec<String>> {
    let exports = &document["exports"];
    let root = || vec![".".to_string()];
    let reachable = |target: &serde_json::Value| reachable(target, conditions);
    if !exports.is_null() && conditions.is_empty() {
        return Err(format!("{path}: declares exports, so name the condition sets its consumers load it with (--export-conditions, e.g. one per environment); which entry points resolve depends on them"));
    }
    match exports {
        serde_json::Value::Null => Ok(match &document["main"] {
            serde_json::Value::String(main) if !main.is_empty() => root(),
            _ => Vec::new(),
        }),
        serde_json::Value::Object(map) => {
            let subpaths = map.keys().filter(|key| key.starts_with('.')).count();
            if subpaths == map.len() {
                Ok(map
                    .iter()
                    .filter(|(_, target)| reachable(target))
                    .map(|(key, _)| key.clone())
                    .collect())
            } else if subpaths == 0 {
                Ok(if reachable(exports) {
                    root()
                } else {
                    Vec::new()
                })
            } else {
                Err(format!(
                    "{path}: exports mixes subpaths and conditions, which Node refuses"
                ))
            }
        }
        other if reachable(other) => Ok(root()),
        _ => Ok(Vec::new()),
    }
}

/// A package.json's contract: its name (`package:`), every entry point it
/// opens (`export:`) and every command it installs (`bin:`).
fn package_names(load: Loader, path: &str, conditions: &[String]) -> Read<Vec<String>> {
    let document: serde_json::Value = serde_json::from_slice(&load(path)?)
        .map_err(|error| format!("{path}: not JSON ({error})"))?;
    let name = document["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("{path}: declares no package name"))?;
    let mut names = vec![format!("package:{name}")];
    names.extend(
        export_names(&document, path, conditions)?
            .into_iter()
            .map(|key| format!("export:{key}")),
    );
    match &document["bin"] {
        serde_json::Value::String(target) if !target.is_empty() => {
            let command = name.rsplit('/').next().unwrap_or(name);
            names.push(format!("bin:{command}"));
        }
        serde_json::Value::Object(map) => names.extend(
            map.iter()
                .filter(|(_, target)| target.as_str().is_some_and(|target| !target.is_empty()))
                .map(|(key, _)| format!("bin:{key}")),
        ),
        serde_json::Value::Null => {}
        _ => return Err(format!("{path}: bin is neither a path nor a map of paths")),
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
        names.extend(package_names(load, package, &sources.export_conditions)?);
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
    for source in &sources.appended_paths {
        for name in appended(source, &text(load, source)?)? {
            names.insert(format!("harness-path:{name}"));
        }
    }
    Ok(names.into_iter().collect())
}

/// The version the Info.plist declares, else the Tuist project's marketing
/// version, else the package.json's `version`.
pub(super) fn declared_version(load: Loader, sources: &AppSources) -> Read<String> {
    let source = sources.version_source();
    if sources.info_plist.is_none() {
        if let Some(project) = &sources.tuist_project {
            return super::tuist::declared_version(load, project);
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

/// Reads repository-relative paths from the tree at `root`.
pub(super) fn tree(root: &Path) -> impl Fn(&str) -> Read<Vec<u8>> + '_ {
    move |relative: &str| {
        std::fs::read(root.join(relative)).map_err(|error| {
            format!(
                "{relative}: not readable under {} ({error})",
                root.display()
            )
        })
    }
}
