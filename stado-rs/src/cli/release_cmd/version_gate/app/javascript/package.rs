//! A package.json's contract: its name (`package:`), every entry point its
//! `exports` opens under the consumers' condition sets (`export:`) and every
//! command it installs (`bin:`).

use super::super::surface::{Loader, Read};

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
pub(in super::super) fn names(
    load: Loader,
    path: &str,
    conditions: &[String],
) -> Read<Vec<String>> {
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
