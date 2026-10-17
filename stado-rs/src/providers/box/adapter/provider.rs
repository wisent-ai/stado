//! `BoxProvider`, the Boat adapter, and the sandbox-lifecycle verbs it owns
//! directly.
//!
//! Construction from the profile's `boat.*` settings, capability admission,
//! the account-limits preflight, and the create / renew / release verbs.
//! The `Provider` implementation lives in `instances` beside this file.

use crate::capabilities::ProviderId;
use crate::targets::{admit_job, box_capabilities, AdmissionDecision};

use super::super::client::{BoxClient, TtlUpdate};
use super::super::http;
use super::super::types::{BoxError, BoxInfo, BoxState, HTTP_NOT_FOUND};
use super::states::BOX_MACHINE_TYPES;

/// What becomes of a sandbox Stado releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseMode {
    Stop,
    Delete,
}

/// One `boat.*` setting from the selected profile or its environment
/// override, as the capability catalog declares it; empty when neither
/// carries it.
fn setting(key: &str) -> String {
    crate::config::compute_setting(ProviderId::Boat, key)
        .trim()
        .to_string()
}

/// The Boat adapter: sandbox lifecycle; command and prompt execution inside
/// a sandbox is dispatched separately.
#[derive(Debug, Clone)]
pub struct BoxProvider {
    pub client: BoxClient,
    pub ttl_seconds: i64,
    pub release_mode: ReleaseMode,
}

impl BoxProvider {
    /// Build a profile-configured client whose API key is read from the
    /// Skarbiec item that plays `cloud-boat` at request time. `boat.api_url`
    /// names another API base than Boat's public one; `boat.ttl_seconds` and
    /// `boat.release_mode` are the deployment's to declare and the provider
    /// is refused without them, before any sandbox exists that it could not
    /// end.
    pub fn from_env() -> Result<Self, BoxError> {
        let base_url = setting("api-url");
        let base_url = if base_url.is_empty() {
            http::DEFAULT_BASE_URL.to_string()
        } else {
            base_url
        };
        let client = BoxClient::from_skarbiec(&base_url)?;
        let ttl = setting("ttl-seconds");
        if ttl.is_empty() {
            return Err(BoxError::configuration(
                "boat.ttl_seconds (environment override BOAT_TTL_SECONDS) is not declared: \
                 how long a Boat sandbox lives is the deployment's to say",
            ));
        }
        let ttl: i64 = ttl.parse().map_err(|error| {
            BoxError::configuration(format!(
                "boat.ttl_seconds (environment override BOAT_TTL_SECONDS) is {ttl:?}, not a \
                 whole number of seconds: {error}"
            ))
        })?;
        let release_mode = match setting("release-mode").to_lowercase().as_str() {
            "stop" => ReleaseMode::Stop,
            "delete" => ReleaseMode::Delete,
            "" => {
                return Err(BoxError::configuration(
                    "boat.release_mode (environment override BOAT_RELEASE_MODE) is not \
                     declared: whether a released Boat sandbox is stopped or deleted is the \
                     deployment's to say",
                ))
            }
            other => {
                return Err(BoxError::configuration(format!(
                    "boat.release_mode (environment override BOAT_RELEASE_MODE) is {other:?}; \
                     declare stop or delete"
                )))
            }
        };
        Self::new(client, ttl, release_mode)
    }

    /// An adapter over `client` with a positive sandbox lifetime.
    pub fn new(
        client: BoxClient,
        ttl_seconds: i64,
        release_mode: ReleaseMode,
    ) -> Result<Self, BoxError> {
        if !ttl_seconds.is_positive() {
            return Err(BoxError::configuration(format!(
                "boat.ttl_seconds (environment override BOAT_TTL_SECONDS) is {ttl_seconds}; a \
                 Boat sandbox's lifetime is a positive number of seconds"
            )));
        }
        Ok(BoxProvider {
            client,
            ttl_seconds,
            release_mode,
        })
    }

    /// The lifetime a sandbox for `job` is created and renewed with: the
    /// job's own when it states one, else the deployment's.
    pub fn ttl_for(&self, job: &crate::models::Job) -> i64 {
        if job.box_ttl_seconds.is_positive() {
            job.box_ttl_seconds
        } else {
            self.ttl_seconds
        }
    }

    /// Capability admission against the sandbox the adapter creates.
    pub fn admit(&self, job: &crate::models::Job) -> AdmissionDecision {
        admit_job(job, box_capabilities())
    }

    /// The account must be able to start a sandbox and have room for one
    /// more active sandbox.
    pub async fn preflight(&self) -> Result<(), BoxError> {
        let limits = self.client.limits().await?;
        if !limits.can_start {
            let reason = if !limits.blocked_reason.is_empty() {
                format!(
                    "the Boat account cannot start a sandbox: {}",
                    limits.blocked_reason
                )
            } else if !limits.billing_status.is_empty() {
                format!(
                    "the Boat account cannot start a sandbox; its billing status is {}",
                    limits.billing_status
                )
            } else {
                "the Boat account cannot start a sandbox and Boat named no reason".to_string()
            };
            return Err(BoxError::configuration(reason));
        }
        if limits.max_active_boxes.is_positive() && limits.active_boxes >= limits.max_active_boxes {
            return Err(BoxError::configuration(format!(
                "the Boat account holds {} active sandboxes of the {} its plan allows",
                limits.active_boxes, limits.max_active_boxes
            )));
        }
        Ok(())
    }

    /// Preflight, then create a sandbox that Boat stops by itself after
    /// `ttl_seconds`.
    pub async fn create_box(&self, ttl_seconds: i64) -> Result<BoxInfo, BoxError> {
        self.preflight().await?;
        self.client.create_box(Some(ttl_seconds), true).await
    }

    /// Move the sandbox's auto-stop `ttl_seconds` forward.
    pub async fn renew_box(&self, box_id: &str, ttl_seconds: i64) -> Result<BoxInfo, BoxError> {
        self.client
            .update_box(box_id, None, TtlUpdate::Set(ttl_seconds))
            .await
    }

    /// A stopped or missing sandbox is already released; a live one is
    /// stopped or deleted as the declared release mode says.
    pub async fn release_box(&self, box_id: &str) -> Result<(), BoxError> {
        let info = match self.client.get_box(box_id).await {
            Ok(info) => info,
            Err(BoxError::Api(api)) if api.status == HTTP_NOT_FOUND => return Ok(()),
            Err(err) => return Err(err),
        };
        if matches!(info.lifecycle(), BoxState::Archived | BoxState::Cancelled) {
            return Ok(());
        }
        let result = match self.release_mode {
            ReleaseMode::Delete => self.client.delete_box(box_id).await,
            ReleaseMode::Stop => self.client.stop_box(box_id).await.map(|_| ()),
        };
        match result {
            Ok(()) => Ok(()),
            // Gone already, or already stopped.
            Err(BoxError::Api(api))
                if api.status == HTTP_NOT_FOUND || api.code == "machine_not_running" =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Why the generic provider fields of a create request do not fit the
    /// sandbox this adapter creates, or `None` when they fit.
    pub(super) fn shape_rejection(
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Option<String> {
        let mut reasons: Vec<&str> = Vec::new();
        if !BOX_MACHINE_TYPES.contains(&machine_type) {
            reasons.push("Boat creates one sandbox size");
        }
        if !accel_type.is_empty() {
            reasons.push("a Boat sandbox has no accelerator");
        }
        if boot_disk_gb > box_capabilities().disk_gb {
            reasons.push("the requested disk exceeds the Boat sandbox's disk");
        }
        if !image.is_empty() || !image_project.is_empty() {
            reasons.push("Boat does not boot a caller-selected image");
        }
        if !startup_script.is_empty() {
            reasons.push("Boat does not accept cloud startup scripts");
        }
        if preemptible {
            reasons.push("Boat has no preemptible sandbox");
        }
        if reasons.is_empty() {
            None
        } else {
            Some(reasons.join("; "))
        }
    }
}
