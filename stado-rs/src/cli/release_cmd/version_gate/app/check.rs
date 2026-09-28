//! `app-check`: an application's version gate as a quality step of its
//! release build. The build's archive has no git history, so the baseline's
//! origin is read from the `.wisent-provenance/baseline.json` record
//! `stado build submit` wrote after verifying it against `origin`; the record
//! must carry every field the handoff writes and be bound to the very source
//! (`WISENT_SOURCE_COMMIT`) and marker being checked.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use super::super::{conformance, rule};
use super::baseline::{newest, version_of};
use super::surface::{self, Read};
use super::AppSources;

const BASELINE: &str = "released-surface.json";
const VERIFIED_BASELINE: &str = ".wisent-provenance/baseline.json";
const FIXTURES_URL: &str =
    "https://raw.githubusercontent.com/lbartoszcze/AutoVersion/v0.1.0/FIXTURES.md";
const REGENERATE: &str = "Regenerate it with `stado release version-gate app-baseline`.";

fn read_json(path: &Path) -> Read<Value> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: not JSON ({error})", path.display()))
}

fn fixtures() -> Read<String> {
    let output = Command::new("curl")
        .args(["-fsSL", FIXTURES_URL])
        .output()
        .map_err(|error| format!("curl could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{FIXTURES_URL} could not be read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("{FIXTURES_URL}: not UTF-8 ({error})"))
}

/// The provenance record, refused unless complete and bound to this source.
fn verified_record(root: &Path, marker: &str) -> Read<Value> {
    let record = read_json(&root.join(VERIFIED_BASELINE)).map_err(|error| {
        format!("{error}; the source archive carries no verified baseline provenance, so it was not made by a `stado build submit` that checks the baseline against origin")
    })?;
    let field = |name: &str| {
        record[name]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("{VERIFIED_BASELINE}: has no {name}"))
    };
    let source = std::env::var("WISENT_SOURCE_COMMIT")
        .map_err(|_| "WISENT_SOURCE_COMMIT is not set, so the provenance record cannot be bound to the source being built".to_string())?;
    if field("source_commit")? != source {
        return Err(format!(
            "{VERIFIED_BASELINE} was recorded for {}, but this build is {source}",
            field("source_commit")?
        ));
    }
    if field("marker")? != marker {
        return Err(format!(
            "{VERIFIED_BASELINE} was recorded for marker {}, but {BASELINE} says {marker}",
            field("marker")?
        ));
    }
    field("verified_against")?;
    let tags = record["origin_tags"]
        .as_array()
        .filter(|tags| tags.iter().all(Value::is_string))
        .ok_or_else(|| format!("{VERIFIED_BASELINE}: origin_tags is not a list of tag names"))?;
    if let Some(tag) = marker.strip_prefix("git-archive:") {
        if field("tag")? != tag {
            return Err(format!(
                "{BASELINE} names tag {tag}, but the source handoff verified {}. {REGENERATE}",
                field("tag")?
            ));
        }
        field("commit")?;
        field("tree")?;
        if !tags.iter().any(|name| name.as_str() == Some(tag)) {
            return Err(format!("{VERIFIED_BASELINE} verified tag {tag}, but does not list it among the tags origin serves"));
        }
    }
    Ok(record)
}

fn provenance(root: &Path, marker: &str, released: &str) -> Read<()> {
    let record = verified_record(root, marker)?;
    let newest_tag = newest(
        record["origin_tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    );
    if let Some(tag) = marker.strip_prefix("git-archive:") {
        if version_of(tag).as_deref() != Some(released) {
            return Err(format!(
                "{BASELINE} claims {released} but was recovered from tag {tag}. {REGENERATE}"
            ));
        }
        if newest_tag.as_deref() != Some(tag) {
            return Err(format!("{BASELINE} is tag {tag}, but the newest version tag origin serves is {}. {REGENERATE}", newest_tag.unwrap_or_default()));
        }
        return Ok(());
    }
    if marker.starts_with("head:") {
        return match newest_tag {
            Some(tag) => Err(format!(
                "{BASELINE} claims nothing is released, but origin serves {tag}. {REGENERATE}"
            )),
            None => Ok(()),
        };
    }
    Err(format!("unknown baseline marker '{marker}': app-baseline writes git-archive or head, so the file was edited by hand"))
}

/// The whole gate for the tree at `root`.
pub(super) fn check(root: &Path, sources: &AppSources) -> Read<()> {
    if !conformance::run(&fixtures()?)? {
        return Err("this port does not reproduce AutoVersion's fixtures".into());
    }
    let load = surface::tree(root);
    let candidate = surface::of(&load, sources)?;
    let declared = surface::declared_version(&load, sources)?;
    let committed = read_json(&root.join(BASELINE))?;
    let released = committed["version"]
        .as_str()
        .ok_or_else(|| format!("{BASELINE} has no version"))?;
    let marker = committed["source"]
        .as_str()
        .and_then(|source| source.split_whitespace().next())
        .unwrap_or_default();
    provenance(root, marker, released)?;
    let published = committed["surface"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut sorted = published.clone();
    sorted.sort();
    let shrunk = sorted.get(1..).unwrap_or_default().to_vec();
    let proof = rule::decide(released, &published, &shrunk, false)
        .map_err(|refusal| refusal.to_string())?;
    if proof.change.name() != "breaking" {
        return Err("removing one name from the released contract was not classified breaking, so this gate cannot refuse".into());
    }
    let verdict = rule::decide(released, &published, &candidate, false)
        .map_err(|refusal| refusal.to_string())?;
    let (change, required) = (verdict.change.name(), verdict.next);
    println!("{change} since {released} requires {required}; declared {declared}; removed {:?}; added {:?}", verdict.removed, verdict.added);
    if declared == released {
        if change != "internal" {
            return Err(format!("what a user of the app holds changed ({change}) but {} still declares {released}; declare {required}", sources.info_plist));
        }
    } else if declared != required {
        return Err(format!(
            "{} declares {declared}, but a {change} change since {released} requires {required}",
            sources.info_plist
        ));
    }
    Ok(())
}
