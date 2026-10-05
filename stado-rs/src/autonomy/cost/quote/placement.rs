use super::super::PriceQuote;
use super::schema::{AzurePriority, GcpProvisioningModel, HourUnit, Purchase};
use crate::autonomy::model::{InventorySnapshot, ResourceRecord};
use crate::capabilities::ProviderId;
use crate::models::{Job, WorkerResource};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

pub(super) fn timestamp(value: &str, label: &str) -> Result<DateTime<FixedOffset>, String> {
    DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("invalid {label} {value:?}: {error}"))
}

pub(super) fn resource<'a>(
    job: &Job,
    snapshot: &'a InventorySnapshot,
) -> Result<&'a ResourceRecord, String> {
    let reference = job
        .instance_ref
        .as_deref()
        .ok_or("job has no recorded allocation")?;
    let identity = job
        .worker_allocation
        .as_ref()
        .and_then(|worker| worker.resource.as_ref())
        .ok_or("job has no observed worker resource identity")?;
    let mut matches = snapshot.resources.iter().filter(|resource| {
        resource.resource_type == "instance"
            && match identity {
                WorkerResource::Aws {
                    account_id,
                    region,
                    instance_id,
                } => {
                    resource.provider == ProviderId::Aws
                        && resource.account == *account_id
                        && resource.region.as_deref() == Some(region.as_str())
                        && resource.native_reference == *instance_id
                }
                WorkerResource::Azure {
                    subscription_id,
                    location,
                    resource_id,
                    ..
                } => {
                    resource.provider == ProviderId::Azure
                        && resource.account.eq_ignore_ascii_case(subscription_id)
                        && resource
                            .region
                            .as_deref()
                            .is_some_and(|region| region.eq_ignore_ascii_case(location))
                        && resource.native_reference.eq_ignore_ascii_case(resource_id)
                }
                WorkerResource::Gcp {
                    project_id,
                    zone,
                    name,
                    ..
                } => {
                    resource.provider == ProviderId::Gcp
                        && resource.account == *project_id
                        && resource.name == *name
                        && resource.zone.as_deref() == Some(zone.as_str())
                }
                WorkerResource::Local => false,
            }
    });
    let resource = matches.next().ok_or_else(|| {
        format!(
            "allocation {reference:?} has no matching compute instance in inventory {}",
            snapshot.snapshot_id
        )
    })?;
    if matches.next().is_some() {
        return Err(format!("allocation {reference:?} matches multiple inventory resources; no account or machine was guessed"));
    }
    match identity {
        WorkerResource::Gcp { instance_id, .. } => {
            let observed = text(resource, "/item/instance_id")?
                .parse::<u64>()
                .map_err(|error| {
                    format!("inventory {} instance ID: {error}", resource.resource_id)
                })?;
            if observed != *instance_id {
                return Err(
                    "inventory names a different Compute Engine instance generation".into(),
                );
            }
        }
        WorkerResource::Azure { vm_id, .. }
            if !text(resource, "/properties/vmId")?.eq_ignore_ascii_case(vm_id) =>
        {
            return Err("inventory names a different Azure VM generation".into());
        }
        _ => {}
    }
    let started = timestamp(
        job.started_at
            .as_deref()
            .ok_or("allocated job omitted started_at")?,
        "job start",
    )?;
    if timestamp(&resource.last_seen_at, "inventory observation")? < started {
        return Err(format!(
            "inventory observation {} predates job start {}; refresh inventory through Stado",
            resource.last_seen_at, started
        ));
    }
    if let Some(created) = &resource.created_at {
        if timestamp(created, "resource creation")? > started {
            return Err(format!("resource {} was created after this job started; the allocation name may have been reused", resource.resource_id));
        }
    }
    Ok(resource)
}

fn text<'a>(resource: &'a ResourceRecord, pointer: &str) -> Result<&'a str, String> {
    resource
        .evidence
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "inventory {} omitted {pointer}; requested job resources are not measurements",
                resource.resource_id
            )
        })
}

pub(super) fn attributes(resource: &ResourceRecord) -> Result<(&str, Purchase), String> {
    match resource.provider {
        ProviderId::Aws => {
            if text(resource, "/platform_details")? != "Linux/UNIX" {
                return Err(
                    "AWS whole-machine price book covers Linux/UNIX; the observed platform differs"
                        .into(),
                );
            }
            let lifecycle = resource
                .evidence
                .get("instance_lifecycle")
                .ok_or("AWS inventory omitted its lifecycle observation")?;
            let purchase = if lifecycle.is_null() {
                Purchase::OnDemand
            } else {
                let lifecycle = aws_sdk_ec2::types::InstanceLifecycleType::from(text(
                    resource,
                    "/instance_lifecycle",
                )?);
                match lifecycle {
                    aws_sdk_ec2::types::InstanceLifecycleType::Spot => Purchase::Spot,
                    value => return Err(format!("unsupported observed EC2 lifecycle {value:?}")),
                }
            };
            Ok((text(resource, "/instance_type")?, purchase))
        }
        ProviderId::Azure => {
            if text(resource, "/properties/storageProfile/osDisk/osType")? != "Linux" {
                return Err(
                    "Azure whole-machine price book covers Linux; the observed OS differs".into(),
                );
            }
            // Azure declares Regular as the protocol default for an omitted priority.
            let priority = match resource.evidence.pointer("/properties/priority") {
                Some(value) => AzurePriority::deserialize(value)
                    .map_err(|error| format!("Azure properties.priority={value}: {error}"))?,
                None => AzurePriority::Regular,
            };
            let purchase = match priority {
                AzurePriority::Regular => Purchase::OnDemand,
                AzurePriority::Spot => Purchase::Spot,
                AzurePriority::Low => {
                    return Err("legacy Azure Low priority is not a Spot allocation quote".into())
                }
            };
            Ok((
                text(resource, "/properties/hardwareProfile/vmSize")?,
                purchase,
            ))
        }
        ProviderId::Gcp => {
            let value = resource
                .evidence
                .pointer("/item/provisioning_model")
                .ok_or("GCP inventory omitted provisioning_model")?;
            let model = GcpProvisioningModel::deserialize(value)
                .map_err(|error| format!("GCP provisioning_model={value}: {error}"))?;
            let preemptible = resource
                .evidence
                .pointer("/item/preemptible")
                .and_then(serde_json::Value::as_bool);
            let purchase = match (model, preemptible) {
                (GcpProvisioningModel::Spot, _) | (GcpProvisioningModel::Standard, Some(true)) => {
                    Purchase::Spot
                }
                (GcpProvisioningModel::Standard, Some(false)) => Purchase::OnDemand,
                value => {
                    return Err(format!(
                        "GCP inventory omitted its observed preemptible flag: {value:?}"
                    ))
                }
            };
            Ok((text(resource, "/item/machine_type")?, purchase))
        }
        provider => Err(format!(
            "no whole-machine quote adapter for observed provider {provider:?}"
        )),
    }
}

pub(super) fn matches(
    quote: &PriceQuote,
    resource: &ResourceRecord,
    machine: &str,
    purchase: Purchase,
) -> bool {
    if quote.provider != resource.provider
        || quote.machine_type.as_deref() != Some(machine)
        || Purchase::decode(&quote.purchase_option).ok() != Some(purchase)
        || !HourUnit::accepts(&quote.unit)
        || quote.currency != "USD"
        || !quote.hourly_usd.is_finite()
        || quote.hourly_usd < 0.0
        || quote.source.is_empty()
    {
        return false;
    }
    if resource.provider == ProviderId::Aws {
        // Old books aggregated history or selected the cheapest dimension.
        // Only the revised provider observations establish a distinct quote.
        match purchase {
            Purchase::Spot => {
                return quote.source == super::super::prices::SPOT_SOURCE
                    && quote.region.is_some()
                    && quote.region == resource.zone
            }
            Purchase::OnDemand if quote.source != super::super::prices::ON_DEMAND_SOURCE => {
                return false
            }
            Purchase::OnDemand => {}
        }
    }
    match quote.region.as_deref() {
        None | Some("global") => true,
        Some(region) => {
            resource.region.as_deref() == Some(region) || resource.zone.as_deref() == Some(region)
        }
    }
}
