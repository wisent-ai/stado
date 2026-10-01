//! The build-cache reader: resolve one host's cleaner from its declaration or
//! from the reporting default, and read the verdicts back.

use crate::deploy::{host_channel, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::invocation::{remote_command, validate_days, validate_root};
use super::report::{parse_report, BuildCacheDeclaration, BuildCacheReport};

/// Re-exported from [`crate::deploy::host_channel`], which now owns the one
/// definition of "this machine" the whole deploy family shares.
use crate::deploy::host_channel::target_is_this_host as target_is_local;

/// Resolve the build-cache cleaner from the target's own cleanup declaration,
/// or from the reporting default a target that declares nothing is measured
/// against.
///
/// The default is not this function's invention: it is
/// [`crate::targets::DiskCleanupPolicy::reporting_default`], which the janitor
/// resolves undeclared hosts against, and whose whole point is that silence
/// in the registry means "nobody has said", not "do not look". Refusing
/// instead would give one declaration two answers: the janitor reports an
/// undeclared host's reclaimable caches while `stado space report` and
/// `stado host build-caches` say the host declares no policy at all. A leased
/// scratch target meets it every time — the registry `stado scratch create`
/// emits declares no `disk_cleanup`.
///
/// Nothing here arms a cleaner. The default's mode is `report`, deleting stays
/// an explicit registry declaration, and the two refusals below are unchanged:
/// a declared policy naming no `build_caches` cleaner, and a root that is
/// neither absolute nor home-relative.
pub async fn declared_for_target(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<BuildCacheDeclaration, DeployError> {
    let policy = match target.disk_cleanup.clone() {
        Some(policy) => policy,
        None => crate::targets::DiskCleanupPolicy::reporting_default(),
    };
    let cleaner = policy.cleaners.get("build_caches").ok_or_else(|| {
        DeployError(format!(
            "{} declares no build cache cleaner; add it to registry targets[].disk_cleanup.cleaners.build_caches",
            target.name
        ))
    })?;
    let configured = cleaner.root.as_deref();
    let root = match configured {
        Some(root) if root.starts_with('/') => root.to_string(),
        Some("~") => host_channel::remote_home(target, runner).await?,
        Some(root) if root.starts_with("~/") => {
            let home = host_channel::remote_home(target, runner).await?;
            format!("{home}/{}", root.trim_start_matches("~/"))
        }
        Some(root) => {
            return Err(DeployError(format!(
                "{} declares build cache root {root:?} outside an absolute or home-relative path; fix registry targets[].disk_cleanup.cleaners.build_caches.root",
                target.name
            )))
        }
        None => host_channel::remote_home(target, runner).await?,
    };
    Ok(BuildCacheDeclaration {
        root,
        min_age_seconds: cleaner.min_age_seconds,
    })
}

/// Read cache verdicts from one already-resolved cleaner declaration.
pub async fn report_declaration_on_host(
    target: &ComputeTarget,
    declaration: &BuildCacheDeclaration,
    runner: &Runner,
) -> BuildCacheReport {
    let min_age_days = declaration
        .min_age_seconds
        .saturating_sub(1)
        .div_euclid(86_400)
        .to_string();
    run_on_host(
        target,
        &declaration.root,
        &min_age_days,
        false,
        false,
        runner,
    )
    .await
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
