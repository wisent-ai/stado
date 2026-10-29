//! The script one Cargo operation runs on its host: the confined manifest,
//! a Stado-approved Cargo, the declared compiler cache, the host's one reused
//! Cargo target in its build cache, and the checkout's own toolchain.

use crate::deploy::{host_exec, shlex_quote, DeployError};
use crate::providers::local::work_base::BUILD_CACHE_LEAF;
use crate::targets::ComputeTarget;

use super::super::confined_file_prelude;
use super::CargoOperation;

/// The leaf below the host's build cache that every Cargo operation run
/// through `stado host cargo` shares, so a crate one revision compiled is
/// reused by the next instead of a fresh multi-gigabyte target per run.
const TARGET_LEAF: &str = "host-build/cargo-target";

/// The build cache root on `target`: its declared work root's, or the one
/// under the account's `~/.stado` when the registry declares none, as the
/// host's own agent and release worker read it.
pub(super) fn cache_root(target: &ComputeTarget) -> String {
    match target.work_root.as_deref() {
        Some(root) => format!("{}/{BUILD_CACHE_LEAF}", shlex_quote(root)),
        None => format!("\"$HOME/.stado/{BUILD_CACHE_LEAF}\""),
    }
}

/// The whole script. `manifest_path` is the absolute manifest inside the run
/// directory, `output` the run directory's `out` the built binaries are
/// copied into, `source` the commit and the SHA-256 of the archive it was
/// exported from; none of them is caller text, all were built by
/// [`super::run`].
pub(super) fn script(
    target: &ComputeTarget,
    manifest_path: &str,
    output: &str,
    source: (&str, &str),
    operation: CargoOperation,
    binaries: &[String],
    arguments: &[String],
) -> Result<String, DeployError> {
    let mut script = String::from("set -uo pipefail\numask 077\n");
    script.push_str(&confined_file_prelude(manifest_path, "-f", "manifest"));
    // The delivered tree is an export with no Git directory, so a build
    // script that stamps its revision reads it the way it does on a release
    // worker: from the source contract the worker states to a build.
    let (commit, archive_sha256) = source;
    script.push_str(&format!(
        "export WISENT_SOURCE_COMMIT={} WISENT_SOURCE_SHA256={}\n",
        shlex_quote(commit),
        shlex_quote(archive_sha256)
    ));
    script.push_str("cargo=''\n");
    for candidate in host_exec::cargo_candidates() {
        let candidate = if let Some(relative) = candidate.strip_prefix("~/") {
            format!("\"$HOME/{}\"", shlex_quote(relative))
        } else {
            shlex_quote(candidate)
        };
        script.push_str(&format!(
            "if [ -z \"$cargo\" ] && [ -x {candidate} ]; then cargo={candidate}; fi\n"
        ));
    }
    // Each refusal below exits with the status of the step that failed, which
    // `exit` without an argument returns.
    script.push_str(
        "if [ -z \"$cargo\" ]; then printf '%s\\n' 'Cargo is not installed at a Stado-approved path' >&2; fi\n\
         [ -n \"$cargo\" ] || exit\n",
    );
    // The chosen Cargo's directory and the account's own Cargo first, then
    // the fleet's declared tool path.
    script.push_str(&format!(
        "export PATH=\"${{cargo%/*}}:$HOME/.cargo/bin:{}\"\n",
        crate::primitives::tool_path::SEARCH_PATH
    ));
    // The declared compiler cache: installed on the target with its own Cargo
    // when absent or another version, then Cargo's RUSTC_WRAPPER.
    script.push_str(
        &stado_product::compiler_cache::shell_ensure().map_err(|error| {
            DeployError(format!("{error:#}"))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?,
    );
    script.push_str(&format!(
        "export CARGO_TARGET_DIR={}/{TARGET_LEAF}\nmkdir -p \"$CARGO_TARGET_DIR\" || exit\n",
        cache_root(target)
    ));
    // rustup picks the toolchain from the directory it is started in, never
    // from `--manifest-path`: started from the home it compiled with the
    // host's default toolchain, whatever `rust-toolchain.toml` the revision
    // pins. Starting in the manifest's directory makes the pin the one used.
    script.push_str("cd \"${path%/*}\" || exit\n");
    if operation == CargoOperation::Clippy {
        // A pinned toolchain installed with the minimal profile carries no
        // Clippy; the component is added to that toolchain, never elsewhere.
        script.push_str(
            "if command -v rustup >/dev/null 2>&1; then rustup component add clippy || exit; fi\n",
        );
    }
    let mut command = format!(
        "\"$cargo\" {} --locked --manifest-path \"$path\"",
        operation.word()
    );
    if operation == CargoOperation::Build {
        command.push_str(" --release");
    }
    for binary in binaries {
        command.push_str(&format!(" --bin {}", shlex_quote(binary)));
    }
    for argument in arguments {
        command.push(' ');
        command.push_str(&shlex_quote(argument));
    }
    script.push_str(&format!("{command}\nstatus=$?\n"));
    if operation == CargoOperation::Build && !binaries.is_empty() {
        // Only the executables leave the shared target, into the run's own
        // `out`, which the caller pulls before the run is removed.
        let output = shlex_quote(output);
        script.push_str(&format!(
            "if [ \"$status\" -eq 0 ]; then mkdir -p {output} || exit\n"
        ));
        for binary in binaries {
            script.push_str(&format!(
                "  cp \"$CARGO_TARGET_DIR/release/\"{binary} {output}/ || exit\n",
                binary = shlex_quote(binary)
            ));
        }
        script.push_str("fi\n");
    }
    script.push_str("exit \"$status\"\n");
    Ok(script)
}
