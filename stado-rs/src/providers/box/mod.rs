//! Box by ASCII provider adapter for fixed-shape Linux sandboxes.
//!
//! Port of `stado/providers/box/__init__.py`. The provider is a lifecycle
//! adapter: admission goes through `targets::box_capabilities`, capacity is
//! preflighted against the account limits endpoint, TTL renews via PATCH,
//! and release is stop-or-delete per `BOX_RELEASE_MODE`. Legacy
//! `delete_instance` calls on a box still referenced by a running/ job
//! bridge through the fenced cancel path
//! (`scheduler::dispatch::box::cancel_box_for_legacy_move`).

pub mod client;
pub mod http;
pub mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use async_trait::async_trait;

use crate::targets::{admit_job, box_capabilities, AdmissionDecision};

use super::{Provider, ProviderError};
pub use client::BoxClient;
pub use client::TtlUpdate;
pub use http::BoxHttpTransport;
pub use types::{
    BoxApiError, BoxCommandResult, BoxError, BoxEventPage, BoxInfo, BoxLimits, BoxPromptRun,
};

/// Python `_ACTIVE_STATES`.
fn active_states() -> &'static BTreeSet<&'static str> {
    static STATES: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
        BTreeSet::from([
            "init",
            "provisioning",
            "provisioned",
            "cloning",
            "ready",
            "idle",
            "running",
            "archiving",
        ])
    });
    &STATES
}

/// Python `_RUNNING_STATES`.
fn running_states() -> &'static BTreeSet<&'static str> {
    static STATES: LazyLock<BTreeSet<&'static str>> =
        LazyLock::new(|| BTreeSet::from(["ready", "idle", "running"]));
    &STATES
}

/// Python `_BOX_MACHINE_TYPES`.
const BOX_MACHINE_TYPES: [&str; 3] = ["", "box", "box-linux-4cpu-8gb"];

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
    fn shape_rejection(
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

/// Map the box-dispatch layer's error onto the provider error surface:
/// Box/storage failures keep their native variants; lease conflicts and
/// Python-style ValueError/RuntimeError collapse to the Value arm.
fn box_dispatch_to_provider_error(
    err: crate::scheduler::dispatch::r#box::BoxDispatchError,
) -> ProviderError {
    use crate::scheduler::dispatch::r#box::BoxDispatchError as Bde;
    match err {
        Bde::Box(err) => err.into(),
        Bde::Storage(err) => err.into(),
        other => ProviderError::Value(other.to_string()),
    }
}

#[async_trait]
impl Provider for BoxProvider {
    /// Python `create_instance`: the generic provider fields are a shape
    /// contract; a fitting request returns the new box id as the instance
    /// ref. Note: unlike GCP (which returns None on capacity exhaustion),
    /// Python Box raises `BoxConfigurationError` from preflight — preserved.
    async fn create_instance(
        &self,
        _name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<Option<String>, ProviderError> {
        if let Some(message) = Self::shape_rejection(
            machine_type,
            accel_type,
            boot_disk_gb,
            image,
            image_project,
            startup_script,
            preemptible,
        ) {
            return Err(ProviderError::Value(message));
        }
        Ok(Some(self.create_box(None).await?.box_id))
    }

    /// Python `delete_instance`: bridge the legacy CLI deletion call
    /// through the fenced cancel path when a running/ job still references
    /// this box; otherwise delete the box directly.
    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let store = crate::queue::JobStorage::with_bucket(crate::config::bucket()).await?;
        // Find a running/ job that still references this box. A manual
        // scan instead of JobStorage::list_jobs: the latter's
        // buffer_unordered closure trips rustc's "FnOnce is not general
        // enough" check when instantiated inside an async-trait method.
        let mut found: Option<crate::models::Job> = None;
        for path in store.list_paths("running/", 0).await? {
            // Strict-raise on corrupt JSON, like Python list_jobs.
            let Some(text) = store.download_text(&path).await? else {
                continue;
            };
            let candidate =
                crate::models::Job::from_json(&text).map_err(crate::queue::StorageError::Json)?;
            if crate::capabilities::ProviderId::Box.matches(&candidate.provider)
                && candidate.instance_ref.as_deref() == Some(instance_ref)
            {
                found = Some(candidate);
                break;
            }
        }
        let Some(mut job) = found else {
            self.client.delete_box(instance_ref).await?;
            return Ok(());
        };
        // Fenced cancel bridge: the Python path guarantees the scheduler
        // can't race a legacy delete against a live dispatch.
        let owner = format!("cli:{}", std::process::id());
        crate::scheduler::dispatch::r#box::cancel_box_for_legacy_move(
            &store, self, &mut job, &owner,
        )
        .await
        .map_err(box_dispatch_to_provider_error)
    }

    /// Python `instance_exists`: alive iff the box state is active; 404 is
    /// False.
    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        match self.client.get_box(instance_ref).await {
            Ok(info) => Ok(active_states().contains(info.state.as_str())),
            Err(BoxError::Api(api)) if api.status == 404 => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Python `instance_lifecycle_state`: raw state uppercased; 404 is None.
    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        match self.client.get_box(instance_ref).await {
            Ok(info) => Ok(Some(info.state.to_uppercase())),
            Err(BoxError::Api(api)) if api.status == 404 => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Python `list_running_instances`: `{"box-cpu": count}` over the
    /// running-state boxes, empty when none.
    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let count = self
            .client
            .list_boxes()
            .await?
            .iter()
            .filter(|b| running_states().contains(b.state.as_str()))
            .count() as i64;
        let mut out = BTreeMap::new();
        if count > 0 {
            out.insert("box-cpu".to_string(), count);
        }
        Ok(out)
    }
}
