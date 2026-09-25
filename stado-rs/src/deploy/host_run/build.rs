//! Release build of one manifest inside the target account's managed run
//! area, through a Stado-approved Cargo only.

use serde::Serialize;

use crate::deploy::{host_channel, host_exec, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::{confined_file_prelude, BUILD_TIMEOUT};

#[derive(Debug, Serialize)]
pub struct BuildOutcome {
    pub target: String,
    pub manifest_path: String,
    pub binary: String,
    pub status: &'static str,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub async fn build(
    target: &ComputeTarget,
    manifest_path: &str,
    binary: &str,
    runner: &Runner,
) -> Result<BuildOutcome, DeployError> {
    // A release build takes gigabytes. On the control host that is the
    // difference between serving and `No space left on device` for every
    // role of its one process, so a host already under its low watermark is
    // refused before anything is compiled.
    let gates = crate::deploy::host_gates::read_host_gates(&target.name, runner).await?;
    if let (Some(free), Some(low)) = (gates.free_gb, gates.low_watermark_gb) {
        if free < low as f64 {
            return Err(DeployError(format!(
                "{}: {free:.1} GiB free is under its {low} GiB low watermark; a release build \
                 would take the host's services down. Reclaim space first \
                 (`stado space reclaim {} --apply --reason ...`)",
                target.name, target.name
            )));
        }
    }
    let mut script = String::from("set -uo pipefail\numask 077\n");
    script.push_str(&confined_file_prelude(manifest_path, "-f", "manifest"));
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
    script.push_str(
        "if [ -z \"$cargo\" ]; then printf '%s\\n' 'Cargo is not installed at a Stado-approved path' >&2; exit 69; fi\n",
    );
    script.push_str("export PATH=\"${cargo%/*}:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin\"\n");
    // One reused target inside the declared build-cache root, which the
    // build_caches cleaner owns, rather than a fresh multi-gigabyte target
    // inside every run directory.
    script
        .push_str("export CARGO_TARGET_DIR=\"$HOME/.stado/build-cache/host-build/cargo-target\"\n");
    script.push_str("mkdir -p \"$CARGO_TARGET_DIR\" || exit 73\n");
    script.push_str(&format!(
        "\"$cargo\" build --locked --release --manifest-path \"$path\" --bin {binary} || exit $?\n\
         # Only the executable enters the run directory, where run-attached\n\
         # accepts it and remove-run-directory takes it away again.\n\
         mkdir -p \"${{path%/*}}/target/release\" || exit 73\n\
         cp \"$CARGO_TARGET_DIR/release/\"{binary} \"${{path%/*}}/target/release/\"{binary} || exit 73\n",
        binary = shlex_quote(binary)
    ));

    let output =
        host_channel::run_script_with_timeout(target, &script, BUILD_TIMEOUT, runner).await?;
    Ok(BuildOutcome {
        target: target.name.clone(),
        manifest_path: manifest_path.to_string(),
        binary: binary.to_string(),
        status: if output.ok() { "built" } else { "failed" },
        exit_code: output.code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}
