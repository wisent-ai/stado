//! Moving a managed unit onto a new version: `update` for an artifact or a
//! bundle, `release` for a product release behind a readiness gate, and the
//! restart, stop and install steps both of them are built from.

use super::*;

pub(crate) mod awaited;
pub(crate) mod gate;
pub(crate) mod install;
pub(crate) mod unit;
pub(crate) mod update;

use gate::run::release;
pub(crate) use install::archive::install_from_archive;
use install::archive::ROLLBACK_BODY;
use install::current::follow_current;
use install::{archive_members, install_from_artifact, refuse_archive_without_program};
use update::update;

/// The loopback endpoint a released unit must answer after its restart and
/// the whole seconds it has to answer: declared together or not at all, so a
/// release without readiness carries no window nobody stated.
#[derive(Clone, Copy)]
pub(crate) struct ServiceReadiness<'a> {
    pub(crate) url: &'a str,
    pub(crate) window_seconds: std::num::NonZeroU64,
}

pub(crate) struct ServiceReleaseOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) product: &'a str,
    pub(crate) version: &'a str,
    pub(crate) readiness: Option<ServiceReadiness<'a>>,
    pub(crate) reload_unit: bool,
    pub(crate) require_release_version: bool,
    pub(crate) supersede_unit: Option<&'a str>,
    pub(crate) supersede_same_label_user: bool,
    pub(crate) json: bool,
    pub(crate) emit: bool,
}

/// One replace pass of a product release onto a managed unit. `Deferred` is
/// not a failure: the unit was left as it was because the running service
/// named work a restart would end, and the pipeline's next pass tries again.
pub(crate) async fn release_pipeline_product(
    name: &str,
    host: &str,
    product: &str,
    version: &str,
    readiness_url: &str,
    readiness_window_seconds: u64,
) -> Result<gate::ReleaseOutcome, CmdError> {
    let window_seconds = std::num::NonZeroU64::new(readiness_window_seconds).ok_or_else(|| {
        CmdError::declaration(format!(
            "release control declares a readiness window of {readiness_window_seconds}s \
                 for {product}; a replace release needs at least one second"
        ))
    })?;
    release(ServiceReleaseOptions {
        name,
        host,
        product,
        version,
        readiness: Some(ServiceReadiness {
            url: readiness_url,
            window_seconds,
        }),
        reload_unit: false,
        require_release_version: true,
        supersede_unit: None,
        supersede_same_label_user: true,
        json: false,
        emit: false,
    })
    .await
}

#[derive(Default, Deserialize)]
struct ObservedServiceRelease {
    active_version: Option<String>,
    active_sha256: Option<String>,
}

struct ServiceReleaseBundle {
    artifact: crate::release_control::ReleaseArtifactRef,
    archive: Vec<u8>,
    rollout_generation: u64,
    previous_version: Option<String>,
    previous_sha256: Option<String>,
}
