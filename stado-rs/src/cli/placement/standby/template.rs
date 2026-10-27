//! What a profile needs on a host, read from the service catalog, and the
//! one registry commit that writes the host into the profile and the
//! directory once every unit is staged on it.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::deploy::service;
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
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let mut services = Vec::with_capacity(profile.services.len());
    for logical in &profile.services {
        let entry = catalog_entry(logical, placed_template)
            .map_err(|error| {
                CmdError::click(error).stating(crate::primitives::failure::FailureCode::Config)
            })?
            .ok_or_else(|| {
                CmdError::click(format!(
                    "the Wisent service catalog this build ships declares no service named \
                     {logical:?}, nor the unit {} declares for it on {}; {} cannot be rendered on \
                     {} from the catalog",
                    profile.name, placed.name, profile.name, target.name
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let kind = classify(&entry.program).ok_or_else(|| {
            CmdError::click(format!(
                "{logical} runs {} on a host, which is neither a managed program under \
                 {MANAGED_PROGRAM_ROOT} nor a release-controlled tree under \
                 {RELEASE_TREE_ROOT}<product>{RELEASE_TREE_CURRENT}; nothing delivers it to {}",
                entry.program, target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
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

/// The catalog entry a profile service runs: the one named like the
/// directory service, else the one the placed host's unit for it names (by
/// its name, then its launchd label or systemd unit). A directory service is
/// often named for what it offers (`stado-object-api`) while the catalog
/// names the product whose unit serves it (`stado`, `com.wisent.stado`).
fn catalog_entry(
    logical: &str,
    placed: &crate::placement::PlacementHost,
) -> Result<Option<crate::deploy::service_catalog::CatalogService>, String> {
    use crate::deploy::service_catalog::lookup;
    if let Some(entry) = lookup(logical)? {
        return Ok(Some(entry));
    }
    let Some(unit) = placed.units.get(logical) else {
        return Ok(None);
    };
    if let Some(entry) = lookup(&unit.name)? {
        return Ok(Some(entry));
    }
    match unit.managed() {
        Some(managed) => lookup(&managed.unit),
        None => Ok(None),
    }
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

/// Write `target` into the profile with the units it now holds staged and
/// the placed host's probes, and into the directory as a standby address for
/// each service the directory does not already name it under. One
/// compare-and-swapped commit; a document that already says so is left
/// alone.
pub(super) async fn commit_host(
    profile: &PlacementProfile,
    placed_on: &str,
    target: &ComputeTarget,
    staged: &[super::UnitReceipt],
) -> Result<String, CmdError> {
    let profile_name = profile.name.clone();
    let target_name = target.name.clone();
    let placed_on = placed_on.to_string();
    let kind = if target.release_platform.starts_with("darwin") {
        service::KIND_LAUNCHD
    } else {
        service::KIND_SYSTEMD
    };
    let mut units = Map::new();
    for unit in staged {
        units.insert(
            unit.service.clone(),
            json!({ "kind": kind, "name": unit.label, "path": unit.path, "unit": unit.label }),
        );
    }
    let logicals: Vec<String> = staged.iter().map(|unit| unit.service.clone()).collect();
    crate::cli::registry::commit_document(move |current| {
        let mut next = current.clone();
        let profiles = next
            .get_mut("placement_profiles")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                CmdError::click("registry.placement_profiles is not an array")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let entry = profiles
            .iter_mut()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(&profile_name))
            .ok_or_else(|| {
                CmdError::click(format!("placement profile {profile_name:?} disappeared"))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        let hosts = entry
            .get_mut("hosts")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click("placement profile hosts is not an object")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let placed_probes = hosts
            .get(&placed_on)
            .and_then(|template| template.get("probes"))
            .ok_or_else(|| {
                CmdError::click(format!(
                    "placement profile {profile_name:?} declares no probes for its placed host \
                     {placed_on}"
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let probes = super::ports::probes_for(current, placed_probes, &target_name, &placed_on)?;
        hosts.insert(
            target_name.clone(),
            json!({ "probes": probes, "units": Value::Object(units.clone()) }),
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
                // The directory names each placement host exactly once, as an
                // endpoint or as a standby address.
                let named = ["endpoints", "standby"].iter().any(|map| {
                    route
                        .get(*map)
                        .and_then(|hosts| hosts.get(&target_name))
                        .is_some()
                });
                let Some(serving) = route
                    .get("endpoints")
                    .and_then(|endpoints| endpoints.get(&placed_on))
                    .cloned()
                    .filter(|_value| !named)
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
                        .stating(crate::primitives::failure::FailureCode::Config)
                    })?
                    .insert(target_name.clone(), serving);
            }
        }
        Ok(next)
    })
    .await
}
