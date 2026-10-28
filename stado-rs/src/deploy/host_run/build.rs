//! Release build of one manifest inside the target account's managed run
//! area, through a Stado-approved Cargo only.

use serde::Serialize;

use crate::deploy::{host_channel, host_exec, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::confined_file_prelude;

#[derive(Debug, Serialize)]
pub struct BuildOutcome {
    pub target: String,
    pub manifest_path: String,
    pub binary: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Free bytes on the target's volume before the build and after it, from
    /// the host's own `df -Pk` reading (`stado host gates`); `None` when the
    /// host reported no reading.
    pub free_bytes_before: Option<u64>,
    pub free_bytes_after: Option<u64>,
    /// Why there is no after-build reading, when the host did not answer it.
    /// The build's own result stands either way.
    pub free_bytes_after_error: Option<String>,
}

impl BuildOutcome {
    /// The volume's free space this build took, in GiB: what the two
    /// readings differ by, other writers on the volume included.
    pub fn consumed_gib(&self) -> Option<f64> {
        let (Some(before), Some(after)) = (self.free_bytes_before, self.free_bytes_after) else {
            return None;
        };
        let gib = crate::providers::local::host_memory::constants::GIB as f64;
        Some((before as f64 - after as f64) / gib)
    }
}

pub async fn build(
    target: &ComputeTarget,
    manifest_path: &str,
    binary: &str,
    runner: &Runner,
) -> Result<BuildOutcome, DeployError> {
    // A release build takes gigabytes. On the control host that is the
    // difference between serving and `No space left on device` for every
    // role of its one process, so a host whose volume is already at the
    // disk-full threshold is refused before anything is compiled.
    let gates = crate::deploy::host_gates::read_host_gates(&target.name, runner).await?;
    let threshold = crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT;
    if let Some(used) = gates
        .used_percent
        .filter(|used| *used >= f64::from(threshold))
    {
        return Err(DeployError::unreachable(format!(
            "{}: its volume is {used:.1}% used, at the {threshold}% disk-full threshold; a \
             release build would take the host's services down while its janitor deletes \
             everything the fleet put there (`stado space report {}`)",
            target.name, target.name
        )));
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
    // Each refusal below exits with the status of the step that failed (the
    // Cargo check, mkdir, cp), which `exit` without an argument returns.
    script.push_str(
        "if [ -z \"$cargo\" ]; then printf '%s\\n' 'Cargo is not installed at a Stado-approved path' >&2; fi\n\
         [ -n \"$cargo\" ] || exit\n",
    );
    script.push_str("export PATH=\"${cargo%/*}:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin\"\n");
    // The declared compiler cache: installed on the target with its own Cargo
    // when absent or another version, then Cargo's RUSTC_WRAPPER, so a crate
    // the target compiled for any earlier build is restored.
    script.push_str(
        &stado_product::compiler_cache::shell_ensure()
            // Its only failure is reading the compiler cache declaration.
            .map_err(|error| {
                DeployError(format!("{error:#}"))
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?,
    );
    // One reused target inside the fleet's build cache, which the
    // build_caches cleaner takes at the disk-full threshold, rather than a
    // fresh multi-gigabyte target inside every run directory.
    script
        .push_str("export CARGO_TARGET_DIR=\"$HOME/.stado/build-cache/host-build/cargo-target\"\n");
    script.push_str("mkdir -p \"$CARGO_TARGET_DIR\" || exit\n");
    let binary_word = shlex_quote(binary);
    script.push_str(&format!(
        "\"$cargo\" build --locked --release --manifest-path \"$path\" --bin {binary_word} || exit $?\n\
         # Only the executable enters the run directory, where run attach\n\
         # accepts it and run remove takes it away again.\n\
         mkdir -p \"${{path%/*}}/target/release\" || exit\n\
         cp \"$CARGO_TARGET_DIR/release/\"{binary_word} \"${{path%/*}}/target/release/\"{binary_word} || exit\n"
    ));

    let output = host_channel::run_script(target, &script, runner).await?;
    // The same reading again, so the outcome says what the build took from
    // the volume; a host that does not answer it leaves the build's own
    // result standing and says why the reading is missing.
    let (free_bytes_after, free_bytes_after_error) =
        match crate::deploy::host_gates::read_host_gates(&target.name, runner).await {
            Ok(after) => (after.free_bytes, None),
            Err(error) => (None, Some(error.to_string())),
        };
    Ok(BuildOutcome {
        target: target.name.clone(),
        manifest_path: manifest_path.to_string(),
        binary: binary.to_string(),
        exit_code: output.code,
        stdout: output.stdout,
        stderr: output.stderr,
        free_bytes_before: gates.free_bytes,
        free_bytes_after,
        free_bytes_after_error,
    })
}
