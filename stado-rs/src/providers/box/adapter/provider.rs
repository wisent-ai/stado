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
use super::super::types::{BoxError, BoxInfo};
use super::states::BOX_MACHINE_TYPES;

/// Python `BoxProvider`: lifecycle adapter; structured workload execution
/// (box-command / box-prompt dispatch) is handled separately.
#[derive(Debug, Clone)]
pub struct BoxProvider {
    pub client: BoxClient,
    pub ttl_seconds: i64,
}

impl BoxProvider {
    /// Build an environment-configured client whose API key is resolved from
    /// `stado-box/api_key` in Skarbiec at request time.
    pub fn from_env() -> Result<Self, BoxError> {
        let base_url =
            std::env::var("BOX_API_URL").unwrap_or_else(|_| http::DEFAULT_BASE_URL.to_string());
        let timeout: f64 = std::env::var("BOX_API_TIMEOUT_SECONDS")
            .unwrap_or_else(|_| "70".to_string())
            .parse()
            .map_err(|_| BoxError::configuration("BOX_API_TIMEOUT_SECONDS must be a number"))?;
        let client = BoxClient::from_skarbiec(&base_url, timeout)?;
        Self::from_client_env_ttl(client)
    }

    /// The TTL half of [`BoxProvider::from_env`], split out so tests can
    /// bind a mock-transport client without touching `BOX_API_*` env.
    fn from_client_env_ttl(client: BoxClient) -> Result<Self, BoxError> {
        let ttl: i64 = std::env::var("BOX_TTL_SECONDS")
            .unwrap_or_else(|_| "7200".to_string())
            .parse()
            .map_err(|_| BoxError::configuration("BOX_TTL_SECONDS must be an integer"))?;
        Self::new(client, ttl)
    }

    /// Python `BoxProvider(client=..., ttl_seconds=...)`.
    pub fn new(client: BoxClient, ttl_seconds: i64) -> Result<Self, BoxError> {
        if ttl_seconds <= 0 {
            return Err(BoxError::configuration("BOX_TTL_SECONDS must be positive"));
        }
        Ok(BoxProvider {
            client,
            ttl_seconds,
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
    /// the mode comes from `BOX_RELEASE_MODE` (default "stop").
    pub async fn release_box(&self, box_id: &str) -> Result<(), BoxError> {
        let mode = std::env::var("BOX_RELEASE_MODE").unwrap_or_else(|_| "stop".to_string());
        self.release_box_with_mode(box_id, &mode).await
    }

    /// [`BoxProvider::release_box`] with the mode passed explicitly (the
    /// env lookup is split out so tests don't race on `BOX_RELEASE_MODE`).
    pub async fn release_box_with_mode(&self, box_id: &str, mode: &str) -> Result<(), BoxError> {
        let info = match self.client.get_box(box_id).await {
            Ok(info) => info,
            Err(BoxError::Api(api)) if api.status == 404 => return Ok(()),
            Err(err) => return Err(err),
        };
        if info.state == "archived" {
            return Ok(());
        }
        let result = match mode.trim().to_lowercase().as_str() {
            "delete" => self.client.delete_box(box_id).await,
            "stop" => self.client.stop_box(box_id).await.map(|_| ()),
            _ => {
                return Err(BoxError::configuration(
                    "BOX_RELEASE_MODE must be stop or delete",
                ));
            }
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
