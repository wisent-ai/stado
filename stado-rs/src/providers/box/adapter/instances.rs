//! The generic `Provider` surface of the Boat adapter.
//!
//! `create_instance`, `delete_instance`, `instance_exists`,
//! `instance_lifecycle_state` and `list_running_instances`, together with
//! the dispatch-error lift that `delete_instance` needs for its fenced
//! cancel bridge.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::providers::{Provider, ProviderError};

use super::super::types::{BoxError, HTTP_NOT_FOUND};
use super::BoxProvider;

/// Map the Boat-dispatch layer's error onto the provider error surface:
/// Boat and storage failures keep their native variants; lease conflicts
/// and argument or runtime failures collapse to the Value arm.
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
    /// The generic provider fields are a shape contract; a fitting request
    /// returns the new sandbox id as the instance ref. An account that
    /// cannot start a sandbox is refused by the preflight with Boat's reason.
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
        Ok(Some(self.create_box(self.ttl_seconds).await?.box_id))
    }

    /// Bridge a deletion through the fenced cancel path when a running/ job
    /// still references this sandbox; otherwise delete the sandbox directly.
    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let store = crate::queue::JobStorage::with_bucket(crate::config::bucket()).await?;
        // Find a running/ job that still references this sandbox, one job at
        // a time: a buffered read's closure trips rustc's "FnOnce is not
        // general enough" check when instantiated inside an async-trait
        // method, and the scan stops at the first match.
        let mut found: Option<crate::models::Job> = None;
        for job_id in store.list_job_ids("running").await? {
            let Some(candidate) = store.read_job("running", &job_id).await? else {
                continue;
            };
            if candidate.state == crate::models::job_state::RUNNING
                && crate::capabilities::ProviderId::Boat.matches(&candidate.provider)
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
        // Fenced cancel bridge: the scheduler cannot race a deletion against
        // a live dispatch.
        let owner = format!("cli:{}", std::process::id());
        crate::scheduler::dispatch::r#box::cancel_box_for_legacy_move(
            &store, self, &mut job, &owner,
        )
        .await
        .map_err(box_dispatch_to_provider_error)
    }

    /// Alive while the sandbox state is active; a sandbox Boat no longer
    /// holds is not alive.
    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        match self.client.get_box(instance_ref).await {
            Ok(info) => Ok(info.lifecycle().active()),
            Err(BoxError::Api(api)) if api.status == HTTP_NOT_FOUND => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// The sandbox's state word and its meaning; `None` once Boat no longer
    /// holds it.
    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<crate::providers::Lifecycle>, ProviderError> {
        match self.client.get_box(instance_ref).await {
            Ok(info) => Ok(Some(crate::providers::Lifecycle {
                phase: info.lifecycle().phase(),
                word: info.state,
            })),
            Err(BoxError::Api(api)) if api.status == HTTP_NOT_FOUND => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// `{"box-cpu": count}` over the serving sandboxes, empty when none.
    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let serving = self
            .client
            .list_boxes()
            .await?
            .iter()
            .filter(|b| b.lifecycle().serving())
            .count();
        let mut out = BTreeMap::new();
        if let Some(serving) = std::num::NonZeroUsize::new(serving) {
            let count = i64::try_from(serving.get()).map_err(|error| {
                ProviderError::Value(format!(
                    "{serving} serving Boat sandboxes do not fit a count: {error}"
                ))
            })?;
            out.insert("box-cpu".to_string(), count);
        }
        Ok(out)
    }
}
