//! The build-cache reader: the scan the janitor's `build_caches` cleaner
//! makes under the disk-full rule — the host's whole home, at any age — and
//! the verdicts it reads back.

use crate::deploy::{host_channel, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::invocation::{remote_command, validate_days, validate_root};
use super::report::{parse_report, BuildCacheDeclaration, BuildCacheReport};

/// Re-exported from [`crate::deploy::host_channel`], which now owns the one
/// definition of "this machine" the whole deploy family shares.
use crate::deploy::host_channel::target_is_this_host as target_is_local;

/// The scan the janitor makes on `target`: its home, read from the host.
pub async fn home_scan_for_target(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<BuildCacheDeclaration, DeployError> {
    Ok(BuildCacheDeclaration {
        root: host_channel::remote_home(target, runner).await?,
    })
}

/// Read cache verdicts under the scan root, at any age.
pub async fn report_declaration_on_host(
    target: &ComputeTarget,
    declaration: &BuildCacheDeclaration,
    runner: &Runner,
) -> BuildCacheReport {
    run_on_host(target, &declaration.root, "0", false, false, runner).await
}

/// Report or prune on one registry host.
pub async fn run_on_host(
    target: &ComputeTarget,
    root: &str,
    days: &str,
    apply: bool,
    force: bool,
    runner: &Runner,
) -> BuildCacheReport {
    let mut report = BuildCacheReport {
        target: target.name.clone(),
        entries: Vec::new(),
        error: None,
        timed_out: false,
    };
    if let Err(error) = validate_root(root).and_then(|()| validate_days(days)) {
        report.error = Some(error.0);
        return report;
    }
    // The refused roots are the target's, not this machine's: a Linux
    // operator reading a Mac still prunes the Mac's photo library. A local
    // target that declares no platform is this binary's platform.
    let darwin = if target.release_platform.is_empty() {
        target_is_local(target) && cfg!(target_os = "macos")
    } else {
        target.release_platform.starts_with("darwin")
    };
    let prune =
        crate::providers::local::disk_cleanup::build_caches::privacy_protected_parts(darwin);
    let command = remote_command(root, days, apply, force, prune);
    // The walk runs until it ends; its own exit and output are the answer.
    let result = if target_is_local(target) {
        let spec = CommandSpec::new(vec!["/bin/sh".to_string(), "-c".to_string(), command]);
        runner(spec).await
    } else if !target.has_ssh_connection() {
        report.error = Some(format!(
            "target {} has no SSH connection path and is not this host",
            target.name
        ));
        return report;
    } else {
        host_channel::run_script_to_completion(target, &command, runner)
            .await
            .map_err(|error| error.0)
    };
    match result {
        Ok(output) if output.ok() => report.entries = parse_report(&output.stdout),
        Ok(output) => {
            report.entries = parse_report(&output.stdout);
            report.error = Some(output.detail().trim().to_string());
        }
        Err(error) => report.error = Some(error),
    }
    report
}
