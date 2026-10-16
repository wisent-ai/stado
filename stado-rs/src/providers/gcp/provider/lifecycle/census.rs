//! Live GCE accelerator use. A malformed instance cannot be counted as free quota.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config;
use crate::providers::ProviderError;

use super::super::{GcpProvider, InstanceStatus};

pub(super) async fn running_accelerators(
    provider: &GcpProvider,
) -> Result<BTreeMap<String, i64>, ProviderError> {
    let state = provider.state().await?;
    let filter = format!("name:{}-*", config::INSTANCE_PREFIX);
    let instances = state.client.aggregated_instances(&filter).await?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for (zone, instance) in instances {
        let name = instance
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Value(format!(
                    "GCP instance in {zone} has no name; running accelerator use cannot be counted"
                ))
            })?;
        let status = instance
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Value(format!(
                    "GCP instance {name} in {zone} has no status; running accelerator use cannot be counted"
                ))
            })?;
        let phase = InstanceStatus::parse(status).ok_or_else(|| {
            ProviderError::Value(format!(
                "GCP instance {name} in {zone} has unknown status {status}; running accelerator use cannot be counted"
            ))
        })?;
        if !phase.booting_or_running() {
            continue;
        }
        if let Some(accelerators) = instance.get("guestAccelerators") {
            let accelerators = accelerators.as_array().ok_or_else(|| {
                ProviderError::Value(format!(
                    "GCP instance {name} in {zone} has malformed guestAccelerators; running accelerator use cannot be counted"
                ))
            })?;
            for accel in accelerators {
                let atype = accel
                    .get("acceleratorType")
                    .and_then(Value::as_str)
                    .and_then(|value| value.rsplit('/').next())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        ProviderError::Value(format!(
                            "GCP instance {name} in {zone} has an accelerator without a type; running accelerator use cannot be counted"
                        ))
                    })?;
                let count = accel
                    .get("acceleratorCount")
                    .and_then(Value::as_i64)
                    .filter(|count| count.is_positive())
                    .ok_or_else(|| {
                        ProviderError::Value(format!(
                            "GCP instance {name} in {zone} has an accelerator without a positive count; running accelerator use cannot be counted"
                        ))
                    })?;
                if let Some(running) = counts.get_mut(atype) {
                    *running = running.checked_add(count).ok_or_else(|| {
                        ProviderError::Value(format!(
                            "GCP accelerator {atype} use exceeds the representable count; quota cannot be calculated"
                        ))
                    })?;
                } else {
                    counts.insert(atype.to_owned(), count);
                }
            }
        }
    }
    Ok(counts)
}
