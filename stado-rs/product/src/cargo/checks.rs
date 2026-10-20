//! What a canonical Cargo run checks around the build: the Cargo it runs,
//! the resolved graph it builds, and that every source it read is still the
//! one it recorded.

use super::pinned::Pinned;
use super::sources;
use crate::common::{checked, toolchain_command, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{fs, path::Path};

/// The toolchain Stado declares for itself, compiled in: the Cargo it is
/// built and verified with, which reads an isolated lockfile.
const TOOLCHAIN: &str = include_str!("../../../../rust-toolchain.toml");

/// The version line of the Cargo this host runs, refused when it is older
/// than the toolchain Stado declares in `rust-toolchain.toml`.
pub(super) fn cargo_version() -> Result<String> {
    let declared: toml::Value = toml::from_str(TOOLCHAIN)
        .context("Stado's compiled-in rust-toolchain.toml is unreadable")?;
    let channel = declared["toolchain"]["channel"]
        .as_str()
        .context("Stado's compiled-in rust-toolchain.toml names no toolchain channel")?;
    let floor = semver::Version::parse(channel).with_context(|| {
        format!("Stado's compiled-in rust-toolchain.toml channel {channel} is no version")
    })?;
    let version = checked(toolchain_command("cargo")?.arg("--version"))?;
    let version = String::from_utf8(version.stdout)?.trim().to_owned();
    let word = version
        .strip_prefix("cargo ")
        .and_then(|rest| rest.split_whitespace().next())
        .with_context(|| format!("Cargo did not report its version: {version}"))?;
    let observed = semver::Version::parse(word)
        .with_context(|| format!("Cargo reported a version that is no version: {version}"))?;
    if observed < floor {
        bail!("canonical Cargo is the toolchain Stado declares, {floor}, or later, which reads isolated lockfiles; observed {version}");
    }
    Ok(version)
}

/// Refuse a resolved `graph` that kept a Git dependency instead of its
/// canonical source, and require every path package's manifest to sit in
/// its canonical checkout (or the `pinned` export).
pub(super) fn canonical_graph(
    runtime: &Runtime,
    pinned: Option<&Pinned>,
    graph: &Value,
) -> Result<()> {
    for package in graph["packages"]
        .as_array()
        .context("resolved Cargo graph has no packages")?
    {
        if package["source"]
            .as_str()
            .is_some_and(|s| s.starts_with("git+"))
        {
            bail!(
                "Cargo retained a Git dependency instead of canonical source: {}",
                package["id"]
            );
        }
        if package["source"].is_null() {
            sources::manifest_in(
                runtime,
                pinned,
                Path::new(
                    package["manifest_path"]
                        .as_str()
                        .context("resolved Cargo package has no manifest")?,
                ),
            )?;
        }
    }
    Ok(())
}

/// Every source `records` names is still what the run recorded: a pinned
/// export's marker still names its commit and archive, and a checkout is
/// unchanged, read again into `evidence` with `scratch` for working space.
pub(super) fn sources_unchanged(records: &[Value], evidence: &Path, scratch: &Path) -> Result<()> {
    for (index, source) in records.iter().enumerate() {
        // A pinned export is a committed tree: its identity is the
        // commit and archive digest its marker records, read again.
        if source["pinned"].as_bool() == Some(true) {
            let exported = Path::new(
                source["exported_path"]
                    .as_str()
                    .context("pinned source record has no exported path")?,
            );
            let marker: Value =
                serde_json::from_slice(&fs::read(exported.join(".stado-exported.json"))?)?;
            if marker["archive_sha256"] != source["archive_sha256"]
                || marker["commit"] != source["commit"]
            {
                bail!(
                    "the pinned export {} changed under the build: its marker no longer \
                         names commit {} at archive {}",
                    exported.display(),
                    source["commit"],
                    source["archive_sha256"]
                );
            }
            continue;
        }
        let root = Path::new(
            source["repository_path"]
                .as_str()
                .context("source record has no repository path")?,
        );
        crate::source::verify_unchanged(
            root,
            source,
            &evidence.join("sources").join(index.to_string()),
            scratch,
        )?;
    }
    Ok(())
}
