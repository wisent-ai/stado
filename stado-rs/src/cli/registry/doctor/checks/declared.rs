//! The services a registry target declares it manages, and the catalog
//! product whose one process replaced one of them.

use serde_json::Value;

use crate::deploy::{service, service_catalog};
use crate::targets::ComputeTarget;

/// One service a registry target declares it manages.
pub(super) struct DeclaredUnit {
    /// Operator-facing service name.
    pub(super) name: String,
    /// The identifier the beacon reports it under: the launchd label on
    /// macOS, the systemd unit on Linux.
    pub(super) id: String,
}

/// Services declared on a target by `stado service adopt|deploy`
/// (`deploy/service.rs`), which writes a per-target `services` array. The
/// key is unknown to [`ComputeTarget`], so it round-trips through
/// [`ComputeTarget::extra`]; a target that declares none is checked for
/// liveness only, never for units.
pub(super) fn declared_units(
    target: &ComputeTarget,
    release_control: Option<&crate::release_control::ReleaseControl>,
) -> Vec<DeclaredUnit> {
    let legacy_labels = service::legacy_launchd_labels(target, release_control);
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
            };
            let id = text("label")
                .or_else(|| text("unit"))
                .or_else(|| text("name"))?;
            if legacy_labels.contains(id) {
                return None;
            }
            Some(DeclaredUnit {
                name: text("name").unwrap_or(id).to_string(),
                id: id.to_string(),
            })
        })
        .collect()
}

/// The catalog product whose one process does the work of `declared`
/// although it is not that product's unit: the product whose program the
/// declaration runs, as the service reconciler judges it before it declines
/// to repair it. The host Stado process's units are judged role by role on
/// the host, so they are not reported here.
pub(super) fn retired_replacement(
    target: &ComputeTarget,
    declared: &DeclaredUnit,
) -> Option<service_catalog::CatalogService> {
    let services = service::declared_services(target);
    let service = services
        .iter()
        .find(|service| service.unit_id() == declared.id)?;
    let owner = service::declared_owner(target, service).ok()??;
    let host = service_catalog::host_process().ok()?;
    (owner.name != host.name).then_some(owner)
}
