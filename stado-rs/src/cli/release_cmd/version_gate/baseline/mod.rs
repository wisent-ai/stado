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

use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use semver::Version;

pub(super) use channel::Refusal;
use channel::{release_base, state, surface_from_release, ObjectState};

static CARGO_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^version\s*=\s*"(?P<version>[^"]+)"\s*$"#).expect("static")
});

/// Byte-exact: the version gate asserts this source string literally.
const BOOTSTRAP_SOURCE: &str = "bootstrap from the candidate binary; release channel was empty";

fn git(args: &[&str]) -> Result<String, Refusal> {
    let output = crate::wait::output(Command::new("git").args(args))
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

/// The release a `vMAJOR.MINOR.PATCH[-PRE]` tag names, or none for any other
/// tag (build metadata included: a release tag carries none).
fn release(tag: &str) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix('v')?);
    match version {
        Ok(version) if version.build.is_empty() => Some(version),
        Ok(_with_build_metadata) => None,
        Err(_not_a_release) => None,
    }
}

fn tag_of(reference: &str) -> &str {
    let reference = match reference.strip_prefix("refs/tags/") {
        Some(tag) => tag,
        None => reference,
    };
    match reference.strip_suffix("^{}") {
        Some(tag) => tag,
        None => reference,
    }
}

/// Every release tag, refusing a clone where some are not visible: a tier
/// ranked blind could pick a baseline that is not the newest.
fn visible_versions() -> Result<Vec<Version>, Refusal> {
    if git(&["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        return Err("repository is shallow; release tags are not fully visible"
            .to_string()
            .into());
    }
    let local: std::collections::BTreeMap<String, Version> = git(&["tag", "--list", "v*"])?
        .lines()
        .filter_map(|tag| release(tag).map(|version| (tag.to_string(), version)))
        .collect();
    let remote_listing = git(&["ls-remote", "--tags", "origin"])?;
    let mut missing: Vec<&str> = remote_listing
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(_, reference)| tag_of(reference))
        })
        .filter(|tag| release(tag).is_some() && !local.contains_key(*tag))
        .collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(format!("release tags are not fully visible: missing {missing:?}").into());
    }
    Ok(local.into_values().collect())
}

/// The version `stado-rs/Cargo.toml` declares, when it declares a release
/// version. A manifest that cannot be read is refused naming it: read as no
/// declared version, the baseline was chosen without the release being made.
fn declared_version() -> Result<Option<Version>, Refusal> {
    let manifest = "stado-rs/Cargo.toml";
    let text = std::fs::read_to_string(manifest)
        .map_err(|error| Refusal::from(format!("cannot read {manifest}: {error}")))?;
    Ok(CARGO_VERSION
        .captures(&text)
        .and_then(|captures| captures.name("version"))
        .and_then(|version| release(&format!("v{}", version.as_str()))))
}

fn native_platform() -> Result<&'static str, Refusal> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(format!("no executable Stado release platform for {os}/{arch}").into()),
    }
}

/// `stado:VERSION` for the newest whole release, or `bootstrap:VERSION` on
/// an empty channel; with `output`, also the baseline document.
pub(super) fn best(stado: &Path, output: Option<&Path>) -> Result<String, Refusal> {
    let mut releases = visible_versions()?;
    let current = declared_version()?;
    releases.extend(current.clone());
    // Newest first, by SemVer precedence: a release above its own
    // pre-releases, pre-release identifiers compared field by field.
    releases.sort_by(|left, right| right.cmp_precedence(left));
    releases.dedup_by(|left, right| left.cmp_precedence(right).is_eq());
    let versions: Vec<String> = releases.iter().map(Version::to_string).collect();
    let current = current.map(|version| version.to_string());
    // The surface is architecture-independent: verify and run the release
    // matching this runner only.
    let platform = native_platform()?;
    let mut partial: Vec<String> = Vec::new();
    for version in &versions {
        let marker = state(
            stado,
            &format!("{}/release.json", release_base(version, platform)),
        )?;
        if marker != ObjectState::Present {
            continue;
        }
        let archive = state(
            stado,
            &format!("{}/release.tar.gz", release_base(version, platform)),
        )?;
        if archive != ObjectState::Present {
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
