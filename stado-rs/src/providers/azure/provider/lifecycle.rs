//! The [`Provider`] trait implementation for [`AzureProvider`]: VM create,
//! delete, stop, start, and the fleet read-backs the reaper consumes.

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use serde_json::Value;

use crate::catalog::AZURE_VM_TO_ACCEL;
use crate::config;
use crate::providers::{Provider, ProviderError};

use super::super::arm::{vm_path, COMPUTE_API_VERSION};
use super::super::builders::{nic_skip_error, power_state, vm_body, vm_is_alive, vm_skip_error};
use super::super::network;
use super::{log, py_bool, AzureProvider};

#[async_trait]
impl Provider for AzureProvider {
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
        let agent_grant = agent_grant
            .filter(|grant| !grant.is_empty())
            .ok_or_else(|| {
                ProviderError::Value(
                    "Azure agent creation requires protected-settings grant delivery".to_string(),
                )
            })?;
        let reference = self
            .create_instance(
                name,
                machine_type,
                accel_type,
                boot_disk_gb,
                image,
                image_project,
                startup_script,
                preemptible,
            )
            .await?;
        let Some(reference) = reference else {
            return Ok(None);
        };
        if let Err(error) = self
            .install_agent_grant_extension(&reference, agent_grant)
            .await
        {
            if let Err(cleanup_error) = self.delete_instance(&reference).await {
                log(&format!(
                    "protected agent-grant delivery failed for {reference}; VM cleanup also failed: {cleanup_error}"
                ));
            }
            return Err(error);
        }
        Ok(Some(reference))
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_instance(
        &self,
        name: &str,
        machine_type: &str,
        _accel_type: &str,
        boot_disk_gb: i64,
        _image: &str,
        _image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let client = &state.client;
        let rg = config::azure_resource_group();
        let mut skipped: HashSet<String> = HashSet::new();
        for location in config::azure_locations() {
            if skipped.contains(location) {
                continue;
            }
            let subnet = network::subnet_id(
                client.subscription(),
                rg,
                config::azure_vnet(),
                config::azure_subnet(),
                location,
            );
            let nsg = network::nsg_id(client.subscription(), rg, config::azure_nsg(), location);
            let nic_id = match network::create_nic(client, rg, name, location, &subnet, &nsg).await
            {
                Ok(id) => id,
                Err(err) => {
                    let msg = err.to_string();
                    log(&format!("NIC create failed in {location}: {err}"));
                    if nic_skip_error(&msg) {
                        skipped.insert(location.clone());
                    }
                    continue;
                }
            };

            let body = vm_body(
                name,
                location,
                machine_type,
                boot_disk_gb,
                config::azure_image_urn(),
                config::azure_vm_username(),
                config::azure_ssh_public_key(),
                startup_script,
                &nic_id,
                config::azure_vm_identity_id(),
                preemptible,
            )
            .map_err(ProviderError::Value)?;
            let path = format!(
                "{}?api-version={COMPUTE_API_VERSION}",
                vm_path(client.subscription(), rg, name)
            );
            match client
                .put_lro(&path, &body, &format!("create VM {name}@{location}"))
                .await
            {
                Ok(_) => {
                    log(&format!(
                        "Created {} preemptible={}",
                        Self::reference(name, location),
                        py_bool(preemptible)
                    ));
                    return Ok(Some(Self::reference(name, location)));
                }
                Err(err) => {
                    let msg = err.to_string();
                    if msg.to_lowercase().contains("already exists") {
                        return Ok(Some(Self::reference(name, location)));
                    }
                    log(&format!("VM create failed in {location}: {err}"));
                    // Roll back the NIC we just created so we don't leak
                    // it.
                    network::delete_nic(client, rg, name).await;
                    if vm_skip_error(&msg) {
                        skipped.insert(location.clone());
                    }
                    continue;
                }
            }
        }
        Ok(None)
    }

    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let rg = config::azure_resource_group();
        let path = format!(
            "{}?api-version={COMPUTE_API_VERSION}",
            vm_path(state.client.subscription(), rg, name)
        );
        // Idempotent: already gone is the desired terminal state.
        state
            .client
            .delete_allow_404(&path, &format!("delete VM {name}"))
            .await?;
        // NIC cleanup mirrors the VM-delete contract: NotFound is
        // idempotent success, and failures are best-effort log-only
        // inside delete_nic (Python network.py swallows all exceptions).
        network::delete_nic(&state.client, rg, name).await;
        Ok(())
    }

    async fn stop_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let path = format!(
            "{}/deallocate?api-version={COMPUTE_API_VERSION}",
            vm_path(
                state.client.subscription(),
                config::azure_resource_group(),
                name,
            )
        );
        state
            .client
            .post_action(&path, &format!("deallocate VM {name}"))
            .await?;
        Ok(())
    }

    async fn start_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let path = format!(
            "{}/start?api-version={COMPUTE_API_VERSION}",
            vm_path(
                state.client.subscription(),
                config::azure_resource_group(),
                name,
            )
        );
        state
            .client
            .post_action(&path, &format!("start VM {name}"))
            .await?;
        Ok(())
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let Some(vm) = state
            .client
            .get_vm(config::azure_resource_group(), name)
            .await?
        else {
            return Ok(false);
        };
        let prov = vm
            .get("properties")
            .and_then(|p| p.get("provisioningState"))
            .and_then(Value::as_str);
        Ok(vm_is_alive(prov, power_state(&vm).as_deref()))
    }

    /// Return the literal Azure power-state ('running', 'deallocated',
    /// ...).
    ///
    /// The monitor uses lifecycle_state == "TERMINATED" (GCE) to detect
    /// Spot preemption. On Azure, Spot eviction lands the VM in
    /// PowerState/deallocated — the monitor treats that string as the
    /// preemption signal.
    async fn instance_lifecycle_state(
        &self,
        instance_ref: &str,
    ) -> Result<Option<String>, ProviderError> {
        let state = self.state().await?;
        let name = Self::parse_ref(instance_ref)?;
        let Some(vm) = state
            .client
            .get_vm(config::azure_resource_group(), name)
            .await?
        else {
            return Ok(None);
        };
        Ok(power_state(&vm))
    }

    /// Trait override delegating to the inherent method (kept for direct
    /// AzureProvider callers) so `&dyn Provider` consumers — the dead-agent
    /// reaper and `cli/instances.rs` — can reach it. Without this the base
    /// default applied and every Azure agent VM was invisible to both.
    /// Mirrors providers/gcp.
    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        AzureProvider::list_running_instance_refs_with_age(self).await
    }

    /// `{accel_type: count}` for all live wisent-* VMs across the
    /// resource group.
    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let state = self.state().await?;
        let vms = state
            .client
            .list_vms(config::azure_resource_group())
            .await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        let prefix = format!("{}-", config::INSTANCE_PREFIX);
        for vm in &vms {
            let name = vm.get("name").and_then(Value::as_str).unwrap_or("");
            if !name.starts_with(&prefix) {
                continue;
            }
            // Cheap state probe: list() doesn't include instance_view by
            // default, so trust the tag we stamped at create-time. A VM
            // in the resource group with the wisent_managed tag and a
            // known GPU SKU consumes quota until we delete it; counting
            // it as running is the safe direction.
            let sku = vm
                .get("properties")
                .and_then(|p| p.get("hardwareProfile"))
                .and_then(|h| h.get("vmSize"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let Some((accel, n)) = AZURE_VM_TO_ACCEL.get(sku) else {
                continue;
            };
            *counts.entry((*accel).to_string()).or_insert(0) += n;
        }
        Ok(counts)
    }
}
