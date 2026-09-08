//! Moving a managed unit onto a new version: `update` for an artifact or a
//! bundle, `release` for a product release behind a readiness gate, and the
//! restart, stop and install steps both of them are built from.

use super::*;

pub(crate) mod gate;
pub(crate) mod install;
pub(crate) mod unit;
pub(crate) mod update;

use gate::run::release;
use install::archive::{install_from_archive, ROLLBACK_BODY};
use install::current::follow_current;
use install::{archive_members, install_from_artifact, refuse_archive_without_program};
use update::update;

pub(crate) struct ServiceReleaseOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) product: &'a str,
    pub(crate) version: &'a str,
    pub(crate) readiness_url: Option<&'a str>,
    pub(crate) readiness_timeout_seconds: u64,
    pub(crate) reload_unit: bool,
    pub(crate) require_release_version: bool,
    pub(crate) supersede_unit: Option<&'a str>,
    pub(crate) supersede_same_label_user: bool,
    pub(crate) json: bool,
    pub(crate) emit: bool,
}

pub(crate) async fn release_pipeline_product(
    name: &str,
    host: &str,
    product: &str,
    version: &str,
    readiness_url: &str,
    readiness_timeout_seconds: u64,
) -> Result<(), CmdError> {
    release(ServiceReleaseOptions {
        name,
        host,
        product,
        version,
        readiness_url: Some(readiness_url),
        readiness_timeout_seconds,
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
