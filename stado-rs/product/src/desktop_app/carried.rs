//! What a desktop application kept under its retired `ai.wisent.` bundle
//! identifier, carried to the `com.wisent.` one it is installed as now.
//!
//! macOS files an application's preferences, caches, HTTP storage and saved
//! window state under its bundle identifier. An application renamed to the
//! `com.wisent.<product>` convention starts with none of them: the person's
//! settings, onboarding progress and remembered workspace stayed under the
//! old name. Before the renamed application first runs, its installation
//! copies each of them across, once: a place the new identifier already has
//! is never overwritten, so a second install, or one after the person
//! changed something, changes nothing. The old places are left as they were,
//! so a rollback to the previous build still finds them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::signing::current;

/// Where macOS keeps an application's own state, under the home directory,
/// with `{id}` for its bundle identifier.
const PLACES: [&str; 5] = [
    "Library/Preferences/{id}.plist",
    "Library/Caches/{id}",
    "Library/HTTPStorages/{id}",
    "Library/Saved Application State/{id}.savedState",
    "Library/Application Support/{id}",
];

/// The retired identifier `bundle_id` was installed as before, when it is a
/// `com.wisent.` identifier one of the retired spellings maps to.
fn retired_for(bundle_id: &str) -> Vec<String> {
    let Some(rest) = bundle_id.strip_prefix("com.wisent.") else {
        return Vec::new();
    };
    let mut candidates = vec![format!("ai.wisent.{rest}")];
    if let Some(product) = rest.strip_suffix(".desktop") {
        candidates.push(format!("ai.wisent.{product}"));
    }
    candidates
        .into_iter()
        .filter(|old| current(old) == bundle_id)
        .collect()
}

/// Copy what `home` keeps under the retired identifier of `bundle_id` to
/// `bundle_id`'s own places, never over one that exists. Answers each place
/// carried, kept or absent.
pub fn carry(home: &Path, bundle_id: &str) -> Result<Value> {
    let mut rows = Vec::new();
    for old in retired_for(bundle_id) {
        for place in PLACES {
            let from = home.join(place.replace("{id}", &old));
            let to = home.join(place.replace("{id}", bundle_id));
            let state = match (from.symlink_metadata(), to.symlink_metadata()) {
                (Err(error), _) if error.kind() == std::io::ErrorKind::NotFound => "absent",
                (Err(error), _) => {
                    return Err(error).with_context(|| {
                        format!("{} cannot be read to carry it to {bundle_id}", from.display())
                    })
                }
                (Ok(_), Ok(_)) => "kept",
                (Ok(_), Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    copy(&from, &to).with_context(|| {
                        format!(
                            "{} could not be carried to {}; {bundle_id} starts without it",
                            from.display(),
                            to.display()
                        )
                    })?;
                    "carried"
                }
                (Ok(_), Err(error)) => {
                    return Err(error).with_context(|| {
                        format!("{} cannot be read, so whether {bundle_id} has it is unknown", to.display())
                    })
                }
            };
            rows.push(json!({ "from": from, "to": to, "state": state }));
        }
    }
    Ok(Value::Array(rows))
}

/// Copy `from` to `to` through a staged name beside `to`, renamed into place
/// only once whole, so a failed copy never leaves half a preference file. A
/// preferences file also has its own retired keys renamed: an app kept
/// settings under keys named like itself (`ai.wisent.lem.onboarding…`), and
/// the renamed app reads them under their `com.wisent.` names.
fn copy(from: &Path, to: &Path) -> Result<()> {
    let parent = to.parent().context("the destination has no parent")?;
    std::fs::create_dir_all(parent)?;
    let staged: PathBuf = parent.join(format!(".wisent-carry-{}", uuid::Uuid::new_v4()));
    let copied = std::process::Command::new("/usr/bin/ditto")
        .arg(from)
        .arg(&staged)
        .output()
        .context("ditto could not be started")?;
    if !copied.status.success() {
        let _ = std::fs::remove_dir_all(&staged).or_else(|_| std::fs::remove_file(&staged));
        anyhow::bail!("ditto refused: {}", String::from_utf8_lossy(&copied.stderr).trim());
    }
    if to.extension().is_some_and(|extension| extension == "plist") {
        if let Err(error) = renamed_keys(&staged) {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
    }
    std::fs::rename(&staged, to)?;
    Ok(())
}

/// Rename each top-level key of the preferences file at `path` that is a
/// retired identifier to its convention name; a key already present under
/// the new name keeps its value.
fn renamed_keys(path: &Path) -> Result<()> {
    let mut document = plist::Value::from_file(path)
        .with_context(|| format!("{} is not a preferences file that can be read", path.display()))?;
    let Some(keys) = document.as_dictionary_mut() else {
        return Ok(());
    };
    let retired: Vec<String> = keys.keys().filter(|key| key.starts_with("ai.wisent.")).cloned().collect();
    for old in retired {
        let new = current(&old);
        if let Some(value) = keys.remove(&old) {
            if !keys.contains_key(&new) {
                keys.insert(new, value);
            }
        }
    }
    document
        .to_file_binary(path)
        .with_context(|| format!("{} could not be written with its keys renamed", path.display()))
}
