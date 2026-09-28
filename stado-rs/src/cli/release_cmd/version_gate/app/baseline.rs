//! `released-surface.json` for an application from the best reachable
//! artifact: `git-archive:<tag>`, the newest version tag `origin` serves at
//! the commit this checkout resolves it to, with the surface read from the
//! tag's blobs; else `head:<sha>` with the version the Info.plist declares,
//! only while `origin` serves no version tag.

use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::surface::{self, Read};
use super::AppSources;

const REMOTE: &str = "origin";
const PEELED: &str = "^{}";

/// A version tag: `v1.2.3`, optionally with a pre-release suffix.
static VERSION_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v?(?P<core>[0-9]+(?:\.[0-9]+)*)(?P<pre>[-+].*)?$").expect("valid")
});

fn run(root: &Path, arguments: &[&str]) -> Read<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(|error| format!("git {} could not start: {error}", arguments.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn git(root: &Path, arguments: &[&str]) -> Read<String> {
    run(root, arguments).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
}

/// The version a tag names.
pub(super) fn version_of(tag: &str) -> Option<String> {
    let found = VERSION_TAG.captures(tag)?;
    Some(format!(
        "{}{}",
        &found["core"],
        found.name("pre").map_or("", |pre| pre.as_str())
    ))
}

fn slots(tag: &str) -> Vec<u64> {
    VERSION_TAG
        .captures(tag)
        .map(|found| {
            found["core"]
                .split('.')
                .map(|slot| slot.parse().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// The newest version tag among `names`.
pub(super) fn newest<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    names
        .filter(|name| VERSION_TAG.is_match(name))
        .max_by(|left, right| slots(left).cmp(&slots(right)))
        .map(str::to_string)
}

/// Tag names and the commit each names, as `origin` serves them.
fn remote_tags(root: &Path) -> Read<Vec<(String, String)>> {
    let listing = git(root, &["ls-remote", "--tags", REMOTE])?;
    let mut found: Vec<(String, String)> = Vec::new();
    for line in listing.lines() {
        let Some((sha, reference)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Some(name) = reference.trim().strip_prefix("refs/tags/") else {
            continue;
        };
        let (name, peeled) = match name.strip_suffix(PEELED) {
            Some(bare) => (bare, true),
            None => (name, false),
        };
        match found.iter_mut().find(|(known, _)| known == name) {
            Some(entry) if peeled => entry.1 = sha.to_string(),
            Some(_) => {}
            None => found.push((name.to_string(), sha.to_string())),
        }
    }
    Ok(found)
}

/// The baseline document the best reachable artifact yields.
pub(super) fn build(root: &Path, sources: &AppSources) -> Read<Value> {
    let tags = remote_tags(root)?;
    if let Some(tag) = newest(tags.iter().map(|(name, _)| name.as_str())) {
        let remote = &tags
            .iter()
            .find(|(name, _)| *name == tag)
            .expect("listed")
            .1;
        let local = git(root, &["rev-parse", "--verify", &format!("refs/tags/{tag}^{{commit}}")])
            .map_err(|error| format!("{REMOTE} serves tag {tag} but this clone cannot resolve it ({error}); fetch tags and history, then ask again"))?;
        if &local != remote {
            return Err(format!("tag {tag} is {local} here and {remote} at {REMOTE}; refusing to measure against a tag whose identity is not settled"));
        }
        let load = |relative: &str| run(root, &["show", &format!("{tag}:{relative}")]);
        return Ok(json!({
            "version": version_of(&tag).expect("a version tag names a version"),
            "source": format!("git-archive:{tag} the newest version tag at {REMOTE}, at the commit {REMOTE} serves; the surface is read from its blobs"),
            "surface": surface::of(&load, sources)?,
        }));
    }
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let load = surface::tree(root);
    Ok(json!({
        "version": surface::declared_version(&load, sources)?,
        "source": format!("head:{sha} {REMOTE} serves no version tag, so nothing has been released and the version is the one {} declares", sources.version_source()),
        "surface": surface::of(&load, sources)?,
    }))
}
