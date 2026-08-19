//! Abstract provider interface and provider registry.
//!
//! Port of `stado/providers/base.py` (the `Provider` ABC) and
//! `stado/providers/__init__.py` (the `get_provider` factory). Python
//! methods are sync and return `None`/raise; here the trait is async and
//! fallible, with `create_instance` returning `Ok(None)` on capacity
//! exhaustion (provider-specific, per the Python contract).
//!
//! The `vast` module is NOT a `Provider` implementation: on Vast.ai
//! wisent-compute is the host, not the renter — it is the marketplace
//! host-listing bridge ported from `stado/providers/vast/`.

pub mod aws;
pub mod azure;
pub mod r#box;
pub mod gcp;
pub mod local;
pub mod vast;

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::capabilities::{ComputeAdapter, RuntimeAdapter, RuntimeFacet};
use async_trait::async_trait;

pub use r#box::BoxProvider;

/// Provider-layer error. Python raises `ValueError` for unknown provider
/// names and shape rejections, provider-specific exceptions otherwise.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Box provider failures (configuration/transport/API/value).
    #[error(transparent)]
    Box(#[from] r#box::BoxError),
    /// GCP (GCE REST) failures.
    #[error(transparent)]
    Gcp(#[from] gcp::GceError),
    /// AWS (EC2) failures. The message carries the service error code
    /// (e.g. `InsufficientInstanceCapacity`, `InvalidInstanceID.NotFound`)
    /// so Python's substring classification works on `error.to_string()`.
    #[error("{0}")]
    Aws(String),
    /// Azure (ARM REST) failures.
    #[error(transparent)]
    Azure(#[from] azure::AzureError),
    /// Storage failures from provider code that consults the queue.
    #[error(transparent)]
    Storage(#[from] crate::queue::StorageError),
    /// Python `ValueError` (unknown provider, shape rejection).
    #[error("{0}")]
    Value(String),
    /// Explicit phase-3 stub for provider surface not ported yet.
    #[error("{0}")]
    NotImplemented(String),
}

/// Python `providers.base.Provider`.
///
/// `instance_ref` is the `"name@zone"`-style opaque handle returned by
/// [`Provider::create_instance`] (the box provider uses the raw box id).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Create instance. Returns the `"name@zone"` ref, or `None` on
    /// capacity exhaustion. When `preemptible` is true the instance is
    /// launched as Spot/Preemptible — cheaper but can be terminated by the
    /// provider at any time.
    // The 9-parameter shape is the Python base.Provider contract.
    #[allow(clippy::too_many_arguments)]
    async fn create_instance(
        &self,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<Option<String>, ProviderError>;

    /// Create a workload-agent instance and, when supplied, deliver its
    /// dedicated grant over a provider-native protected channel. The default
    /// rejects grants so a provider can never silently fall back to startup
    /// metadata. Azure overrides this with a protected VM extension.
    #[allow(clippy::too_many_arguments)]
    async fn create_agent_instance(
        &self,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
        agent_grant: Option<&str>,
    ) -> Result<Option<String>, ProviderError> {
        if agent_grant.is_some() {
            return Err(ProviderError::Value(
                "provider has no protected agent-grant delivery channel".to_string(),
            ));
        }
        self.create_instance(
            name,
            machine_type,
            accel_type,
            boot_disk_gb,
            image,
            image_project,
            startup_script,
            preemptible,
        )
        .await
    }

    /// Delete instance by ref. NotFound is an idempotent success.
    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError>;

    /// Stop an existing instance while preserving its disks and identity.
    async fn stop_instance(&self, _instance_ref: &str) -> Result<(), ProviderError> {
        Err(ProviderError::NotImplemented(
            "provider does not support stopping instances".to_string(),
        ))
    }

    /// Start a previously stopped instance.
    async fn start_instance(&self, _instance_ref: &str) -> Result<(), ProviderError> {
        Err(ProviderError::NotImplemented(
            "provider does not support starting instances".to_string(),
        ))
    }

    /// Check if instance is alive (RUNNING/STAGING/PROVISIONING).
    ///
    /// Returns false for TERMINATED, STOPPED, or missing instances. Use
    /// [`Provider::instance_lifecycle_state`] to distinguish
    /// preempted-TERMINATED from actually-gone.
    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError>;

    /// Return the raw lifecycle state ("RUNNING"/"TERMINATED"/"STOPPED"/
    /// None).
    ///
    /// Optional method — providers that don't implement it return None and
    /// the reaper falls back to the [`Provider::instance_exists`] boolean
    /// check (Python base-class default).
    async fn instance_lifecycle_state(
        &self,
        _instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }

    /// Return `{accel_type: count}` for all wisent-* instances.
    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError>;

    /// Return `(name@zone, age_seconds)` for running agent VMs. Optional —
    /// default empty (providers without a VM fleet); the dead-agent reaper
    /// then has nothing to reap.
    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        Ok(vec![])
    }
}

/// Python `get_provider(name)`. All cloud arms are lazy: credentials and
/// clients resolve on the first API call, so the factory itself stays
/// cheap and infallible (see the gcp/aws/azure module docs).
pub fn get_provider(name: &str) -> Result<Arc<dyn Provider>, ProviderError> {
    let variant = crate::capabilities::constructible_variant(RuntimeFacet::Compute, name)
        .ok_or_else(|| ProviderError::Value(format!("Unknown provider: {name}")))?;
    match variant.adapter {
        RuntimeAdapter::Compute(ComputeAdapter::Box) => Ok(Arc::new(BoxProvider::from_env()?)),
        // Lazy: credentials + storage resolve on the first API call, so the
        // factory itself stays cheap and infallible (see gcp module docs).
        RuntimeAdapter::Compute(ComputeAdapter::Gcp) => Ok(Arc::new(gcp::GcpProvider::from_env())),
        RuntimeAdapter::Compute(ComputeAdapter::Aws) => Ok(Arc::new(aws::AwsProvider::from_env())),
        RuntimeAdapter::Compute(ComputeAdapter::Azure) => {
            Ok(Arc::new(azure::AzureProvider::from_env()))
        }
        _ => Err(ProviderError::Value(format!(
            "Provider {} has no constructible compute adapter",
            variant.id
        ))),
    }
}
