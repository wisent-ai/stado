//! The web platform's quality gate, as the handoff check runs it.
//!
//! A web product's formatting, typing and lint checks are its own `package.json`
//! scripts, and the gate its recipe declares is `stado web quality` with
//! `"kind": "format"`, which runs them on the release worker. The handoff runs
//! that gate over the exported tree and under the worker contract the release
//! worker sets, so the verdict is the one the release will reach.

use std::path::Path;

use crate::cli::CmdError;
use crate::release_pipeline::{self, ReleasePipelineManifest};

/// Whether `platform` is the platform whose recipe runs `stado web`.
pub(super) fn is_web_platform(platform: &str) -> bool {
    platform == crate::cli::web::WEB_PLATFORM
}

/// The version the manifest at `root` declares, which `stado web quality`
/// checks `package.json` against.
pub(super) fn declared_version(
    root: &Path,
    manifest: &ReleasePipelineManifest,
) -> Result<String, CmdError> {
    release_pipeline::declared_version(&manifest.version_source, |path| {
        std::fs::read(root.join(path)).map_err(|error| format!("{path}: {error}"))
    })
    .map_err(CmdError::declaration)
}

/// The release worker's contract a gate reads, pointed at `tree`: the source
/// is the exported tree, staged output goes inside it, and the version and
/// platform are the ones the release would cut. `stado web quality` and a
/// product's own gate script (Brama's `src/release/quality.sh`) both read it.
pub(super) fn worker_contract(
    tree: &Path,
    version: &str,
    platform: &str,
) -> Result<Vec<(&'static str, String)>, CmdError> {
    let output = tree.join(".wisent-output").join("quality-gate");
    std::fs::create_dir_all(&output).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", output.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Ok(vec![
        ("WISENT_SOURCE_DIR", tree.to_string_lossy().into_owned()),
        ("WISENT_OUTPUT_DIR", output.to_string_lossy().into_owned()),
        ("WISENT_VERSION", version.to_owned()),
        ("WISENT_PLATFORM", platform.to_owned()),
    ])
}
