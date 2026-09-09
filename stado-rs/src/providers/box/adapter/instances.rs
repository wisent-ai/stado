//! The generic `Provider` surface of the Box adapter.
//!
//! Python `create_instance`, `delete_instance`, `instance_exists`,
//! `instance_lifecycle_state` and `list_running_instances`, together with
//! the dispatch-error lift that `delete_instance` needs for its fenced
//! cancel bridge.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::providers::{Provider, ProviderError};

use super::super::types::BoxError;
use super::states::{active_states, running_states};
use super::BoxProvider;

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
            if candidate.state == crate::models::job_state::RUNNING
                && crate::capabilities::ProviderId::Box.matches(&candidate.provider)
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
