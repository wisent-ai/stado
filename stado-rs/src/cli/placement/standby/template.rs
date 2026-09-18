//! What a profile needs on a host, read from the service catalog, and the
//! one registry commit that writes the host into the profile and the
//! directory once the host declares every unit.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::deploy::service::{self, SOURCE_REGISTRY};
use crate::placement::PlacementProfile;
use crate::targets::ComputeTarget;

/// A managed program lives here; the release path delivers it.
const MANAGED_PROGRAM_ROOT: &str = "$HOME/.stado/bin/";
/// A release-controlled tree lives here; the host's agent rolls it out.
const RELEASE_TREE_ROOT: &str = "$HOME/.stado/services/";
const RELEASE_TREE_CURRENT: &str = "/current/";

/// How one service's program reaches a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramKind {
    /// `$HOME/.stado/bin/<product>`: a managed program product.
    Managed { product: String },
    /// `$HOME/.stado/services/<product>/current/...`: a release-controlled
    /// tree.
    Tree { product: String },
}

#[derive(Debug, Clone)]
pub struct ServicePlan {
    pub logical: String,
    pub catalog_name: String,
    pub kind: ProgramKind,
    /// The placed host runs it in the system domain; the standby does too.
    pub as_daemon: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub services: Vec<ServicePlan>,
}

/// Every service of the profile, classified. Refused by name when the
/// catalog does not know a service or its program is somewhere neither
/// delivery path reaches.
pub(super) fn plan(
    profile: &PlacementProfile,
    placed: &ComputeTarget,
    target: &ComputeTarget,
) -> Result<Plan, CmdError> {
    let placed_units = service::declared_services(placed);
    let placed_template = profile.hosts.get(&placed.name).ok_or_else(|| {
        CmdError::click(format!(
            "placement profile {:?} declares no template for its placed host {}",
            profile.name, placed.name
        ))
    })?;
    let mut services = Vec::with_capacity(profile.services.len());
    for logical in &profile.services {
        let entry = crate::deploy::service_catalog::lookup(logical)
            .map_err(CmdError::click)?
            .ok_or_else(|| {
                CmdError::click(format!(
                    "the Wisent service catalog this build ships declares no service named \
                     {logical:?}; {} cannot be rendered on {} from the catalog",
                    profile.name, target.name
                ))
            })?;
        let kind = classify(&entry.program).ok_or_else(|| {
            CmdError::click(format!(
                "{logical} runs {} on a host, which is neither a managed program under \
                 {MANAGED_PROGRAM_ROOT} nor a release-controlled tree under \
                 {RELEASE_TREE_ROOT}<product>{RELEASE_TREE_CURRENT}; nothing delivers it to {}",
                entry.program, target.name
            ))
        })?;
        let as_daemon = placed_template
            .units
            .get(logical)
            .and_then(|unit| unit.managed())
            .map(|unit| service::UnitDomain::from_path(&unit.path))
            .or_else(|| {
                placed_units
                    .iter()
                    .find(|declared| declared.matches(logical))
                    .map(|declared| service::UnitDomain::from_path(&declared.path))
            })
            .is_some_and(|domain| !domain.is_per_login());
        services.push(ServicePlan {
            logical: logical.clone(),
            catalog_name: entry.name.clone(),
            kind,
            as_daemon,
        });
    }
    Ok(Plan { services })
}

fn classify(program: &str) -> Option<ProgramKind> {
    if let Some(rest) = program.strip_prefix(MANAGED_PROGRAM_ROOT) {
        let product = rest.split('/').next()?;
        return (!product.is_empty()).then(|| ProgramKind::Managed {
            product: product.to_string(),
        });
    }
    let rest = program.strip_prefix(RELEASE_TREE_ROOT)?;
    let (product, tail) = rest.split_once('/')?;
    (!product.is_empty() && tail.starts_with(RELEASE_TREE_CURRENT.trim_start_matches('/'))).then(
        || ProgramKind::Tree {
            product: product.to_string(),
        },
    )
}

/// The unit the registry declares on `target` for one logical service, when
/// it declares one from the registry's own source.
fn declared_unit<'a>(
    declared: &'a [service::ManagedService],
    logical: &str,
) -> Option<&'a service::ManagedService> {
    let unit_label = crate::deploy::service_catalog::lookup(logical)
        .ok()
        .flatten()
        .and_then(|entry| entry.unit);
    declared.iter().find(|managed| {
        managed.source == SOURCE_REGISTRY
            && (managed.matches(logical)
                || unit_label
                    .as_deref()
                    .is_some_and(|label| managed.matches(label)))
    })
}

/// Whether the host already sits in the profile with every unit declared.
pub(super) fn already_declared(
    profile: &PlacementProfile,
    target: &ComputeTarget,
    plan: &Plan,
) -> bool {
    let declared = service::declared_services(target);
    profile.hosts.contains_key(&target.name)
        && plan
            .services
            .iter()
            .all(|service| declared_unit(&declared, &service.logical).is_some())
}

/// Write `target` into the profile with the units the registry now declares
/// on it and the placed host's probes, and into the directory as a standby
/// endpoint for each of the profile's services. One compare-and-swapped
/// commit; a document that already says so is left alone.
pub(super) async fn commit_host(
    profile: &PlacementProfile,
    placed_on: &str,
    target: &ComputeTarget,
    plan: &Plan,
) -> Result<String, CmdError> {
    let profile_name = profile.name.clone();
    let target_name = target.name.clone();
    let placed_on = placed_on.to_string();
    let logicals: Vec<String> = plan.services.iter().map(|s| s.logical.clone()).collect();
    crate::cli::registry::commit_document(move |current| {
        let registry = super::super::candidates::parse_registry(current)?;
        let live_target = super::super::candidates::target(&registry, &target_name)?;
        let declared = service::declared_services(live_target);
        let mut units = Map::new();
        for logical in &logicals {
            let managed = declared_unit(&declared, logical).ok_or_else(|| {
                CmdError::click(format!(
                    "{target_name} no longer declares a unit for {logical}; the profile is not \
                     written half"
                ))
            })?;
            units.insert(
                logical.clone(),
                json!({
                    "kind": managed.kind,
                    "name": managed.unit_id(),
                    "path": managed.path,
                    "unit": managed.unit_id(),
                }),
            );
        }
        let mut next = current.clone();
        let profiles = next
            .get_mut("placement_profiles")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| CmdError::click("registry.placement_profiles is not an array"))?;
        let entry = profiles
            .iter_mut()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(&profile_name))
            .ok_or_else(|| {
                CmdError::click(format!("placement profile {profile_name:?} disappeared"))
            })?;
        let hosts = entry
            .get_mut("hosts")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CmdError::click("placement profile hosts is not an object"))?;
        let probes = hosts
            .get(&placed_on)
            .and_then(|template| template.get("probes"))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        hosts.insert(
            target_name.clone(),
            json!({ "probes": probes, "units": Value::Object(units) }),
        );
        if let Some(services) = next
            .get_mut("service_directory")
            .and_then(|directory| directory.get_mut("services"))
            .and_then(Value::as_object_mut)
        {
            for logical in &logicals {
                let Some(route) = services.get_mut(logical).and_then(Value::as_object_mut) else {
                    continue;
                };
                let Some(serving) = route
                    .get("endpoints")
                    .and_then(|endpoints| endpoints.get(&placed_on))
                    .cloned()
                else {
                    continue;
                };
                route
                    .entry("standby")
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or_else(|| {
                        CmdError::click(format!(
                            "directory service {logical} standby is not an object"
                        ))
                    })?
                    .insert(target_name.clone(), serving);
            }
        }
        Ok(next)
    })
    .await
}
