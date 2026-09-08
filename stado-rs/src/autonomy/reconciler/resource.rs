//! The resource fields both plan builders write: age, locator, execution
//! reference and ownership label.

use chrono::{DateTime, Utc};

use crate::cli::resources::model::ResourceLocator;

use crate::autonomy::model::{Ownership, ResourceRecord};

pub(super) fn resource_age_seconds(resource: &ResourceRecord, now: DateTime<Utc>) -> Option<u64> {
    let raw = resource.created_at.as_deref()?;
    let created = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
    now.signed_duration_since(created)
        .num_seconds()
        .try_into()
        .ok()
}

pub(super) fn locator(resource: &ResourceRecord) -> ResourceLocator {
    ResourceLocator {
        provider: resource.provider,
        resource_type: if resource.resource_type == "instance" {
            "agent-vm".to_string()
        } else {
            resource.resource_type.clone()
        },
        project: (resource.provider == crate::capabilities::ProviderId::Gcp)
            .then(|| resource.account.clone()),
        location: resource.zone.clone().or_else(|| resource.region.clone()),
        name: resource.name.clone(),
        reference: execution_reference(resource),
    }
}

fn execution_reference(resource: &ResourceRecord) -> String {
    match resource.provider {
        crate::capabilities::ProviderId::Gcp => resource
            .zone
            .as_deref()
            .map(tail)
            .map(|zone| format!("{}@{zone}", resource.name))
            .unwrap_or_else(|| resource.native_reference.clone()),
        crate::capabilities::ProviderId::Azure => resource
            .region
            .as_deref()
            .map(|region| format!("{}@{region}", resource.name))
            .unwrap_or_else(|| resource.native_reference.clone()),
        _ => resource.native_reference.clone(),
    }
}

fn tail(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

pub(super) fn ownership_name(ownership: Ownership) -> &'static str {
    match ownership {
        Ownership::Owned => "owned",
        Ownership::Adopted => "adopted",
        Ownership::Observed => "observed",
        Ownership::Unknown => "unknown",
    }
}
