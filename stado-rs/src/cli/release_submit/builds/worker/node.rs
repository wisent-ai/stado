//! The Node.js a Node product's release steps run.
//!
//! A source whose root carries `package.json` is built with `npm` and checked
//! with `npx`, and a Linux builder has no Node of its own: Weles' linux-amd64
//! release failed its formatting gate with `npx: not found` before a line was
//! built. The worker installs the release Stado declares
//! ([`stado_product::node_runtime`]) before the first step, and the step
//! search path already puts `~/.local/bin`, where its programs are linked,
//! ahead of the inherited one. A builder that cannot install it refuses the
//! build naming the step that failed.

use std::path::Path;

use crate::cli::CmdError;

pub(super) fn ensure_for(source: &Path) -> Result<(), CmdError> {
    if !source.join("package.json").is_file() {
        return Ok(());
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| CmdError::refused("HOME is not set; Node.js is installed under it"))?;
    let report = stado_product::node_runtime::ensure(&home).map_err(|error| {
        CmdError::click(format!("{error:#}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    println!(
        "[release-worker] node runtime: {} {}",
        report["node"], report["version"]
    );
    Ok(())
}
