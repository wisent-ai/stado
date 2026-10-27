use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::common::{emit, Runtime};

use super::execute_revision;

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
    revision: Option<&str>,
    forwarded: &[String],
    json_output: bool,
) -> Result<i32> {
    let output = std::env::var_os("WISENT_OUTPUT_DIR")
        .map(PathBuf::from)
        .context("stage places binaries in WISENT_OUTPUT_DIR, which is not set")?;
    if !output.is_absolute() {
        bail!("WISENT_OUTPUT_DIR must be absolute");
    }
    // Cargo reads `--bin` as an option that takes the argument after it as its
    // value, so each `--bin` names the binary that follows it.
    let mut remaining = forwarded.iter();
    let mut binaries = Vec::new();
    while let Some(argument) = remaining.next() {
        if argument == "--bin" {
            binaries.extend(remaining.next().cloned());
        }
    }
    if binaries.is_empty() {
        bail!("stage needs at least one --bin <name> to place");
    }
    let worker = std::env::var_os("WISENT_SOURCE_DIR").is_some();
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(directory) if worker => PathBuf::from(directory),
        _ => output.join("cargo-target"),
    };
    // The source is checked before the release build: a compile error is
    // found in a fraction of a release build's time, and no release build is
    // started on a source that does not compile. Both read the same targets.
    let mut selection = vec![
        "--target-dir".to_owned(),
        target.to_string_lossy().into_owned(),
    ];
    selection.extend(forwarded.iter().cloned());
    let check = execute_revision(runtime, manifest, revision, "check", &selection)?;
    let mut report = if check.code != libc::EXIT_SUCCESS {
        let cause = match check.report["error"].as_str() {
            Some(error) => error.to_owned(),
            None => format!(
                "cargo check exited {} and its report records no error: {}",
                check.code, check.report
            ),
        };
        let mut report = check.report;
        report["error"] = json!(format!(
            "cargo check failed, so the release build was not started: {cause}"
        ));
        report
    } else {
        let mut arguments = vec!["--release".to_owned()];
        arguments.extend(selection);
        execute_revision(runtime, manifest, revision, "build", &arguments)?.report
    };
    let succeeded = report.get("error").is_none();
    let mut placed = Vec::new();
    if succeeded {
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
        // The paths this command placed, printed as placed: read back from
        // the report with an empty-text default, a path that is not text
        // would print as nothing staged.
        for path in &placed {
            println!("staged {}", path.display());
        }
    }
    Ok(if succeeded {
        libc::EXIT_SUCCESS
    } else {
        libc::EXIT_FAILURE
    })
}
