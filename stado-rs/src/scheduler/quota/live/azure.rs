//! The Azure half of the live read: the machine-catalog lookup that turns
//! one quota family into (accel, vCPUs per schedulable slot), and the
//! per-location usages fan-out that divides the reported vCPU limits by it.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::catalog::{AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE, AZURE_VM_TO_ACCEL};
use crate::providers::azure::ArmClient;
use crate::scheduler::quota::scalar::py_int;
use crate::scheduler::quota::QuotaError;

fn azure_family_slot(family: &str) -> Option<(&'static str, i64)> {
    let machine_type = AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE.get(family)?;
    let (accel, gpu_count) = AZURE_VM_TO_ACCEL.get(machine_type)?;
    if !gpu_count.is_positive() {
        return None;
    }
    let digits: String = machine_type
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    // The vCPU count is the machine type's first run of digits; a type
    // without one, or wider than i64, declares no slot.
    let vcpus = match digits.parse::<i64>() {
        Ok(vcpus) => vcpus,
        Err(_no_digits_or_too_wide) => return None,
    };
    let vcpus_per_slot = vcpus / *gpu_count;
    vcpus_per_slot
        .is_positive()
        .then_some((*accel, vcpus_per_slot))
}

/// Live Azure regional quota limits converted from vCPU-family limits to
/// schedulable GPU slots.
pub async fn fetch_quotas_azure(
    client: &ArmClient,
    locations: &[String],
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let mut out = BTreeMap::new();
    for location in locations {
        for usage in client.list_usages(location).await? {
            // A usage row without a family name maps to no accelerator and
            // is passed over, as every family this table does not name is.
            let Some(family) = usage.pointer("/name/value").and_then(Value::as_str) else {
                continue;
            };
            let Some((accel, vcpus_per_slot)) = azure_family_slot(family) else {
                continue;
            };
            let slots = py_int("limit", usage.get("limit"))?
                .into_iter()
                .sum::<i64>()
                / vcpus_per_slot;
            *out.entry(accel.to_string()).or_default() += slots;
        }
    }
    Ok(out)
}
