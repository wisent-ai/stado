//! What a standby pass knows before it changes anything, and the check that
//! stops there.

use serde_json::{json, Value};

use super::template::{self, Plan, ProgramKind};
use crate::cli::{registry, CmdError};
use crate::placement::{self, PlacementProfile};
use crate::targets::ComputeTarget;

/// The registry a pass read, the profile, the host that stands by, the host
/// the profile is placed on, and the plan the catalog gives for it.
pub(super) struct Planned {
    pub document: Value,
    pub profile: PlacementProfile,
    pub target: ComputeTarget,
    pub placed: ComputeTarget,
    pub placed_on: String,
    pub plan: Plan,
}

/// Every refusal a pass can give before it delivers anything: no reason, an
/// unknown profile or one a move holds, the placed host standing by for
/// itself, a service the catalog does not know, a program neither delivery
/// path reaches.
pub(super) async fn planned(
    profile_name: &str,
    host: &str,
    reason: &str,
) -> Result<Planned, CmdError> {
    if reason.trim().is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host must stand by for the profile; the receipt carries \
             it",
        ));
    }
    let (document, _generation) = registry::fetch_versioned_document().await?;
    let parsed = super::super::candidates::parse_registry(&document)?;
    let profile = placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the registry declares no placement profile named {profile_name:?}"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    super::super::candidates::ensure_profile_lifecycle_mutable(&profile)?;
    let target = crate::cli::canonical_host(host).await?;
    let placed_on = super::super::placed_host(&parsed, &profile).map_err(CmdError::declaration)?;
    if placed_on == target.name {
        return Err(CmdError::refused(format!(
            "{} is placed on {placed_on}; a placed host cannot stand by for its own profile",
            profile.name
        )));
    }
    let placed = super::super::candidates::target(&parsed, &placed_on)?.clone();
    let plan = template::plan(&profile, &placed, &target)?;
    Ok(Planned { document, profile, target, placed, placed_on, plan })
}

/// `stado placement standby … --check`: every refusal a pass gives before it
/// delivers anything, and what it would deliver, with nothing delivered,
/// staged or written.
pub(crate) async fn check(profile_name: &str, host: &str, reason: &str) -> Result<Value, CmdError> {
    let planned = planned(profile_name, host, reason).await?;
    let services: Vec<Value> = planned
        .plan
        .services
        .iter()
        .map(|service| {
            let (delivery, product, tree) = match &service.kind {
                ProgramKind::Managed { product } => ("managed_program", product, false),
                ProgramKind::Tree { product } => ("release_tree", product, true),
            };
            // A tree is rolled out by the release-control product of its
            // name, or the one whose `service` names it; its install root
            // says whether that rollout writes where the catalog program runs.
            let control = planned
                .document
                .get("release_control")
                .and_then(|control| control.get("products"))
                .and_then(Value::as_object)
                .and_then(|products| {
                    products.get_key_value(product.as_str()).or_else(|| {
                        products.iter().find(|(_, policy)| {
                            policy.get("service").and_then(Value::as_str) == Some(product.as_str())
                        })
                    })
                })
                .filter(|_| tree)
                .map(|(name, policy)| json!({ "product": name, "install_root": policy.get("install_root") }));
            json!({
                "service": service.logical,
                "catalog": service.catalog_name,
                "delivery": delivery,
                "product": product,
                "release_control": control,
                "as_daemon": service.as_daemon,
            })
        })
        .collect();
    Ok(json!({
        "status": "would_stand_by",
        "profile": planned.profile.name,
        "host": planned.target.name,
        "placed_on": planned.placed_on,
        "platform": planned.target.release_platform,
        "services": services,
    }))
}
