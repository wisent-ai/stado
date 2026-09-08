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
    script.push_str(&format!(
        "\"$cargo\" build --locked --release --manifest-path \"$path\" --bin {}\n",
        shlex_quote(binary)
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
