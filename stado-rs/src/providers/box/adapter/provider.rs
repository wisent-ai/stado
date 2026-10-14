//! `BoxProvider` and the box-lifecycle verbs it owns directly.
//!
//! The struct plus the Python `BoxProvider` methods that sit outside the
//! generic provider trait: construction from the `BOX_API_*` / `BOX_TTL_*`
//! environment, capability admission, the account-limits preflight, and
//! the create / renew / release verbs. The `Provider` implementation
//! lives in `instances` beside this file.

use crate::targets::{admit_job, box_capabilities, AdmissionDecision};

use super::super::client::{BoxClient, TtlUpdate};
use super::super::http;
use super::super::types::{BoxError, BoxInfo, BoxState};
use super::states::BOX_MACHINE_TYPES;

/// What becomes of a box Stado releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseMode {
    Stop,
    Delete,
}

/// One Box setting: none when unset or blank, refused by name when its value
/// is not text.
fn setting(name: &str) -> Result<Option<String>, BoxError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(BoxError::configuration(format!(
            "{name} cannot be read: {error}"
        ))),
    }
}

/// Python `BoxProvider`: lifecycle adapter; structured workload execution
/// (box-command / box-prompt dispatch) is handled separately.
#[derive(Debug, Clone)]
pub struct BoxProvider {
    pub client: BoxClient,
    pub ttl_seconds: i64,
    pub release_mode: ReleaseMode,
}

impl BoxProvider {
    /// Build an environment-configured client whose API key is resolved from
    /// `stado-box/api_key` in Skarbiec at request time. `BOX_API_URL` names
    /// another API base than Box's public one; `BOX_TTL_SECONDS` and
    /// `BOX_RELEASE_MODE` are the deployment's to declare and the provider is
    /// refused without them, before any box exists that it could not end.
    pub fn from_env() -> Result<Self, BoxError> {
        let base_url = match setting("BOX_API_URL")? {
            Some(base_url) => base_url,
            None => http::DEFAULT_BASE_URL.to_string(),
        };
        let client = BoxClient::from_skarbiec(&base_url)?;
        let Some(ttl) = setting("BOX_TTL_SECONDS")? else {
            return Err(BoxError::configuration(
                "BOX_TTL_SECONDS is not declared: how long a box lives is the deployment's to say",
            ));
        };
        let ttl: i64 = ttl
            .trim()
            .parse()
            .map_err(|_| BoxError::configuration("BOX_TTL_SECONDS must be an integer"))?;
        let release_mode = match setting("BOX_RELEASE_MODE")?.map(|mode| mode.trim().to_lowercase()) {
            Some(mode) if mode == "stop" => ReleaseMode::Stop,
            Some(mode) if mode == "delete" => ReleaseMode::Delete,
            Some(_) => return Err(BoxError::configuration("BOX_RELEASE_MODE must be stop or delete")),
            None => {
                return Err(BoxError::configuration(
                    "BOX_RELEASE_MODE is not declared: whether a released box is stopped or deleted is the deployment's to say",
                ))
            }
        };
        Self::new(client, ttl, release_mode)
    }

    /// Python `BoxProvider(client=..., ttl_seconds=...)`.
    pub fn new(
        client: BoxClient,
        ttl_seconds: i64,
        release_mode: ReleaseMode,
    ) -> Result<Self, BoxError> {
        if !ttl_seconds.is_positive() {
            return Err(BoxError::configuration("BOX_TTL_SECONDS must be positive"));
        }
        Ok(BoxProvider {
            client,
            ttl_seconds,
            release_mode,
        })
    }

    /// Python `admit`: capability admission against the fixed box shape.
    pub fn admit(&self, job: &crate::models::Job) -> AdmissionDecision {
        admit_job(job, box_capabilities())
    }

    /// Python `preflight`: the account must be able to start a box and have
    /// active-box headroom.
    pub async fn preflight(&self) -> Result<(), BoxError> {
        let limits = self.client.limits().await?;
        if !limits.can_start {
            let reason = if !limits.blocked_reason.is_empty() {
                limits.blocked_reason
            } else if !limits.billing_status.is_empty() {
                limits.billing_status
            } else {
                "Box account cannot start a box".to_string()
            };
            return Err(BoxError::configuration(reason));
        }
        if limits.max_active_boxes != 0 && limits.active_boxes >= limits.max_active_boxes {
            return Err(BoxError::configuration(
                "Box active-box capacity is exhausted",
            ));
        }
        Ok(())
    }

    /// Python `create_box`: preflight, then create with the provider TTL
    /// when the caller did not pin one.
    pub async fn create_box(&self, ttl_seconds: Option<i64>) -> Result<BoxInfo, BoxError> {
        self.preflight().await?;
        self.client
            .create_box(Some(ttl_seconds.unwrap_or(self.ttl_seconds)), true)
            .await
    }

    /// Python `renew_box`: PATCH the TTL forward.
    pub async fn renew_box(
        &self,
        box_id: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<BoxInfo, BoxError> {
        let ttl = ttl_seconds.unwrap_or(self.ttl_seconds);
        self.client
            .update_box(box_id, None, TtlUpdate::Set(ttl))
            .await
    }

    /// Python `release_box`: archived/missing boxes are already released;
    /// a live one is stopped or deleted as the declared release mode says.
    pub async fn release_box(&self, box_id: &str) -> Result<(), BoxError> {
        let info = match self.client.get_box(box_id).await {
            Ok(info) => info,
            Err(BoxError::Api(api)) if api.status == 404 => return Ok(()),
            Err(err) => return Err(err),
        };
        if info.lifecycle() == BoxState::Archived {
            return Ok(());
        }
        let result = match self.release_mode {
            ReleaseMode::Delete => self.client.delete_box(box_id).await,
            ReleaseMode::Stop => self.client.stop_box(box_id).await.map(|_| ()),
        };
        match result {
            Ok(()) => Ok(()),
            // 404 = already gone; machine_not_running = already stopped.
            Err(BoxError::Api(api)) if api.status == 404 || api.code == "machine_not_running" => {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Python `create_instance`'s shape validation, split out so the
    /// rejection reasons are testable without network. Returns the joined
    /// `ValueError` message when the shape doesn't fit the fixed box.
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
            reasons.push("Box has one fixed machine shape");
        }
        if !accel_type.is_empty() {
            reasons.push("Box has no accelerator");
        }
        if boot_disk_gb > box_capabilities().disk_gb {
            reasons.push("requested disk exceeds fixed Box disk");
        }
        if !image.is_empty() || !image_project.is_empty() {
            reasons.push("Box does not support a caller-selected image");
        }
        if !startup_script.is_empty() {
            reasons.push("Box does not accept cloud startup scripts");
        }
        if preemptible {
            reasons.push("Box does not expose preemptible lifecycle");
        }
        if reasons.is_empty() {
            None
        } else {
            Some(reasons.join("; "))
        }
    }
}
