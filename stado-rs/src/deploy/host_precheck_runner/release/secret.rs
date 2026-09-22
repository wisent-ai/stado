//! Placing one runner's signing credential on a host without it ever
//! appearing in a command line or in captured output.
//!
//! Split out of `release/install.rs`, which had grown past the module line
//! cap; what a profile declares it needs stays there.

use crate::deploy::host_precheck_runner::declaration::RunnerProfile;
use crate::deploy::host_precheck_runner::platform::Platform;
use crate::deploy::host_precheck_runner::verdict::report::command_failure;
use crate::deploy::{host_channel, production_runner, DeployError};
use crate::targets::ComputeTarget;

pub(super) async fn install_kronika_agent_secret(
    target: &ComputeTarget,
    platform: Platform,
    profile: &RunnerProfile,
    secret: &str,
) -> Result<String, DeployError> {
    let runner = production_runner();
    let secret_file = platform.kronika_agent_secret_file(profile);
    let runner_user = profile.slug.as_str();
    // macOS install(1) rejects /dev/stdin as a source. Create the destination
    // with its final owner and mode first, then let that owner replace only its
    // bytes through dd. The secret never appears in argv or command output.
    let prepared = host_channel::run_program(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/install",
            "-o",
            runner_user,
            "-g",
            runner_user,
            "-m",
            "600",
            "/dev/null",
            &secret_file,
        ],
        &runner,
    )
    .await?;
    if !prepared.ok() {
        return Err(DeployError(format!(
            "{}: cannot prepare the Probierz signing credential file: {}",
            target.name,
            command_failure(&prepared, "remote secret file preparation failed")
        )));
    }
    let destination = format!("of={secret_file}");
    let written = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "-u",
            runner_user,
            "/bin/dd",
            &destination,
            "bs=4096",
        ],
        secret,
        &runner,
    )
    .await?;
    if !written.ok() {
        return Err(DeployError(format!(
            "{}: Probierz signing identity installation failed: {}",
            target.name,
            command_failure(&written, "remote secret write failed")
        )));
    }
    Ok(secret_file)
}
