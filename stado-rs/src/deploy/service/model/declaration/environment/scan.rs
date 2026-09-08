use super::report::unit_names_product;
use crate::deploy::service::*;

/// Every product whose declared environment cannot reach the unit serving it
/// on TARGET.
///
/// Two conditions, one instrument, because they are one root cause read at
/// two ranges: the product's `environment` is written by exactly one place
/// (`release_agent::spawn_release`) and reached through exactly one lookup
/// (`policy.targets.get(host)`), so a host that lookup misses gets no
/// environment, and a unit the registry adopted as a bare path records none
/// either. Both are reported against the same product policy and in the same
/// words.
///
/// Both require two independent witnesses that this host really runs the
/// product — the host's own `managed_versions` entry, which is what every
/// version diagnostic here enumerates, and an adopted unit whose identifier
/// names the product. Requiring both is deliberate: a product declares an
/// environment on every host in this fleet, so keying off the policy alone
/// would fire on every host that has one, and a check that fires everywhere
/// is a check that gets switched off with the defect still in place.
///
/// `local_units` decides which unit files may be opened: it is the name of
/// the host this process is running on, when the registry resolves one.
/// Every other host's unit is reported unread rather than empty.
pub fn unreachable_product_environments(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
    local_units: Option<&str>,
) -> Vec<UnreachableProductEnvironment> {
    let Some(control) = control else {
        return Vec::new();
    };
    let readable_here = local_units == Some(target.name.as_str());
    let services = declared_services(target);
    let mut rows: Vec<UnreachableProductEnvironment> = Vec::new();
    for (product, policy) in &control.products {
        if policy.environment.is_empty() {
            continue;
        }
        // The host's own statement that it runs this product. `stado host
        // declare-version` writes it and `host reconcile` reads it, so it is
        // the fleet's existing answer to "does this box run that product".
        if target.declared_version(product).is_none() {
            continue;
        }
        let Some(service) = services
            .iter()
            .find(|service| unit_names_product(service.unit_id(), product))
        else {
            continue;
        };
        // Use the same home expansion as the delivery path that owns the values.
        let home = policy
            .targets
            .get(&target.name)
            .map(|policy_target| policy_target.home.clone())
            .unwrap_or_else(|| crate::deploy::service_catalog::home_for(target));
        let declared: Vec<(String, String)> = policy
            .environment
            .iter()
            .map(|(name, value)| {
                let value = if home.is_empty() {
                    value.clone()
                } else {
                    value.replace("{home}", &home)
                };
                (name.clone(), value)
            })
            .collect();
        let row = |gap: EnvironmentGap| UnreachableProductEnvironment {
            host: target.name.clone(),
            product: product.clone(),
            declared: declared.clone(),
            unit: service.unit_id().to_string(),
            path: service.path.clone(),
            gap,
        };
        // What this pass may say about the unit's own bytes: read them when
        // the unit is on the machine running this command, and otherwise say
        // which of the two silences it is.
        let reading = || {
            if !readable_here {
                return UnitReading::OtherHost;
            }
            local_unit_file(&service.path, &service.kind)
                .map_or(UnitReading::Unreadable, UnitReading::Read)
        };
        if !policy.targets.contains_key(&target.name) {
            let pinned_in_service = declared
                .iter()
                .all(|(name, value)| service.env.get(name) == Some(value));
            if pinned_in_service {
                let observed = reading();
                let agrees = observed.file().is_some_and(|unit| {
                    declared
                        .iter()
                        .all(|(name, value)| unit.env.get(name) == Some(value))
                });
                if !agrees {
                    rows.push(row(EnvironmentGap::PinnedServiceEnvironment { observed }));
                }
            } else {
                let mut named_hosts: Vec<String> = policy.targets.keys().cloned().collect();
                named_hosts.sort();
                rows.push(row(EnvironmentGap::HostNamedByNoTarget { named_hosts }));
            }
        }
        // An adopted stub: the record names a path and declares nothing about
        // what runs there. Reported independently of the target question,
        // because a host the policy DOES name still has no recorded
        // declaration to diff, and that is the second silence.
        if service.program.is_empty() {
            rows.push(row(EnvironmentGap::UnrecordedDeclaration {
                adopted_at: service.managed_since.clone(),
                observed: reading(),
            }));
        }
    }
    rows
}
