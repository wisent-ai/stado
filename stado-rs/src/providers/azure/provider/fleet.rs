//! What the resource group holds, read from ARM: whether a VM whose create
//! was refused stands anyway, and how much accelerator capacity the
//! wisent-* VMs hold.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::catalog::AZURE_VM_TO_ACCEL;
use crate::config;
use crate::providers::ProviderError;

use super::super::super::arm::{ArmClient, AzureError};
use super::super::{log, AzureProvider};

impl AzureProvider {
    /// The reference of VM `name` when it stands although its create in
    /// `location` was refused with `refusal`. Whether it was created is
    /// ARM's answer to a read of it, not words in the refusal: a create
    /// refused because the VM already stands leaves it readable, in the
    /// location it stands in. A VM that cannot be read after the refusal
    /// stops the create: whether it stands is unknown, and trying another
    /// location could create a second VM of the same name.
    pub(super) async fn standing_after_refusal(
        client: &ArmClient,
        rg: &str,
        name: &str,
        location: &str,
        refusal: &AzureError,
    ) -> Result<Option<String>, ProviderError> {
        let vm = match client.get_vm(rg, name).await {
            Ok(Some(vm)) => vm,
            Ok(None) => return Ok(None),
            Err(read) => {
                return Err(ProviderError::Value(format!(
                    "VM create of {name} in {location} was refused ({refusal}), and reading {name} \
                     afterwards failed too ({read}); no other location is tried"
                )));
            }
        };
        match vm.get("location").and_then(Value::as_str) {
            Some(standing) => {
                log(&format!(
                    "VM create in {location} was refused ({refusal}); {name} stands in {standing}"
                ));
                Ok(Some(Self::reference(name, standing)))
            }
            None => Err(ProviderError::Value(format!(
                "VM {name} stands after its create was refused ({refusal}), but ARM's answer names no location: {vm}"
            ))),
        }
    }

    /// `{accel_type: count}` for all live wisent-* VMs across the
    /// resource group.
    pub(super) async fn running_counts(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let state = self.state().await?;
        let vms = state
            .client
            .list_vms(config::azure_resource_group())
            .await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        let prefix = format!("{}-", config::INSTANCE_PREFIX);
        for vm in &vms {
            let Some(name) = vm.get("name").and_then(Value::as_str) else {
                return Err(ProviderError::Value(format!(
                    "a VM ARM lists in the resource group names no name: {vm}"
                )));
            };
            if !name.starts_with(&prefix) {
                continue;
            }
            // Cheap state probe: list() doesn't include instance_view by
            // default, so trust the tag we stamped at create-time. A VM
            // in the resource group with the wisent_managed tag and a
            // known GPU SKU consumes quota until we delete it; counting
            // it as running is the safe direction.
            let Some(sku) = vm
                .get("properties")
                .and_then(|p| p.get("hardwareProfile"))
                .and_then(|h| h.get("vmSize"))
                .and_then(Value::as_str)
            else {
                return Err(ProviderError::Value(format!(
                    "VM {name} names no hardwareProfile.vmSize, so the capacity it holds is unknown"
                )));
            };
            let Some((accel, n)) = AZURE_VM_TO_ACCEL.get(sku) else {
                continue;
            };
            counts
                .entry((*accel).to_string())
                .and_modify(|count| *count += n)
                .or_insert(*n);
        }
        Ok(counts)
    }
}
