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
            if matches!(candidate.provider.as_str(), "box" | "box-ascii")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Job;
    use crate::testutil::{http_response, mock_http};

    const BX: &str = "bx_2abcdefg";

    async fn provider_for(responses: Vec<String>) -> (crate::testutil::MockHttp, BoxProvider) {
        let server = mock_http(responses).await;
        let transport = BoxHttpTransport::new_for_test("box_testkey", &server.base_url, 5.0);
        let provider = BoxProvider::new(BoxClient::from_transport(transport), 7200).unwrap();
        (server, provider)
    }

    fn box_job() -> Job {
        let mut job = Job::new("j1", "echo hi");
        job.executor = "box-prompt".into();
        job
    }

    #[tokio::test]
    async fn admit_matches_box_capabilities() {
        let (server, provider) = provider_for(vec![]).await;
        // A CPU-only box-prompt job fits the sandbox.
        assert!(provider.admit(&box_job()).accepted);
        // GPU work is rejected (Box has no accelerator).
        let mut gpu_job = box_job();
        gpu_job.gpu_mem_gb = 24;
        let decision = provider.admit(&gpu_job);
        assert!(!decision.accepted);
        assert!(
            decision
                .reasons
                .iter()
                .any(|r| r.contains("no accelerator")),
            "{decision:?}"
        );
        // Unsupported executor.
        let mut bad_exec = box_job();
        bad_exec.executor = "weird".into();
        let decision = provider.admit(&bad_exec);
        assert!(!decision.accepted);
        assert!(
            decision.reasons.iter().any(|r| r.contains("unsupported")),
            "{decision:?}"
        );
        server.stop();
    }

    #[test]
    fn shape_rejection_joins_reasons_like_python() {
        assert_eq!(
            BoxProvider::shape_rejection("", "", 80, "", "", "", false),
            None
        );
        assert_eq!(
            BoxProvider::shape_rejection("box-linux-4cpu-8gb", "", 80, "", "", "", false),
            None
        );
        assert_eq!(
            BoxProvider::shape_rejection("n1-standard-4", "nvidia-l4", 500, "img", "", "", true)
                .unwrap(),
            "Box has one fixed machine shape; Box has no accelerator; \
             requested disk exceeds fixed Box disk; \
             Box does not support a caller-selected image; \
             Box does not expose preemptible lifecycle"
        );
        assert_eq!(
            BoxProvider::shape_rejection("", "", 80, "", "proj", "script", false).unwrap(),
            "Box does not support a caller-selected image; Box does not accept cloud startup scripts"
        );
    }

    #[tokio::test]
    async fn preflight_blocks_on_limits() {
        // canStart=false with a blocked reason.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "limits.info", "canStart": false, "startBlockedReason": "pay up"}"#,
        )])
        .await;
        let err = provider.preflight().await.unwrap_err();
        assert_eq!(err.to_string(), "pay up");
        server.stop();

        // Active-box capacity exhausted.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "limits.info", "canStart": true,
                "activeBoxes": 5, "maxActiveBoxes": 5}"#,
        )])
        .await;
        let err = provider.preflight().await.unwrap_err();
        assert_eq!(err.to_string(), "Box active-box capacity is exhausted");
        server.stop();

        // Headroom -> ok.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "limits.info", "canStart": true,
                "activeBoxes": 1, "maxActiveBoxes": 5}"#,
        )])
        .await;
        provider.preflight().await.unwrap();
        server.stop();
    }

    #[tokio::test]
    async fn create_box_preflights_then_creates_with_provider_ttl() {
        let (server, provider) = provider_for(vec![
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "limits.info", "canStart": true}"#,
            ),
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.created", "box": {"id": "bx_2abcdefg", "state": "provisioning"}}"#,
            ),
        ])
        .await;
        let info = provider.create_box(None).await.unwrap();
        assert_eq!(info.box_id, BX);
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1].ends_with(r#"{"ttlSeconds":7200,"noEnv":true}"#),
            "{}",
            requests[1]
        );
        server.stop();
    }

    #[tokio::test]
    async fn renew_box_patches_ttl() {
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.updated", "box": {"id": "bx_2abcdefg"}}"#,
        )])
        .await;
        provider.renew_box(BX, None).await.unwrap();
        assert!(server.requests.lock().unwrap()[0].ends_with(r#"{"ttlSeconds":7200}"#));
        server.stop();
    }

    #[tokio::test]
    async fn release_box_modes_and_idempotency() {
        // stop mode: GET then POST stop.
        let (server, provider) = provider_for(vec![
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "running"}}"#,
            ),
            http_response(200, "OK", r#"{"ok": true, "type": "box.stopping"}"#),
        ])
        .await;
        provider.release_box_with_mode(BX, "stop").await.unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1].starts_with("POST /boxes/bx_2abcdefg/stop "),
            "{}",
            requests[1]
        );
        server.stop();

        // delete mode: GET then DELETE.
        let (server, provider) = provider_for(vec![
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "idle"}}"#,
            ),
            http_response(200, "OK", r#"{"ok": true, "type": "box.deleted"}"#),
        ])
        .await;
        provider
            .release_box_with_mode(BX, " Delete ")
            .await
            .unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[1].starts_with("DELETE /boxes/bx_2abcdefg "),
            "{}",
            requests[1]
        );
        server.stop();

        // Invalid mode is a configuration error (after the GET).
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "running"}}"#,
        )])
        .await;
        let err = provider
            .release_box_with_mode(BX, "nuke")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "BOX_RELEASE_MODE must be stop or delete");
        server.stop();

        // 404 on GET -> already released.
        let (server, provider) = provider_for(vec![http_response(404, "Not Found", "{}")]).await;
        provider.release_box_with_mode(BX, "stop").await.unwrap();
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        server.stop();

        // Archived -> no action.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "archived"}}"#,
        )])
        .await;
        provider.release_box_with_mode(BX, "stop").await.unwrap();
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        server.stop();

        // machine_not_running on stop is tolerated.
        let (server, provider) = provider_for(vec![
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "ready"}}"#,
            ),
            http_response(409, "Conflict", r#"{"code": "machine_not_running", "message": "stopped"}"#),
        ])
        .await;
        provider.release_box_with_mode(BX, "stop").await.unwrap();
        server.stop();
    }

    #[tokio::test]
    async fn lifecycle_adapter_maps_states_and_404() {
        // exists: active state.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "archiving"}}"#,
        )])
        .await;
        assert!(Provider::instance_exists(&provider, BX).await.unwrap());
        server.stop();

        // exists: non-active state.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "archived"}}"#,
        )])
        .await;
        assert!(!Provider::instance_exists(&provider, BX).await.unwrap());
        server.stop();

        // exists: 404 -> false; lifecycle: 404 -> None.
        let (server, provider) = provider_for(vec![
            http_response(404, "Not Found", "{}"),
            http_response(404, "Not Found", "{}"),
        ])
        .await;
        assert!(!Provider::instance_exists(&provider, BX).await.unwrap());
        assert_eq!(
            Provider::instance_lifecycle_state(&provider, BX)
                .await
                .unwrap(),
            None
        );
        server.stop();

        // lifecycle: uppercased raw state.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "ready"}}"#,
        )])
        .await;
        assert_eq!(
            Provider::instance_lifecycle_state(&provider, BX)
                .await
                .unwrap()
                .as_deref(),
            Some("READY")
        );
        server.stop();
    }

    #[tokio::test]
    async fn list_running_instances_counts_running_states() {
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.list",
                "boxes": [
                    {"id": "bx_2abcdefg", "state": "ready"},
                    {"id": "bx_3bcdefgh", "state": "provisioning"},
                    {"id": "bx_4cdefghj", "state": "running"},
                    {"id": "bx_5defghjk", "state": "archived"}
                ],
                "pageInfo": {"hasMore": false}}"#,
        )])
        .await;
        let counts = Provider::list_running_instances(&provider).await.unwrap();
        assert_eq!(counts, BTreeMap::from([("box-cpu".to_string(), 2)]));
        server.stop();

        // Empty fleet -> empty map.
        let (server, provider) = provider_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.list", "boxes": [], "pageInfo": {"hasMore": false}}"#,
        )])
        .await;
        assert!(Provider::list_running_instances(&provider)
            .await
            .unwrap()
            .is_empty());
        server.stop();
    }

    #[tokio::test]
    async fn create_instance_rejects_unfittable_shape_before_network() {
        let (server, provider) = provider_for(vec![]).await;
        let err = Provider::create_instance(
            &provider,
            "wisent-x",
            "n1-standard-4",
            "",
            80,
            "",
            "",
            "",
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "Box has one fixed machine shape");
        assert!(server.requests.lock().unwrap().is_empty());
        server.stop();
    }
}
