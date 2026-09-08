//! `service release`: one product release installed, activated and proven
//! ready, or rolled back to the version that was there.

use super::*;

mod bundle;
mod readiness;
pub(crate) mod run;
mod source;

use bundle::{current_service_version, service_release_bundle, stage_service_release_archive};
use readiness::wait_for_service_readiness;
use source::{record_released_service_source, rollback_service_release};

use super::super::declare::ensure::run::ensure;
use super::super::declare::ensure::EnsureOptions;

/// The host, and the managed declaration this release will move — after the
/// unit has been converged onto the always-on domain a release requires.
async fn release_convergence(
    options: &ServiceReleaseOptions<'_>,
) -> Result<(targets::ComputeTarget, Vec<ManagedService>), CmdError> {
    let target = host_channel::canonical_target(options.host)
        .await
        .map_err(click)?;
    let mut services = declared_matching(options.name, Some(options.host)).await?;
    let Some(current) = services.first() else {
        return Err(CmdError::click(format!(
            "{} does not manage {}; deploy it first",
            options.host, options.name
        )));
    };
    if service::requires_daemon_domain(&target)
        && UnitDomain::from_path(&current.path).is_per_login()
    {
        let reason = format!(
            "release {} {} requires an always-on system service",
            options.product, options.version
        );
        ensure(EnsureOptions {
            name: options.name,
            host: options.host,
            from: None,
            args: &[],
            env: &[],
            reason: &reason,
            as_daemon: true,
            as_launch_agent: false,
            as_json: false,
        })
        .await?;
        services = declared_matching(options.name, Some(options.host)).await?;
    }
    Ok((target, services))
}
