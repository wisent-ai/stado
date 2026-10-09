//! Derive the Stado command-surface baseline from verified release bytes.
//!
//! Every visible `v*` tag and the version `stado-rs/Cargo.toml` declares are
//! candidates, newest first. The first one whose signed manifest AND archive
//! the channel serves for this runner's platform is the baseline: its
//! archive is verified against the manifest and its own binary is asked for
//! its surface. A manifest without its archive is half published and, since
//! release objects are immutable, skipped and named on stderr. An empty
//! channel bootstraps once from the candidate binary.

mod channel;

use std::cmp::Ordering;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

pub(super) use channel::Refusal;
use channel::{release_base, state, surface_from_release};

static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$")
        .expect("static")
});
static CARGO_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^version\s*=\s*"([^"]+)"\s*$"#).expect("static"));

/// Byte-exact: the version gate asserts this source string literally.
const BOOTSTRAP_SOURCE: &str = "bootstrap from the candidate binary; release channel was empty";

fn git(args: &[&str]) -> Result<String, Refusal> {
    let output = crate::wait::output(&mut Command::new("git").args(args))
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Release order of `vMAJOR.MINOR.PATCH[-PRE]`: a release above its own
/// pre-releases, pre-releases ordered by their text.
fn version_order(left: &str, right: &str) -> Ordering {
    let key = |tag: &str| {
        let found = TAG.captures(tag)?;
        let number = |index: usize| found[index].parse::<u64>().ok();
        let pre = found.get(4).map(|m| m.as_str().to_string());
        Some((
            number(1)?,
            number(2)?,
            number(3)?,
            pre.is_none(),
            pre.unwrap_or_default(),
        ))
    };
    key(left).cmp(&key(right))
}

fn tag_of(reference: &str) -> &str {
    let reference = reference.strip_prefix("refs/tags/").unwrap_or(reference);
    reference.strip_suffix("^{}").unwrap_or(reference)
}

/// Every release tag, refusing a clone where some are not visible: a tier
/// ranked blind could pick a baseline that is not the newest.
fn visible_versions() -> Result<Vec<String>, Refusal> {
    if git(&["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        return Err("repository is shallow; release tags are not fully visible"
            .to_string()
            .into());
    }
    let local: std::collections::BTreeSet<String> = git(&["tag", "--list", "v*"])?
        .lines()
        .filter(|tag| TAG.is_match(tag))
        .map(str::to_string)
        .collect();
    let remote_listing = git(&["ls-remote", "--tags", "origin"])?;
    let mut missing: Vec<&str> = remote_listing
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(_, reference)| tag_of(reference))
        })
        .filter(|tag| TAG.is_match(tag) && !local.contains(*tag))
        .collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(format!("release tags are not fully visible: missing {missing:?}").into());
    }
    Ok(local.iter().map(|tag| tag[1..].to_string()).collect())
}

fn declared_version() -> Option<String> {
    let text = std::fs::read_to_string("stado-rs/Cargo.toml").ok()?;
    let version = CARGO_VERSION.captures(&text)?[1].to_string();
    TAG.is_match(&format!("v{version}")).then_some(version)
}

fn native_platform() -> Result<&'static str, Refusal> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(format!("no executable Stado release platform for {os}/{arch}").into()),
    }
}

fn verdict(value: &str) -> Result<(), Refusal> {
    match value {
        "present" | "absent" => Ok(()),
        other => Err(format!("release channel returned an unknown state {other:?}").into()),
    }
}

/// `stado:VERSION` for the newest whole release, or `bootstrap:VERSION` on
/// an empty channel; with `output`, also the baseline document.
pub(super) fn best(stado: &Path, output: Option<&Path>) -> Result<String, Refusal> {
    let mut versions = visible_versions()?;
    let current = declared_version();
    versions.extend(current.clone());
    versions.sort_by(|a, b| version_order(&format!("v{b}"), &format!("v{a}")));
    versions.dedup();
    // The surface is architecture-independent: verify and run the release
    // matching this runner only.
    let platform = native_platform()?;
    let mut partial: Vec<String> = Vec::new();
    for version in &versions {
        let marker = state(
            stado,
            &format!("{}/release.json", release_base(version, platform)),
        )?;
        verdict(&marker)?;
        if marker != "present" {
            continue;
        }
        let archive = state(
            stado,
            &format!("{}/release.tar.gz", release_base(version, platform)),
        )?;
        verdict(&archive)?;
        if archive != "present" {
            partial.push(format!("{version}/{platform}"));
            eprintln!("{version} is half published, skipping: the release manifest is present and the archive it names is absent for {platform}. Release objects are immutable, so this coordinate cannot be completed; looking for an older whole release to build the baseline from.");
            continue;
        }
        if !partial.is_empty() {
            eprintln!(
                "baseline built from {version}; skipped {} half-published coordinate(s): {}",
                partial.len(),
                partial.join(", ")
            );
        }
        if let Some(output) = output {
            let root = tempfile::Builder::new()
                .prefix("stado-baseline-")
                .tempdir()
                .map_err(|error| format!("cannot create a scratch directory: {error}"))?;
            let baseline = surface_from_release(stado, version, platform, root.path())?;
            write_document(output, &baseline)?;
        }
        return Ok(format!("stado:{version}"));
    }
    let partial_note = if partial.is_empty() {
        String::new()
    } else {
        format!("; skipped {} half-published coordinate(s) whose manifest is present and archive absent, which immutability makes permanent: {}", partial.len(), partial.join(", "))
    };
    let Some(current) = current else {
        return Err(format!(
            "release channel contains no complete verified Stado release{partial_note}"
        )
        .into());
    };
    if !partial.is_empty() {
        eprintln!("release channel holds no whole release{partial_note}. Bootstrapping the baseline from the candidate binary, as it does on an empty channel.");
    }
    if let Some(output) = output {
        let commands = super::surface::of_binary(stado).map_err(|error| {
            format!("candidate binary advertised an invalid bootstrap command surface: {error}")
        })?;
        let document = serde_json::json!({ "version": current, "source": BOOTSTRAP_SOURCE, "surface": commands });
        write_document(output, &document)?;
    }
    Ok(format!("bootstrap:{current}"))
}

fn write_document(output: &Path, document: &serde_json::Value) -> Result<(), Refusal> {
    let text = serde_json::to_string_pretty(document).expect("baseline serialises") + "\n";
    std::fs::write(output, text).map_err(|error| format!("{}: {error}", output.display()).into())
}
