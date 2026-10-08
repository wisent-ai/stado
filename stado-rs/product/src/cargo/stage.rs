use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::common::{emit, Runtime};

use super::execute;

/// `stado product cargo stage --bin NAME…`: the release build a release
/// manifest's build step needs, without a script of its own. A manifest step
/// is an argument vector with no shell, so it cannot name
/// `$WISENT_OUTPUT_DIR`; entitlements-rotator and trading-tools ran
/// `scripts/release-build.sh` for that, a file neither checkout holds. Each
/// binary is placed at `$WISENT_OUTPUT_DIR/<name>`.
///
/// On a release worker (`WISENT_SOURCE_DIR` set) the source is the unpacked
/// `git archive` of the released commit: there is no repository to resolve
/// canonical sources from, and the commit was validated before the archive
/// existed. Both paths use the shared Cargo executor; a worker replaces locked
/// Git packages with its declared private-source input, while local execution
/// resolves canonical checkouts.
pub(super) fn stage(
    runtime: &Runtime,
    manifest: &Path,
    forwarded: &[String],
    json_output: bool,
) -> Result<i32> {
    let output = std::env::var_os("WISENT_OUTPUT_DIR")
        .map(PathBuf::from)
        .context("stage places binaries in WISENT_OUTPUT_DIR, which is not set")?;
    if !output.is_absolute() {
        bail!("WISENT_OUTPUT_DIR must be absolute");
    }
    let binaries: Vec<String> = forwarded
        .windows(2)
        .filter(|pair| pair[0] == "--bin")
        .map(|pair| pair[1].clone())
        .collect();
    if binaries.is_empty() {
        bail!("stage needs at least one --bin <name> to place");
    }
    let worker = std::env::var_os("WISENT_SOURCE_DIR").is_some();
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(directory) if worker => PathBuf::from(directory),
        _ => output.join("cargo-target"),
    };
    let mut arguments = vec![
        "--release".to_owned(),
        "--target-dir".to_owned(),
        target.to_string_lossy().into_owned(),
    ];
    arguments.extend(forwarded.iter().cloned());
    let mut report = execute(runtime, manifest, "build", &arguments)?.report;
    let succeeded = report.get("error").is_none();
    if succeeded {
        let mut placed = Vec::new();
        for binary in &binaries {
            let built = target.join("release").join(binary);
            if !built.is_file() {
                bail!(
                    "Cargo reported success but did not produce {}",
                    built.display()
                );
            }
            let destination = output.join(binary);
            fs::copy(&built, &destination).with_context(|| {
                format!("placing {} at {}", built.display(), destination.display())
            })?;
            placed.push(destination);
        }
        report["staged"] = json!(placed);
    }
    if json_output {
        emit(&report)?;
    } else if let Some(error) = report["error"].as_str() {
        eprintln!("{error}");
    } else {
        for path in report["staged"].as_array().into_iter().flatten() {
            println!("staged {}", path.as_str().unwrap_or_default());
        }
    }
    Ok(if succeeded { 0 } else { 1 })
}
