//! An application sold on the App Store (`--app-store-tags <workflow>`).
//!
//! The artifact a user holds is a signed, encrypted `.ipa` Apple serves only
//! to devices, so no runner can read a surface out of it. What exists is the
//! tag the named workflow writes, `appstore/<version>(<build>)`, only after
//! App Store Connect reports the version READY_FOR_SALE or
//! PENDING_DEVELOPER_RELEASE. `git-archive:<tag>` is therefore the top tier
//! and `head:<sha>` the only one below it; those facts are re-read from the
//! workflow on every baseline, and when it stops saying them this refuses
//! instead of asserting the old ladder. `app-check` also asks the App Store
//! which version it serves: the baseline must be exactly that version, and
//! the version a change requires must be newer than it.

use std::cmp::Ordering;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::super::surface::{self, Read};
use super::super::AppSources;
use super::{git, remote_tags, run, REMOTE};

const TAG_NAMESPACE: &str = "appstore/";
/// What the tagging workflow must still say for a tag to outrank HEAD.
const TAGGING_FACTS: [&str; 3] = [
    "tag=\"appstore/${version_string}(${build_version})\"",
    "READY_FOR_SALE|PENDING_DEVELOPER_RELEASE",
    "git push origin \"refs/tags/$tag\"",
];
/// major.minor.patch
const SLOTS: usize = 3;
const LOOKUP: &str = "https://itunes.apple.com/lookup?bundleId=";
/// The surface names a bundle identifier is spelled with: the Tuist reader's
/// and the Info.plist reader's.
const BUNDLE_KINDS: [&str; 2] = ["app-bundle-id:", "bundle-id:"];
const REGENERATE: &str = "Regenerate it with `stado release version-gate app-baseline`.";

static TAG_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^appstore/(?P<version>[0-9]+(?:\.[0-9]+)*)\((?P<build>[0-9]+(?:\.[0-9]+)*)\)$")
        .expect("valid")
});

fn slots(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|slot| slot.parse().unwrap_or_default())
        .collect()
}

/// `left` compared with `right` slot by slot, a missing slot counting as zero.
pub(in super::super) fn order(left: &str, right: &str) -> Ordering {
    let (mut left, mut right) = (slots(left), slots(right));
    let width = left.len().max(right.len());
    left.resize(width, 0);
    right.resize(width, 0);
    left.cmp(&right)
}

fn assert_release_scheme(root: &Path, workflow: &str) -> Read<()> {
    let text = std::fs::read_to_string(root.join(workflow)).map_err(|error| {
        format!("{workflow}: {error}; nothing in this tree says an {TAG_NAMESPACE}* tag marks a version the App Store published, so the tier ladder has lost its ground")
    })?;
    match TAGGING_FACTS.iter().find(|fact| !text.contains(**fact)) {
        Some(fact) => Err(format!(
            "{workflow} no longer contains {fact:?}, so the meaning of an {TAG_NAMESPACE}* tag has changed; the baseline ladder must be re-decided rather than asserted"
        )),
        None => Ok(()),
    }
}

/// Every App Store tag, newest first, as (tag, version, commit). Older tags
/// spell two slots, so versions are padded, never compared as text.
fn ranked(tags: Vec<(String, String)>) -> Vec<(String, String, String)> {
    let mut entries = tags
        .into_iter()
        .filter_map(|(name, sha)| {
            let found = TAG_PATTERN.captures(&name)?;
            let version = found["version"].to_string();
            let mut padded = slots(&version);
            padded.resize(padded.len().max(SLOTS), 0);
            Some(((padded, slots(&found["build"])), name, version, sha))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.0.cmp(&left.0));
    entries
        .into_iter()
        .map(|(_, name, version, sha)| (name, version, sha))
        .collect()
}

/// The newest tag whose tree declares the version the tag claims.
fn published(root: &Path, sources: &AppSources) -> Read<Option<(String, String, Vec<String>)>> {
    for (tag, version, sha) in ranked(remote_tags(root)?) {
        let local = git(root, &["rev-parse", "--verify", &format!("refs/tags/{tag}^{{commit}}")])
            .map_err(|error| format!("{REMOTE} serves tag {tag} but this clone cannot resolve it ({error}); fetch tags and history, then ask again"))?;
        if local != sha {
            return Err(format!("tag {tag} is {local} here and {sha} at {REMOTE}; refusing to measure against a tag whose identity is not settled"));
        }
        let load = |relative: &str| run(root, &["show", &format!("{tag}:{relative}")]);
        let declared = surface::declared_version(&load, sources)?;
        if declared != version {
            eprintln!("stado release version-gate: tag {tag} names version {version} but its tree declares {declared}; skipping it and looking further back");
            continue;
        }
        if slots(&version).len() != SLOTS {
            return Err(format!("tag {tag} carries version {version}, which is not a major.minor.patch triple, so the rule cannot advance it; name the next version deliberately"));
        }
        let surface = surface::of(&load, sources)?;
        return Ok(Some((tag, version, surface)));
    }
    Ok(None)
}

/// The baseline document the best reachable artifact yields.
pub(super) fn build(root: &Path, sources: &AppSources, workflow: &str) -> Read<Value> {
    assert_release_scheme(root, workflow)?;
    if let Some((tag, version, surface)) = published(root, sources)? {
        return Ok(json!({
            "version": version,
            "source": format!("git-archive:{tag} the newest {TAG_NAMESPACE}<version>(<build>) tag at {REMOTE}, which {workflow} writes only once App Store Connect reports the version READY_FOR_SALE or PENDING_DEVELOPER_RELEASE; the surface is read from that tag's blobs, because the shipped .ipa is served to devices alone"),
            "surface": surface,
        }));
    }
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let load = surface::tree(root);
    Ok(json!({
        "version": surface::declared_version(&load, sources)?,
        "source": format!("head:{sha} no {TAG_NAMESPACE}<version>(<build>) tag at {REMOTE} could be used, so the declared version is the only coordinate left"),
        "surface": surface::of(&load, sources)?,
    }))
}

/// The bundle identifier the frozen baseline names: the question is what the
/// published artifact's identifier serves, and a tree that renamed the bundle
/// would otherwise ask about itself.
fn bundle(committed: &Value) -> Read<String> {
    committed["surface"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|name| BUNDLE_KINDS.iter().find_map(|kind| name.strip_prefix(kind)))
        .map(str::to_string)
        .ok_or_else(|| "released-surface.json names no app bundle identifier, so there is no subject to ask the App Store about".to_string())
}

/// The version the App Store serves for the baseline's bundle. The answer's
/// content is read, never an exit status: no egress, a rate-limit page and
/// "not on sale" look alike by status, and the entry must name the bundle back.
pub(in super::super) fn live_version(committed: &Value) -> Read<String> {
    let bundle = bundle(committed)?;
    let output =
        crate::wait::output(Command::new("curl").args(["-sS", &format!("{LOOKUP}{bundle}")]))
            .map_err(|error| format!("curl could not start: {error}"))?;
    let answer = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    answer["results"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["bundleId"].as_str() == Some(bundle.as_str()))
        .and_then(|entry| entry["version"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!(
            "the App Store did not name {bundle} back (its answer was {} bytes long; {}), so what is on sale is unproven, which is not the same as fine",
            output.stdout.len(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
}

/// The newest `appstore/<version>(<build>)` tag origin served at handoff.
fn newest_tag(record: &Value) -> Option<String> {
    let names = record["origin_tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| (name.to_string(), String::new()))
        .collect();
    ranked(names).into_iter().next().map(|(name, _, _)| name)
}

/// Refuse a baseline that misstates where it came from or that a newer
/// version on sale supersedes. `record` is the verified provenance record.
pub(in super::super) fn provenance(
    record: &Value,
    marker: &str,
    released: &str,
    declared: &str,
    live: &str,
    version_source: &str,
) -> Read<()> {
    let Some(tag) = marker.strip_prefix("git-archive:") else {
        return Err(format!("released-surface.json is recovered from '{marker}', but the App Store sells {live}, so an {TAG_NAMESPACE}* tag exists and is the baseline. {REGENERATE}"));
    };
    let tagged = TAG_PATTERN
        .captures(tag)
        .map(|found| found["version"].to_string())
        .ok_or_else(|| format!("released-surface.json names tag '{tag}', which is not {TAG_NAMESPACE}<version>(<build>)"))?;
    if tagged != released {
        return Err(format!("released-surface.json claims {released} but was recovered from tag {tag}, which names {tagged}. {REGENERATE}"));
    }
    if newest_tag(record).as_deref() != Some(tag) {
        return Err(format!("released-surface.json is tag {tag}, but origin already serves a newer {TAG_NAMESPACE}* tag, which supersedes it. {REGENERATE}"));
    }
    match order(live, released) {
        Ordering::Less => return Err(format!("released-surface.json claims {released}, but the App Store serves {live}, which is older. {REGENERATE}")),
        Ordering::Greater => return Err(format!("released-surface.json describes {released}, but the App Store already sells {live}, whose {TAG_NAMESPACE}* tag supersedes it. {REGENERATE}")),
        Ordering::Equal => {}
    }
    if order(declared, live) == Ordering::Less {
        return Err(format!("{version_source} declares {declared} while {live} is already on sale, and App Store Connect refuses a version it has already seen"));
    }
    println!("baseline {released} is tag {tag}, which origin serves at the commit the source handoff recorded, and is the version the App Store sells");
    Ok(())
}
