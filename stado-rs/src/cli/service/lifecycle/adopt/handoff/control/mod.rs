//! `service handoff-release-control`: the half that establishes every
//! runtime fact, before the lease is taken.

use super::*;

mod leased;

use leased::handoff_under_lease;

/// What the pre-lease half established, handed to the leased half under the
/// names that half already read them by.
struct HandoffContext<'a> {
    document: Value,
    expected_generation: String,
    target: targets::ComputeTarget,
    service_name: &'a str,
    host: &'a str,
    product: &'a str,
    json_output: bool,
    profile_name: &'a str,
    target_policy: &'a crate::release_control::ReleaseTargetPolicy,
    desired: &'a crate::release_control::DesiredRelease,
    desired_artifact: &'a crate::release_control::ReleaseArtifactRef,
    receipt_path: std::path::PathBuf,
    prior_receipt: Option<Value>,
    legacy_label: &'a str,
    legacy_plist: &'a str,
    legacy: &'a ManagedService,
}

/// Transfer one placed service from generic unit lifecycle to release-control.
///
/// Every runtime fact is established before the sole registry CAS. The target
/// service row, placement restart data, and release fallback identity disappear
/// together, so no intermediate registry can restart the legacy executable.
pub(crate) async fn handoff_release_control(
    service_name: &str,
    host: &str,
    product: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, expected_generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let registry_model = targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let target = host_channel::resolve_target(&registry_model, host)
        .map_err(click)?
        .clone();
    let directory = crate::service_resolution::directory(&document)
        .map_err(CmdError::click)?
        .ok_or_else(|| CmdError::click("registry.service_directory is not configured"))?;
    let route = directory.services.get(service_name).ok_or_else(|| {
        CmdError::click(format!(
            "service directory declares no service {service_name:?}"
        ))
    })?;
    if route.active_host != host {
        return Err(CmdError::click(format!(
            "service {service_name:?} is active on {:?}, not {host:?}",
            route.active_host
        )));
    }
    let profile_name = route.placement_profile.as_deref().ok_or_else(|| {
        CmdError::click(format!(
            "service {service_name:?} is not backed by a placement profile"
        ))
    })?;
    let profile = crate::placement::profiles(&document)
        .map_err(CmdError::click)?
        .into_iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| CmdError::click(format!("placement profile {profile_name:?} is absent")))?;
    if let Some(transaction) = crate::placement::transactions(&document)
        .map_err(CmdError::click)?
        .into_iter()
        .find(|transaction| transaction.profile == profile_name)
    {
        return Err(CmdError::click(format!(
            "placement profile {profile_name:?} is owned by active transaction {:?}",
            transaction.id
        )));
    }

    let control = crate::release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control.products.get(product).ok_or_else(|| {
        CmdError::click(format!(
            "release-control product {product:?} is not declared"
        ))
    })?;
    if policy.service != service_name {
        return Err(CmdError::click(format!(
            "release-control product {product:?} owns service {:?}, not {service_name:?}",
            policy.service
        )));
    }
    let target_policy = policy.targets.get(host).ok_or_else(|| {
        CmdError::click(format!(
            "release-control product {product:?} has no target {host:?}"
        ))
    })?;
    let desired = policy.desired.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "release-control product {product:?} has no desired release"
        ))
    })?;
    let desired_artifact = desired
        .artifacts
        .get(&target_policy.platform)
        .ok_or_else(|| {
            CmdError::click(format!(
                "desired release {:?} has no artifact for {}",
                desired.version, target_policy.platform
            ))
        })?;
    let receipt_path = handoff_receipt_path(product, &desired.version, host);
    let prior_receipt = read_handoff_receipt(&receipt_path)?;
    if let Some(receipt) = prior_receipt.as_ref() {
        let same_intent = receipt["schema"] == "stado.service-release-control-handoff.v1"
            && receipt["service"] == service_name
            && receipt["host"] == host
            && receipt["profile"] == profile_name
            && receipt["product"] == product
            && receipt["release"]["version"] == desired.version
            && receipt["release"]["rollout_generation"] == desired.rollout_generation
            && receipt["release"]["artifact_sha256"] == desired_artifact.artifact_sha256
            && receipt["release"]["manifest_sha256"] == desired_artifact.manifest_sha256;
        if !same_intent {
            return Err(CmdError::click(format!(
                "handoff receipt {} records a different operation",
                receipt_path.display()
            )));
        }
        let receipt_label = receipt["legacy"]["label"]
            .as_str()
            .ok_or_else(|| CmdError::click("handoff receipt has no legacy label"))?;
        let receipt_plist = receipt["retirement"]["plist_receipt"]["path"]
            .as_str()
            .ok_or_else(|| CmdError::click("handoff receipt has no legacy plist path"))?;
        let receipt_program = receipt["retirement"]["binary_receipt"]["path"]
            .as_str()
            .ok_or_else(|| CmdError::click("handoff receipt has no legacy binary path"))?;
        if registry_has_intended_handoff(
            &document,
            profile_name,
            service_name,
            product,
            host,
            [receipt_label, receipt_plist, receipt_program],
        ) {
            let installed_stado = format!("{}/.stado/bin/stado", target_policy.home);
            return finish_committed_handoff(
                &document,
                &target,
                &installed_stado,
                &receipt_path,
                receipt.clone(),
                &expected_generation,
                json_output,
            )
            .await;
        }
        if receipt["status"] != "prepared" && receipt["status"] != "registry_committed" {
            return Err(CmdError::click(format!(
                "handoff receipt {} says {:?}, but the registry does not match its intended handoff",
                receipt_path.display(),
                receipt["status"]
            )));
        }
        // An incomplete commit can disappear after authority recovery. Reuse
        // every live-release, legacy-identity, lease and CAS check below rather
        // than trusting the old generation or resetting its receipt by hand.
    }
    for (template_host, template) in &profile.hosts {
        let unit = template.units.get(service_name).ok_or_else(|| {
            CmdError::click(format!(
                "placement host {template_host:?} has no {service_name:?} template"
            ))
        })?;
        if unit.managed().is_none() {
            return Err(CmdError::click(format!(
                "placement service {service_name:?} is already release-controlled"
            )));
        }
    }
    let legacy_label = target_policy
        .legacy_launchd_label
        .as_deref()
        .ok_or_else(|| {
            CmdError::click(format!(
                "release-control target {host:?} has no legacy launchd label to hand off"
            ))
        })?;
    let legacy_plist = target_policy
        .legacy_launchd_plist
        .as_deref()
        .ok_or_else(|| {
            CmdError::click(format!(
                "release-control target {host:?} has no legacy launchd plist to hand off"
            ))
        })?;
    let legacy = service::declared_services(&target)
        .into_iter()
        .find(|declared| declared.name == service_name || declared.unit_id() == legacy_label)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{service_name:?} has no active target-managed legacy service row on {host:?}"
            ))
        })?;
    if legacy.unit_id() != legacy_label || legacy.path != legacy_plist {
        return Err(CmdError::click(format!(
            "legacy service row does not match release-control identity {legacy_label} at \
             {legacy_plist}"
        )));
    }

    let context = HandoffContext {
        document,
        expected_generation,
        target,
        service_name,
        host,
        product,
        json_output,
        profile_name,
        target_policy,
        desired,
        desired_artifact,
        receipt_path,
        prior_receipt,
        legacy_label,
        legacy_plist,
        legacy: &legacy,
    };
    with_service_mutation_lease(&legacy, || handoff_under_lease(context)).await
}
