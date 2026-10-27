//! The resource fields both plan builders write: age, locator, execution
//! reference and ownership label.

use chrono::{DateTime, Utc};

use crate::cli::resources::model::ResourceLocator;

use crate::autonomy::model::{ResourceKind, ResourceRecord};

/// How long ago the provider says the resource was created; `Err` names the
/// record without a creation time, one whose time is not RFC 3339, and one
/// created after `now`, so a resource the plan cannot judge is said rather
/// than skipped.
pub(super) fn resource_age_seconds(
    resource: &ResourceRecord,
    now: DateTime<Utc>,
) -> Result<u64, String> {
    let raw = match resource.created_at.as_deref() {
        Some(raw) => raw,
        None => {
            return Err(format!(
                "{} records no created_at, so its age cannot be judged",
                resource.resource_id
            ))
        }
    };
    let created = DateTime::parse_from_rfc3339(raw)
        .map_err(|error| {
            format!(
                "{} records created_at {raw:?}, which is not RFC 3339: {error}",
                resource.resource_id
            )
        })?
        .with_timezone(&Utc);
    u64::try_from(now.signed_duration_since(created).num_seconds()).map_err(|_negative| {
        format!(
            "{} records created_at {raw}, which is after the plan's own time {now}",
            resource.resource_id
        )
    })
}

pub(super) fn locator(resource: &ResourceRecord) -> ResourceLocator {
    ResourceLocator {
        provider: resource.provider,
        resource_type: if resource.resource_type == ResourceKind::Instance {
            "agent-vm".to_string()
        } else {
            resource.resource_type.to_string()
        },
        project: (resource.provider == crate::capabilities::ProviderId::Gcp)
            .then(|| resource.account.clone()),
        location: resource.zone.clone().or_else(|| resource.region.clone()),
        name: resource.name.clone(),
        reference: execution_reference(resource),
    }
}

fn execution_reference(resource: &ResourceRecord) -> String {
    // A GCP resource is addressed by name and zone, an Azure one by name and
    // region; one recorded without that location keeps its native reference.
    match (
        resource.provider,
        resource.zone.as_deref(),
        resource.region.as_deref(),
    ) {
        (crate::capabilities::ProviderId::Gcp, Some(zone), _region) => {
            format!("{}@{}", resource.name, tail(zone))
        }
        (crate::capabilities::ProviderId::Azure, _zone, Some(region)) => {
            format!("{}@{region}", resource.name)
        }
        (_other_provider, _zone, _region) => resource.native_reference.clone(),
    }
}

/// The last `/` segment of a provider path (`zones/us-central1-a` →
/// `us-central1-a`); `rsplit` always yields one.
fn tail(value: &str) -> &str {
    match value.rsplit('/').next() {
        Some(last) => last,
        None => unreachable!("str::rsplit yields at least one segment"),
    }
}
