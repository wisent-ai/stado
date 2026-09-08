//! Where a fleet host keeps the Node toolchain, and running one `npm` verb
//! with the interpreter its shim needs.

use std::path::{Path, PathBuf};

use crate::cli::web::builds::tooling::command::run_with_path;
use crate::cli::CmdError;

/// Where a fleet host installs the Node toolchain, in probe order.
///
/// The same order `host_exec`'s candidate table probes, so a release step and
/// a `stado host exec node --version` on the same host cannot name different
/// binaries. The list exists because a release worker is a launchd job, and a
/// launchd job's `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin` unless something
/// set it — Homebrew is not on it. `charless-mac-mini` carries node v25.9.0
/// and npm 11.12.1 under `/opt/homebrew/bin`, and a build that trusted `PATH`
/// would have reported that host as having no Node toolchain at all.
const NODE_TOOLCHAIN_DIRECTORIES: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

/// The absolute path of one Node toolchain program, or its bare name.
///
/// Falling back to the bare name rather than refusing keeps a builder that
/// installs Node somewhere else working: `PATH` is then the answer, and
/// [`run`]'s own refusal names the toolchain if that fails too.
fn toolchain_program(name: &str) -> String {
    NODE_TOOLCHAIN_DIRECTORIES
        .iter()
        .map(|directory| PathBuf::from(directory).join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

/// Run one `npm` verb with the interpreter its shim needs.
///
/// `npm` is a JavaScript shim whose first line is `#!/usr/bin/env node`, so
/// running it with a `PATH` that does not carry `node` fails with
/// `env: node: No such file or directory` and says nothing about npm. The
/// directory the resolved `npm` came from is prepended to `PATH`, because the
/// interpreter a shim needs is always its sibling.
pub(in crate::cli::web::builds) fn npm(
    source: &Path,
    arguments: &[&str],
    variables: &[(String, String)],
) -> Result<(), CmdError> {
    let program = toolchain_program("npm");
    let mut path = std::env::var("PATH").unwrap_or_default();
    if let Some(directory) = PathBuf::from(&program).parent() {
        let directory = directory.to_string_lossy().into_owned();
        if !directory.is_empty() && !directory.starts_with("npm") {
            path = if path.is_empty() {
                directory
            } else {
                format!("{directory}:{path}")
            };
        }
    }
    run_with_path(
        source,
        &program,
        arguments,
        "Node toolchain",
        Some(&path),
        variables,
    )
}
