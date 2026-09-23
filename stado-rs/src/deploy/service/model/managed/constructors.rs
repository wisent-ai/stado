use crate::deploy::service::*;

/// A launchd-managed service, the shape both the recovery agents and an
/// adopted macOS unit take.
pub fn launchd_service(
    host: &str,
    label: &str,
    path: &str,
    source: &str,
    since: &str,
) -> ManagedService {
    ManagedService {
        host: host.to_string(),
        host_heuristic: None,
        name: label.to_string(),
        unit: String::new(),
        label: label.to_string(),
        path: path.to_string(),
        kind: KIND_LAUNCHD.to_string(),
        source: source.to_string(),
        managed_since: since.to_string(),
        program: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
        systemd_unit: String::new(),
        onboarding: None,
    }
}

/// A systemd-managed service, in either the system or per-user scope.
pub fn systemd_service(
    host: &str,
    unit: &str,
    path: &str,
    source: &str,
    since: &str,
) -> ManagedService {
    ManagedService {
        host: host.to_string(),
        host_heuristic: None,
        name: unit.to_string(),
        unit: unit.to_string(),
        label: String::new(),
        path: path.to_string(),
        kind: KIND_SYSTEMD.to_string(),
        source: source.to_string(),
        managed_since: since.to_string(),
        program: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
        systemd_unit: String::new(),
        onboarding: None,
    }
}

/// The installed host unit owns Stado components that previously had separate units.
/// Keep explicitly declared legacy units visible until migration removes their records.
pub fn resident_host_service<'a>(services: &'a [ManagedService]) -> Option<&'a ManagedService> {
    services.iter().find(|service| {
        crate::deploy::service_catalog::executable_name(&service.program) == Some("stado")
            && service.args.first().map(String::as_str) == Some("serve")
    })
}

/// Every unit Stado manages on one target: the registry-declared array
/// first, then macOS recovery agents on hosts declared to run macOS. A
/// declaration wins over the fixed list, because an operator who adopted a
/// recovery label explicitly said what its path and name are.
pub fn declared_services(target: &ComputeTarget) -> Vec<ManagedService> {
    let mut services: Vec<ManagedService> = target
        .extra
        .get(SERVICES_KEY)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(Value::as_object)
                // `service declare` records desired state before a unit
                // exists. Operational commands must not address that
                // placeholder as a loaded or managed unit; `deploy` replaces
                // it through `record_declaration` after the host action.
                .filter(|record| record.get("declared_only").and_then(Value::as_bool) != Some(true))
                .map(|record| ManagedService::from_record(&target.name, record))
                .collect()
        })
        .unwrap_or_default();
    if resident_host_service(&services).is_some()
        || !crate::targets::platform_accepts_job(&target.release_platform, "Darwin", "")
    {
        return services;
    }
    for (label, plist) in host_recovery::MANAGED_AGENTS {
        if services.iter().any(|service| service.matches(label)) {
            continue;
        }
        services.push(launchd_service(
            &target.name,
            label,
            plist,
            SOURCE_RECOVERY,
            "",
        ));
    }
    services
}
