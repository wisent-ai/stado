use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

/// Promote one published version into one host's desired state in one fenced
/// registry write. The platform manifest must already exist and identify the
/// canonical coordinate before `managed_versions` moves.
pub async fn promote_version(
    target_name: &str,
    binary: &str,
    version: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let managed = crate::deploy::products::product(binary)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let version = version.trim();
    if !crate::deploy::host_release::is_exact_semver(version) {
        return Err(CmdError::usage(
            "--version must name an exact immutable semantic version",
        ));
    }
    crate::cli::storage::release_api_origin()?;
    let (mut document, expected_generation) =
        crate::cli::registry::fetch_versioned_document().await?;
    let target_specs: Vec<(String, String)> = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?
        .iter()
        .map(|target| {
            let object = target
                .as_object()
                .ok_or_else(|| CmdError::click("registry target must be an object"))?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| CmdError::click("registry target has no name"))?;
            let platform = match object.get("release_platform") {
                None | Some(Value::Null) => "",
                Some(Value::String(platform)) => platform.as_str(),
                Some(_) => {
                    return Err(CmdError::click(format!(
                        "registry target {name:?} has a non-string release_platform"
                    )));
                }
            };
            Ok((name.to_string(), platform.to_string()))
        })
        .collect::<Result<_, CmdError>>()?;
    let target_specs: Vec<(String, String)> = target_specs
        .into_iter()
        .filter(|(name, _)| name == target_name)
        .collect();
    if target_specs.is_empty() {
        return Err(CmdError::click(format!(
            "{target_name} is missing from registry.targets; add the host declaration before \
             promoting a release"
        )));
    }

    // Resolve every legacy omission before mutating the in-memory document.
    // A failed channel, malformed inventory, unsupported observation, or
    // disagreement with an existing declaration aborts the one fenced write.
    let runner = crate::deploy::production_runner();
    let mut observed_platforms = std::collections::BTreeMap::new();
    let mut platforms = std::collections::BTreeSet::new();
    let mut migrated = Vec::new();
    for (name, declared) in &target_specs {
        let report = crate::deploy::host_inventory::inventory_host(name, &runner)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot verify release_platform for {name:?}: {error}"
                ))
            })?;
        if report.get("status").and_then(Value::as_str)
            != Some(crate::deploy::host_inventory::OK_STATUS)
        {
            let detail = report
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("host inventory did not complete");
            return Err(CmdError::click(format!(
                "cannot verify release_platform for {name:?}: {detail}"
            )));
        }
        if report.get("sanitizer_state").and_then(Value::as_str)
            != Some(crate::deploy::host_inventory::SANITIZER_OK)
        {
            return Err(CmdError::click(format!(
                "cannot verify release_platform for {name:?}: host inventory sanitizer failed"
            )));
        }
        let observed = report
            .get("release_platform")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "cannot verify release_platform for {name:?}: inventory omitted it"
                ))
            })?;
        let observed = crate::deploy::products::managed_platform(observed)
            .map_err(|error| CmdError::click(format!("{name}: {error}")))?;
        if !declared.is_empty() && declared != observed {
            return Err(CmdError::click(format!(
                "registry target {name:?} declares release_platform {declared}, \
                 but verified inventory observed {observed}"
            )));
        }
        if declared.is_empty() {
            migrated.push(name.clone());
        }
        observed_platforms.insert(name.clone(), observed.to_string());
        platforms.insert(observed.to_string());
    }
    for platform in &platforms {
        // A product publishes for the platforms it declares, and promoting a
        // version onto a fleet includes hosts it may not publish for at all.
        // Refused rather than skipped: a declaration a host can never receive
        // is drift this pack has no way to close.
        managed
            .platform(platform)
            .map_err(|error| CmdError::click(error.to_string()))?;
        crate::deploy::host_release::catalog_identity(managed, version, platform)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    }

    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    for target in targets {
        let object = target
            .as_object_mut()
            .ok_or_else(|| CmdError::click("registry target must be an object"))?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| CmdError::click("registry target has no name"))?
            .to_string();
        if name != target_name {
            continue;
        }
        let observed = observed_platforms.get(&name).ok_or_else(|| {
            CmdError::click(format!(
                "target {name:?} was not inventoried before promotion"
            ))
        })?;
        if object
            .get("release_platform")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty()
        {
            object.insert("release_platform".to_string(), json!(observed));
        }
        let versions = object
            .entry("managed_versions".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("managed_versions is not an object"))?;
        versions.insert(managed.name.to_string(), json!(version));
    }
    let generation =
        crate::cli::registry::push_document_if(&document, &expected_generation).await?;
    if json_output {
        print_json(&json!({
            "binary": managed.name,
            "host": target_name,
            "version": version,
            "targets": target_specs.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            "platforms": platforms,
            "migrated_release_platforms": migrated,
            "generation": generation,
        }));
    } else {
        println!(
            "{} {version} promoted to {} target(s), migrated {} release platform(s), \
             generation {generation}",
            managed.name,
            target_specs.len(),
            migrated.len(),
        );
    }
    Ok(())
}
