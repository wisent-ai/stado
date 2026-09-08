//! The `Provider` trait implementation for `GcpProvider`: zone rotation
//! with stockout/quota short-circuits and duplicate-VM recovery, the
//! idempotent delete, stop/start, existence and lifecycle-state reads, and
//! the accelerator census.

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config;
use crate::providers::gcp::client::GceError;
use crate::providers::gcp::stockout;
use crate::providers::{Provider, ProviderError};

use super::{log, py_bool, region_of_zone, GcpProvider};

#[async_trait]
impl Provider for GcpProvider {
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
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let client = &state.client;
        let store = &state.store;

        let zones = config::machine_type_zones()
            .get(machine_type)
            .cloned()
            .unwrap_or_else(|| config::zone_rotation().to_vec());
        // Track regions with confirmed QUOTA_EXCEEDED this call. GCP
        // enforces GPU quota at the regional level, so a 403 in one zone
        // means every other zone in the same region will also fail.
        // Without this short-circuit the loop wastes ~10 wall-seconds per
        // zone-retry inside the 60s Cloud Function tick budget — saturated
        // T4 quota in us-central1 alone burned 30+ seconds of every tick
        // today, causing 504s.
        let mut skip_regions: HashSet<String> = HashSet::new();
        for zone in &zones {
            let region = region_of_zone(zone);
            if skip_regions.contains(&region) {
                continue;
            }
            if stockout::zone_recently_stocked_out(store, zone).await? {
                log(&format!(
                    "skip {zone} (recent stockout, TTL {}s)",
                    stockout::STOCKOUT_TTL_S as i64
                ));
                continue;
            }
            // Cross-call quota cache: previous tick's create_instance found
            // this (region, accel) at QUOTA_EXCEEDED. Skip the API call —
            // quota doesn't change within the 60s TTL window.
            if !accel_type.is_empty()
                && stockout::region_recently_quota_exceeded(store, &region, accel_type).await?
            {
                log(&format!(
                    "skip {zone} ({accel_type} quota exhausted in {region}, TTL {}s)",
                    stockout::QUOTA_TTL_S as i64
                ));
                continue;
            }
            // A non-404 delete failure leaves the old VM's existence
            // ambiguous. Abort instead of trying another zone and risking
            // two live VMs writing the same job paths.
            let instance_path = format!(
                "/projects/{}/zones/{zone}/instances/{name}",
                client.project()
            );
            client
                .delete_allow_404(
                    &instance_path,
                    &format!("delete stale instance {name}@{zone}"),
                )
                .await?;
            match Self::attempt_zone(
                client,
                zone,
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
            {
                Ok(()) => {
                    log(&format!(
                        "Created {} preemptible={}",
                        Self::reference(name, zone),
                        py_bool(preemptible)
                    ));
                    return Ok(Some(Self::reference(name, zone)));
                }
                Err(exc) => {
                    let msg = exc.to_string();
                    if msg.contains("already exists") {
                        return Ok(Some(Self::reference(name, zone)));
                    }
                    // The GCE insert call returns an Operation the moment
                    // the API accepts the request. Waiting for completion
                    // polls; if that wait fails (SSL
                    // UNEXPECTED_EOF_WHILE_READING, RetryError on transient
                    // transport failure, etc.) AFTER the insert was already
                    // accepted server-side, the VM may still come up.
                    // Without a probe here the loop falls through to the
                    // next zone and create_instance spawns a SECOND live VM
                    // with the same name in a different zone. Two VMs
                    // sharing one job_id both write to the same GCS log
                    // path gs://wisent-compute/status/<job>/output/
                    // command_output.log, producing interleaved-writer logs
                    // and double-charged compute. Confirmed live
                    // 2026-05-15: Qwen3 job 724084db had concurrent
                    // subprocesses at step 539 (25s/step) and step 68
                    // (80s/step) in the same log. Probe this zone before
                    // continuing: if the VM actually exists, return its ref
                    // instead of falling through.
                    if let Ok(Some(status)) = client.instance_status(zone, name).await {
                        if matches!(status.as_str(), "RUNNING" | "STAGING" | "PROVISIONING") {
                            log(&format!(
                                "Recovered {} (insert accepted, operation wait raised {}); \
                                 returning existing ref to prevent duplicate in another zone",
                                Self::reference(name, zone),
                                gce_error_kind(&exc)
                            ));
                            return Ok(Some(Self::reference(name, zone)));
                        }
                    }
                    log(&format!("Failed in {zone}: {exc}"));
                    if msg.contains("QUOTA_EXCEEDED") {
                        skip_regions.insert(region.clone());
                        if !accel_type.is_empty() {
                            stockout::mark_region_quota_exceeded(store, &region, accel_type)
                                .await?;
                        }
                    }
                    if msg.contains("ZONE_RESOURCE_POOL_EXHAUSTED") || msg.contains("STOCKOUT") {
                        stockout::mark_zone_stockout(store, zone).await?;
                    }
                    continue;
                }
            }
        }
        Ok(None)
    }

    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let path = format!(
            "/projects/{}/zones/{zone}/instances/{name}",
            state.client.project()
        );
        // Idempotent: already-deleted instance is the desired terminal
        // state. Any other API error propagates so the caller sees it.
        state
            .client
            .delete_allow_404(&path, &format!("delete {instance_ref}"))
            .await?;
        Ok(())
    }

    async fn stop_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let path = format!(
            "/projects/{}/zones/{zone}/instances/{name}/stop",
            state.client.project()
        );
        state
            .client
            .post(&path, &json!({}), &format!("stop {instance_ref}"))
            .await?;
        Ok(())
    }

    async fn start_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let path = format!(
            "/projects/{}/zones/{zone}/instances/{name}/start",
            state.client.project()
        );
        state
            .client
            .post(&path, &json!({}), &format!("start {instance_ref}"))
            .await?;
        Ok(())
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        let Some(status) = state.client.instance_status(zone, name).await? else {
            return Ok(false);
        };
        Ok(matches!(
            status.as_str(),
            "RUNNING" | "STAGING" | "PROVISIONING"
        ))
    }

    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let (name, zone) = Self::parse_ref(instance_ref)?;
        Ok(state.client.instance_status(zone, name).await?)
    }

    /// Trait override delegating to the inherent method (kept for direct
    /// GcpProvider callers) so `&dyn Provider` consumers — the dead-agent
    /// reaper — can reach it.
    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        GcpProvider::list_running_instance_refs_with_age(self).await
    }

    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let state = self.state().await?;
        let filter = format!("name:{}-*", config::INSTANCE_PREFIX);
        let instances = state.client.aggregated_instances(&filter).await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for (_zone, instance) in instances {
            let status = instance.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(status, "RUNNING" | "STAGING" | "PROVISIONING") {
                continue;
            }
            if let Some(accelerators) = instance.get("guestAccelerators").and_then(Value::as_array)
            {
                for accel in accelerators {
                    let atype = accel
                        .get("acceleratorType")
                        .and_then(Value::as_str)
                        .and_then(|t| t.rsplit('/').next())
                        .unwrap_or("");
                    if atype.is_empty() {
                        continue;
                    }
                    let count = accel
                        .get("acceleratorCount")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    *counts.entry(atype.to_string()).or_insert(0) += count;
                }
            }
        }
        Ok(counts)
    }
}

/// The Python `type(exc).__name__` slot in the "Recovered ..." log line.
fn gce_error_kind(exc: &GceError) -> &'static str {
    match exc {
        GceError::Auth(_) => "DefaultCredentialsError",
        GceError::Http(_) => "RetryError",
        GceError::Api(_) => "GoogleAPICallError",
    }
}
