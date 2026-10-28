//! The shape of one `targets[].services[]` record: the members the managed
//! service model reads, each in the shape it reads them. A record that
//! passes here reads back without a blank standing in for a value the
//! operator never wrote; one that does not is refused at the write or by
//! `registry doctor`, naming the record and what was found.

use std::collections::BTreeMap;

use crate::targets::*;

/// The members `ManagedService::from_record` reads, typed as it reads them.
/// Members it does not read (`host`, `unit_id`, `source`, `declared_only`,
/// `onboarding`, and whatever a later release adds) pass through unjudged.
#[derive(serde::Deserialize)]
#[allow(
    dead_code,
    reason = "deserializing a member is the check that it has the type the model reads"
)]
struct ServiceRecord {
    name: Option<String>,
    unit: Option<String>,
    label: Option<String>,
    path: Option<String>,
    kind: Option<String>,
    program: Option<String>,
    managed_since: Option<String>,
    systemd_unit: Option<String>,
    host_heuristic: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

pub(super) fn validate_service_record(
    location: &str,
    service: &Map<String, Value>,
) -> Result<(), RegistryValidationError> {
    let record: ServiceRecord =
        serde_json::from_value(Value::Object(service.clone())).map_err(|error| {
            verr(
                location,
                &format!("is not a service record the managed service model reads: {error}"),
            )
        })?;
    // The record must address a unit: what the operator calls it, the
    // launchd label, or the systemd unit. A record naming none of them is
    // a service nothing can be asked about.
    let written = |member: &Option<String>| {
        member
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    };
    if !(written(&record.name) || written(&record.unit) || written(&record.label)) {
        return Err(verr(
            location,
            "must name the service: none of name, unit or label is nonempty text",
        ));
    }
    Ok(())
}
